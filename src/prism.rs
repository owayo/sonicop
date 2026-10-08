//! Ruby 3.3 以降の構文診断を、仕様版の Prism と Translation::Parser に合わせる。

use std::borrow::Cow;
use std::ops::Range;

use crate::engine::{LiteralEncoding, declared_literal_encoding};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

pub(crate) struct Diagnostic {
    pub reason: String,
    pub range: Range<usize>,
}

struct Input<'a> {
    bytes: Cow<'a, [u8]>,
    offsets: Option<Vec<usize>>,
    binary: bool,
}

impl<'a> Input<'a> {
    fn new(source: &'a str) -> Self {
        let binary = declared_literal_encoding(source) == LiteralEncoding::Binary;
        if !binary && !source.contains("\r\n") {
            return Self {
                bytes: Cow::Borrowed(source.as_bytes()),
                offsets: None,
                binary,
            };
        }
        let mut bytes = Vec::with_capacity(source.len());
        let mut offsets = Vec::with_capacity(source.len() + 1);
        let mut characters = source.char_indices().peekable();
        while let Some((position, character)) = characters.next() {
            if character == '\r' && characters.peek().is_some_and(|(_, next)| *next == '\n') {
                characters.next();
                bytes.push(b'\n');
                offsets.push(position);
            } else if binary && let Ok(byte) = u8::try_from(u32::from(character)) {
                bytes.push(byte);
                offsets.push(position);
            } else {
                let mut encoded = [0; 4];
                let written = character.encode_utf8(&mut encoded).as_bytes();
                bytes.extend_from_slice(written);
                offsets.extend(std::iter::repeat_n(position, written.len()));
            }
        }
        offsets.push(source.len());
        Self {
            bytes: Cow::Owned(bytes),
            offsets: Some(offsets),
            binary,
        }
    }

    fn original_offset(&self, mut offset: usize) -> usize {
        offset = offset.min(self.bytes.len());
        if let Some(offsets) = &self.offsets {
            return offsets[offset];
        }
        // Translation::Parser の offset_cache は文字の途中をその文字の先頭へ戻す。
        while offset > 0
            && self
                .bytes
                .get(offset)
                .is_some_and(|byte| byte & 0xc0 == 0x80)
        {
            offset -= 1;
        }
        offset
    }

    fn displayed_range(&self, mut range: Range<usize>) -> Range<usize> {
        if range.start == range.end {
            // Lint/Syntax#diagnostic_location はゼロ幅を前方へ一文字、EOF では後方へ一文字広げる。
            if range.end < self.bytes.len() {
                range.end += 1;
                while !self.binary
                    && self
                        .bytes
                        .get(range.end)
                        .is_some_and(|byte| byte & 0xc0 == 0x80)
                {
                    range.end += 1;
                }
            } else if range.start > 0 {
                range.start -= 1;
                while !self.binary
                    && self
                        .bytes
                        .get(range.start)
                        .is_some_and(|byte| byte & 0xc0 == 0x80)
                {
                    range.start -= 1;
                }
            }
        }
        self.original_offset(range.start)..self.original_offset(range.end)
    }
}

pub(crate) fn diagnostics(context: &RuleContext<'_>) -> Option<Vec<Diagnostic>> {
    let target = context.target_ruby_version();
    let engine = context
        .setting_of::<String>("AllCops", "ParserEngine")
        .unwrap_or_else(|| "default".to_owned());
    if engine != "parser_prism"
        && (engine == "parser_whitequark" || target < RubyVersion::new(3, 3))
    {
        return None;
    }
    // convert_for_prism の版指定をそのまま使う。3.3.0 と 3.3.1 は別の設定である。
    let version = if target == RubyVersion::new(3, 3) {
        "3.3.1"
    } else if target == RubyVersion::new(3, 4) {
        "3.4.0"
    } else if target == RubyVersion::new(4, 0) || target == RubyVersion::new(3, 5) {
        "4.0.0"
    } else if target == RubyVersion::new(4, 1) {
        "4.1.0"
    } else {
        "latest"
    };
    debug_assert_eq!(sonicop_prism_compat::version(), "1.8.1");
    let source = context.source.text();
    let input = Input::new(source);
    let native = sonicop_prism_compat::parse(&input.bytes, version, input.binary)
        .expect("Prism supports every configured Ruby version");
    Some(
        native
            .into_iter()
            .map(|diagnostic| {
                let mut range = diagnostic.range;
                // Translation::Parser#error_diagnostic が渡すレンジを先に調整する。
                match diagnostic.kind.as_str() {
                    "begin_lonely_else" => range.end = range.start + 4,
                    "incomplete_variable_class"
                    | "incomplete_variable_class_3_3"
                    | "incomplete_variable_instance"
                    | "incomplete_variable_instance_3_3" => range.end += 1,
                    _ => {}
                }
                let original = input.original_offset(range.start)..input.original_offset(range.end);
                let written = source.get(original).unwrap_or("");
                let reason = translated_message(&diagnostic.kind, written, diagnostic.message);
                Diagnostic {
                    reason,
                    range: input.displayed_range(range),
                }
            })
            .collect(),
    )
}

