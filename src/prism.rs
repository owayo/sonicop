//! Ruby 3.3 以降の構文診断を、仕様版の Prism と Translation::Parser に合わせる。

use std::borrow::Cow;
use std::ops::Range;

use crate::engine::{LiteralEncoding, declared_literal_encoding};
use crate::ruby_version::RubyVersion;
use crate::rules::{ParserEngine, RuleContext};

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
    if context.parser_engine() == ParserEngine::Whitequark {
        return None;
    }
    let version = parser_version(context.target_ruby_version());
    debug_assert_eq!(sonicop_prism_compat::version(), "1.8.1");
    let source = context.source.text();
    let input = Input::new(source);
    let parsed = sonicop_prism_compat::parse_detailed(&input.bytes, version, input.binary)
        .expect("Prism supports every configured Ruby version");
    let mut native = parsed.diagnostics;
    if native.is_empty() {
        for regexp in parsed.regexps {
            let mut bytes = Vec::new();
            let mut encoding = LiteralEncoding::SevenBit;
            let mut first = true;
            let mut ascii = true;
            let mut compatible = true;
            for part in regexp.parts {
                let part_encoding = match part.encoding {
                    sonicop_prism_compat::StringEncoding::Binary => LiteralEncoding::Binary,
                    sonicop_prism_compat::StringEncoding::UsAscii => LiteralEncoding::SevenBit,
                    sonicop_prism_compat::StringEncoding::Utf8 => LiteralEncoding::Text,
                };
                let part_ascii = part.bytes.is_ascii();
                if first {
                    encoding = part_encoding;
                    first = false;
                }
                if ascii && !part_ascii {
                    encoding = part_encoding;
                } else if !ascii && !part_ascii && encoding != part_encoding {
                    compatible = false;
                    break;
                }
                ascii &= part_ascii;
                bytes.extend_from_slice(&part.bytes);
            }
            if compatible
                && let Err(message) =
                    crate::regexp_validation::validate(&bytes, encoding, &regexp.flags)
            {
                native.push(sonicop_prism_compat::Diagnostic {
                    range: regexp.range,
                    kind: "invalid_regexp".into(),
                    message,
                });
            }
        }
    }
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

// convert_for_prism の版指定をそのまま使う。3.3.0 と 3.3.1 は別の設定である。
fn parser_version(target: RubyVersion) -> &'static str {
    if target == RubyVersion::new(3, 3) {
        "3.3.1"
    } else if target == RubyVersion::new(3, 4) {
        "3.4.0"
    } else if target == RubyVersion::new(4, 0) || target == RubyVersion::new(3, 5) {
        "4.0.0"
    } else if target == RubyVersion::new(4, 1) {
        "4.1.0"
    } else {
        "latest"
    }
}

/// 診断用パーサが受理しすぎても、有効な原本を実行不能なファイルへ上書きしない。
pub(crate) fn would_invalidate(
    original: impl AsRef<[u8]>,
    corrected: impl AsRef<[u8]>,
    target: RubyVersion,
) -> bool {
    let original = original.as_ref();
    let corrected = corrected.as_ref();
    // latest では対象版で合法な構文まで拒み得る。3.3 未満は Prism の対応下限を使い、
    // 原本を同じ設定で受理できた場合だけ、出力のエラーを新しい破壊と判断する。
    let version = parser_version(target.max(RubyVersion::new(3, 3)));
    let broken = script_has_errors(corrected, version)
        || polyglot_launcher_count(&String::from_utf8_lossy(corrected))
            < polyglot_launcher_count(&String::from_utf8_lossy(original));
    broken && !script_has_errors(original, version)
}

/// 対象括弧補正の保護でも、既存の main_script・元 byte の受理条件を共有する。
/// 3.3 未満の下限への代用は、その旧版で有効だったことの証拠にはしない。
pub(crate) fn script_is_valid(source: impl AsRef<[u8]>, target: RubyVersion) -> bool {
    !script_has_errors(source, parser_version(target.max(RubyVersion::new(3, 3))))
}

fn script_has_errors(source: impl AsRef<[u8]>, version: &str) -> bool {
    !sonicop_prism_compat::parse_script(source.as_ref(), version)
        .expect("Prism supports every configured Ruby version")
        .is_empty()
}

/// Ruby の -x 探索前の部分はシェル等にも読まれるため、Ruby の死んだ式でも削除しない。
fn ruby_script_prefix(source: &str) -> Option<&str> {
    let mut lines = source.split_inclusive('\n');
    let first = lines.next()?;
    if !first.starts_with("#!") || first.contains("ruby") {
        return None;
    }
    let mut end = first.len();
    for line in lines {
        end += line.len();
        if line.starts_with("#!") && line.contains("ruby") {
            return Some(&source[..end]);
        }
    }
    None
}

/// Ruby が無視する起動用 exec の消去も防ぐ。シェル全体の意味を検証するものではない。
fn polyglot_launcher_count(source: &str) -> usize {
    ruby_script_prefix(source).map_or(0, |prefix| {
        prefix
            .lines()
            .filter(|line| {
                line.trim_start_matches([' ', '\t'])
                    .strip_prefix("exec")
                    .is_some_and(|rest| rest.starts_with([' ', '\t']))
            })
            .count()
    })
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
    use super::{ruby_script_prefix, script_has_errors, would_invalidate};
    use crate::ruby_version::RubyVersion;

    #[test]
    fn correction_validation_keeps_the_target_grammar_and_requires_a_valid_original() {
        let original = "a[0] = value\n";
        let corrected = "a[&block] = value\n";
        assert!(!would_invalidate(
            original,
            corrected,
            RubyVersion::new(3, 3)
        ));
        assert!(would_invalidate(
            original,
            corrected,
            RubyVersion::new(3, 4)
        ));
        assert!(!would_invalidate("if\n", "break\n", RubyVersion::new(2, 7)));
        // 診断用の部分式設定は従来どおりだが、実行するファイルの break は拒否する。
        assert!(
            sonicop_prism_compat::parse(b"break\n", "3.3.1", false)
                .unwrap()
                .is_empty()
        );
        assert!(script_has_errors("break\n", "3.3.1"));
    }

    #[test]
    fn script_validation_preserves_ascii_literal_encoding_without_changing_diagnostics() {
        for header in [
            "# coding: Shift_JIS",
            "# coding: EUC-JP",
            "# coding: ISO-8859-1",
            "# coding: binary",
            "\u{feff}# coding: US-ASCII",
            "  # coding: US-ASCII",
            "# vim: set fileencoding=us-ascii :",
            "# coding: 646",
            "# coding: ANSI_X3.4-1968",
        ] {
            for newline in ["\n", "\r\n"] {
                let original = format!("{header}{newline}\"/\\u1234#{{ }}\\x80/\"{newline}");
                assert!(
                    would_invalidate(
                        &original,
                        original.replace("#{ }", ""),
                        RubyVersion::new(2, 7)
                    ),
                    "{header}"
                );
            }
        }
        let original = "# coding: US-ASCII\n\"/\\u1234#{ }\\x80/\"\n";
        let corrected = original.replace("#{ }", "");
        assert!(would_invalidate(
            original,
            &corrected,
            RubyVersion::new(2, 7)
        ));
        assert!(would_invalidate(
            original,
            &corrected,
            RubyVersion::new(3, 3)
        ));
        // UTF-8 なら混在ではない。診断用の復号済み入力も、従来の設定で解析する。
        let utf8 = original.replace("US-ASCII", "UTF-8");
        assert!(!would_invalidate(
            &utf8,
            utf8.replace("#{ }", ""),
            RubyVersion::new(2, 7)
        ));
        assert!(
            sonicop_prism_compat::parse(corrected.as_bytes(), "3.3.1", false)
                .unwrap()
                .is_empty()
        );
        assert!(!script_has_errors(
            "# coding: US-ASCII\n\"\\x80\"\n",
            "3.3.1"
        ));
    }

    #[test]
    fn polyglot_launchers_preserve_the_prefix_but_allow_ruby_body_corrections() {
        let prefix = "#!/bin/sh\nexec ruby -x \"$0\" \"$@\" if false\n#!ruby -w\n";
        let original = format!("{prefix}puts(1)\n");
        let body_only = format!("{prefix}puts 1\n");
        let commented = original.replacen("exec", "# example: <<EOF\n\n  exec", 1);
        assert!(!would_invalidate(
            &original,
            &commented,
            RubyVersion::new(2, 7)
        ));
        let twice = original.replacen("exec", "exec first if false\nexec", 1);
        assert!(would_invalidate(&twice, &original, RubyVersion::new(2, 7)));
        let lost_launcher = "#!/bin/sh\n#!ruby -w\nputs 1\n";
        let lost_entry = "#!/bin/sh\n# !ruby -w\nputs 1\n";
        assert_eq!(ruby_script_prefix(&original), Some(prefix));
        assert!(!would_invalidate(
            &original,
            &body_only,
            RubyVersion::new(2, 7)
        ));
        assert!(would_invalidate(
            &original,
            lost_launcher,
            RubyVersion::new(2, 7)
        ));
        assert!(would_invalidate(
            &original,
            lost_entry,
            RubyVersion::new(2, 7)
        ));
        for first in [
            "#!/usr/bin/env ruby\n",
            "#!/usr/bin/env jruby\n",
            "#!/bin/sh # ruby\n",
        ] {
            assert_eq!(ruby_script_prefix(&format!("{first}puts 1\n")), None);
        }
        assert_eq!(ruby_script_prefix("#!/bin/sh\nputs 1\n"), None);
    }

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