/// Prism 1.8.1 の Translation::Parser#error_diagnostic と parser gem の MESSAGES。
fn translated_message(kind: &str, written: &str, native: String) -> String {
    let literal = match kind {
        "argument_block_multi" => "both block argument and literal block are passed",
        "argument_formal_constant" => "formal argument cannot be a constant",
        "argument_formal_class" => "formal argument cannot be a class variable",
        "argument_formal_global" => "formal argument cannot be a global variable",
        "argument_formal_ivar" => "formal argument cannot be an instance variable",
        "argument_no_forwarding_amp" => "no anonymous block parameter",
        "argument_no_forwarding_star" => "no anonymous rest parameter",
        "argument_no_forwarding_star_star" => "no anonymous keyword rest parameter",
        "begin_lonely_else" => "else without rescue is useless",
        "class_name" | "module_name" => "class or module name must be a constant literal",
        "class_in_method" => "class definition in method body",
        "def_endless_setter" => "setter method cannot be defined in an endless method definition",
        "embdoc_term" => {
            "embedded document meets end of file (and they embark on a romantic journey)"
        }
        "module_in_method" => "module definition in method body",
        "numbered_parameter_ordinary" => "ordinary parameter is defined",
        "numbered_parameter_outer_scope" => "numbered parameter is already used in an outer scope",
        "parameter_name_repeat" => "duplicate argument name",
        "singleton_for_literals" => "cannot define a singleton method for a literal",
        "string_literal_eof" => "unterminated string meets end of file",
        "write_target_in_method" => "dynamic constant assignment",
        "incomplete_variable_class" | "incomplete_variable_class_3_3" => {
            return format!("`{written}' is not allowed as a class variable name");
        }
        "incomplete_variable_instance" | "incomplete_variable_instance_3_3" => {
            return format!("`{written}' is not allowed as an instance variable name");
        }
        "invalid_variable_global" | "invalid_variable_global_3_3" => {
            return format!("`{written}' is not allowed as a global variable name");
        }
        "parameter_circular" => return format!("circular argument reference {written}"),
        "parameter_numbered_reserved" => {
            return format!("{written} is reserved for numbered parameter");
        }
        "regexp_unknown_options" => {
            return format!(
                "unknown regexp options: {}",
                written.chars().skip(1).collect::<String>()
            );
        }
        "unexpected_token_ignore" => return format!("unexpected token {written}"),
        _ => return native,
    };
    literal.to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_prism_abi_accepts_each_translation_parser_version() {
        assert_eq!(sonicop_prism_compat::version(), "1.8.1");
        for version in ["3.3.1", "3.4.0", "4.0.0", "4.1.0", "latest"] {
            assert!(
                sonicop_prism_compat::parse(b"foo(1)\n", version, false)
                    .unwrap()
                    .is_empty(),
                "{version}"
            );
        }
        assert!(sonicop_prism_compat::parse(b"", "unknown", false).is_err());
        assert!(sonicop_prism_compat::parse(b"", "latest\0", false).is_err());
        // C の parser 解放後も診断文字列を所有し、UTF-8 のバイト位置を保持する。
        let errors =
            sonicop_prism_compat::parse("名前 = 1\n(\n".as_bytes(), "3.3.1", false).unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "expected a matching `)`");
    }
}
