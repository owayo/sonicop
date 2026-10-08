use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::CStr;
use std::mem::MaybeUninit;

use unsafe_libyaml as unsafe_yaml;

/// Psych は YAML 1.1 の真偽値と、引用されていない `:name` を Ruby の Symbol にする。
/// 引用符とタグの解決結果は値にしてからでは失われるため、Psych の scalar 判定を字句上で補う。
pub(super) fn normalize(contents: &str) -> Cow<'_, str> {
    // 本家は BOM 付き設定を拒否する。スキャナの位置も BOM を数えないため、変換せず診断へ渡す。
    if contents.starts_with('\u{feff}') {
        return Cow::Borrowed(contents);
    }
    let mut storage = Box::<unsafe_yaml::yaml_parser_t>::new_uninit();
    // 入力の参照と parser のアドレスは、Scanner が破棄されるまで変わらない。
    let mut scanner = unsafe {
        if unsafe_yaml::yaml_parser_initialize(storage.as_mut_ptr()).fail {
            return Cow::Borrowed(contents);
        }
        let mut scanner = Scanner(storage.assume_init());
        unsafe_yaml::yaml_parser_set_input_string(
            &mut *scanner.0,
            contents.as_ptr(),
            contents.len() as u64,
        );
        scanner
    };
    let mut replacements = Vec::new();
    let mut explicit_tag = None;
    let mut explicit_tag_range = None;
    let mut symbol_tag = false;
    let mut binary_tag = false;
    let default_tags = || HashMap::from([(b"!!".to_vec(), b"tag:yaml.org,2002:".to_vec())]);
    let mut tag_handles = default_tags();
    loop {
        let mut storage = MaybeUninit::<unsafe_yaml::yaml_token_t>::uninit();
        // scan が成功した token だけを読み、次の scan の前に必ず解放する。
        let finished = unsafe {
            if unsafe_yaml::yaml_parser_scan(&mut *scanner.0, storage.as_mut_ptr()).fail {
                // 構文エラーは本来のデシリアライザに診断させ、途中の変換は採用しない。
                return Cow::Borrowed(contents);
            }
            let mut token = storage.assume_init();
            let finished = token.type_ == unsafe_yaml::YAML_STREAM_END_TOKEN;
            match token.type_ {
                unsafe_yaml::YAML_TAG_DIRECTIVE_TOKEN => {
                    let directive = token.data.tag_directive;
                    tag_handles.insert(
                        CStr::from_ptr(directive.handle.cast()).to_bytes().to_vec(),
                        CStr::from_ptr(directive.prefix.cast()).to_bytes().to_vec(),
                    );
                }
                unsafe_yaml::YAML_DOCUMENT_END_TOKEN => {
                    tag_handles = default_tags();
                    explicit_tag = None;
                    explicit_tag_range = None;
                    symbol_tag = false;
                    binary_tag = false;
                }
                unsafe_yaml::YAML_TAG_TOKEN => {
                    explicit_tag_range = Some((
                        token.start_mark.index as usize,
                        token.end_mark.index as usize,
                    ));
                    let tag = token.data.tag;
                    let handle = CStr::from_ptr(tag.handle.cast()).to_bytes();
                    let suffix = CStr::from_ptr(tag.suffix.cast()).to_bytes();
                    // !!、verbatim tag、%TAG で同じ URI を指す表記を揃える。
                    let mut uri = tag_handles
                        .get(handle)
                        .cloned()
                        .unwrap_or_else(|| handle.to_vec());
                    uri.extend_from_slice(suffix);
                    // serde_yaml_ng は標準 binary タグを String に落とすため、タグを保存できる表記へ揃える。
                    binary_tag = uri == b"tag:yaml.org,2002:binary" || uri == b"!binary";
                    if uri == b"tag:yaml.org,2002:binary" {
                        replacements.push((
                            token.start_mark.index as usize,
                            token.end_mark.index as usize,
                            "!<!binary>".to_owned(),
                        ));
                    }
                    symbol_tag = uri.starts_with(b"!ruby/sym");
                    if symbol_tag {
                        replacements.push((
                            token.start_mark.index as usize,
                            token.end_mark.index as usize,
                            "!<!ruby/symbol>".to_owned(),
                        ));
                    }
                    let local_string = uri == b"!str"
                        || uri.starts_with(b"!str:")
                        || uri == b"!ruby/string"
                        || uri.starts_with(b"!ruby/string:");
                    if local_string {
                        replacements.push((
                            token.start_mark.index as usize,
                            token.end_mark.index as usize,
                            "!<tag:yaml.org,2002:str>".to_owned(),
                        ));
                    }
                    explicit_tag = if uri == b"tag:yaml.org,2002:bool" {
                        Some(true)
                    } else if uri == b"tag:yaml.org,2002:str"
                        || uri == b"tag:yaml.org,2002:binary"
                        || uri == b"!binary"
                        || uri == b"!ruby/regexp"
                        || symbol_tag
                        || local_string
                    {
                        Some(false)
                    } else {
                        None
                    };
                }
                unsafe_yaml::YAML_ANCHOR_TOKEN => {}
                unsafe_yaml::YAML_SCALAR_TOKEN => {
                    let scalar = token.data.scalar;
                    let value = std::slice::from_raw_parts(scalar.value, scalar.length as usize);
                    // 引用した ":name" は String のまま残す。未知のタグは Psych と同様に無視する。
                    if symbol_tag || binary_tag {
                        // Symbol の名前と binary の符号化文字列は、数値・真偽値へ変換すると元に戻せない。
                        let name = std::str::from_utf8(value).expect("UTF-8 scalar");
                        replacements.push((
                            token.start_mark.index as usize,
                            token.end_mark.index as usize,
                            serde_json::to_string(name).expect("UTF-8 symbol"),
                        ));
                    }
                    let implicit = explicit_tag.is_none()
                        && (scalar.style == unsafe_yaml::YAML_PLAIN_SCALAR_STYLE
                            || explicit_tag_range.is_some());
                    if implicit
                        && let Ok(text) = std::str::from_utf8(value)
                        && let Some(name) = psych_symbol_name(text)
                    {
                        if let Some((start, end)) = explicit_tag_range {
                            replacements.push((start, end, String::new()));
                        }
                        replacements.push((
                            token.start_mark.index as usize,
                            token.end_mark.index as usize,
                            format!(
                                "!<!ruby/symbol> {}",
                                serde_json::to_string(name).expect("UTF-8 symbol")
                            ),
                        ));
                    }
                    if ((scalar.style == unsafe_yaml::YAML_PLAIN_SCALAR_STYLE || implicit)
                        && explicit_tag != Some(false))
                        || explicit_tag == Some(true)
                    {
                        let replacement = if [b"yes".as_slice(), b"true", b"on"]
                            .iter()
                            .any(|word| value.eq_ignore_ascii_case(word))
                        {
                            Some("true")
                        } else if [b"no".as_slice(), b"false", b"off"]
                            .iter()
                            .any(|word| value.eq_ignore_ascii_case(word))
                        {
                            Some("false")
                        } else {
                            None
                        };
                        if let Some(replacement) = replacement {
                            if explicit_tag.is_none()
                                && let Some((start, end)) = explicit_tag_range
                            {
                                replacements.push((start, end, String::new()));
                            }
                            replacements.push((
                                token.start_mark.index as usize,
                                token.end_mark.index as usize,
                                replacement.to_owned(),
                            ));
                        }
                    }
                    explicit_tag = None;
                    explicit_tag_range = None;
                    symbol_tag = false;
                    binary_tag = false;
                }
                _ => {
                    explicit_tag = None;
                    explicit_tag_range = None;
                    symbol_tag = false;
                    binary_tag = false;
                }
            }
            unsafe_yaml::yaml_token_delete(&mut token);
            finished
        };
        if finished {
            break;
        }
    }
    if replacements.is_empty() {
        return Cow::Borrowed(contents);
    }
    let mut normalized = String::with_capacity(contents.len());
    let mut previous = 0;
    for (start, end, replacement) in replacements {
        normalized.push_str(&contents[previous..start]);
        normalized.push_str(&replacement);
        // ブロックスカラーの token は最後の行末まで含む。引用値へ置き換えても、
        // 次の mapping entry を同じ行へつなげてはいけない。
        let replaced = &contents[start..end];
        if replaced.ends_with("\r\n") {
            normalized.push_str("\r\n");
        } else if replaced.ends_with('\n') {
            normalized.push('\n');
        } else if replaced.ends_with('\r') {
            normalized.push('\r');
        }
        previous = end;
    }
    normalized.push_str(&contents[previous..]);
    Cow::Owned(normalized)
}

/// Psych の quoted Symbol は末尾を固定せず、最も後ろの引用符までを名前にする。
fn psych_symbol_name(text: &str) -> Option<&str> {
    if text.contains('\n') {
        return None;
    }
    let rest = text.strip_prefix(':').filter(|rest| !rest.is_empty())?;
    if let Some(quote @ ('"' | '\'')) = rest.chars().next()
        && let Some(end) = rest[1..].rfind(quote)
    {
        let name = &rest[1..1 + end];
        return Some(name.strip_prefix(':').unwrap_or(name));
    }
    Some(rest)
}

struct Scanner(Box<unsafe_yaml::yaml_parser_t>);

impl Drop for Scanner {
    fn drop(&mut self) {
        // parser は初期化済みで、token ごとの領域は既に解放されている。
        unsafe { unsafe_yaml::yaml_parser_delete(&mut *self.0) };
    }
}

#[cfg(test)]
mod tests {
    use serde_yaml_ng::Value;

    use super::normalize;

    #[test]
    fn binary_tag_spellings_preserve_the_encoded_bytes() {
        for input in [
            "value: !binary gg==\n",
            "value: !!binary gg==\n",
            "value: !<tag:yaml.org,2002:binary> gg==\n",
            "%TAG !bytes! tag:yaml.org,2002:\n---\nvalue: !bytes!binary |-\n  gg==\n",
            "%TAG ! tag:yaml.org,2002:\n---\nvalue: !binary gg==\n",
        ] {
            let parsed: Value = serde_yaml_ng::from_str(&normalize(input)).unwrap();
            let Value::Tagged(tagged) = &parsed["value"] else {
                panic!("binary tag was lost: {input}");
            };
            assert_eq!(tagged.tag, "binary");
            assert_eq!(tagged.value.as_str(), Some("gg=="));
        }
        // Psych はこの表記をすべて String としてデコードする。YAML の暗黙型へ落とさない。
        for text in ["1234", "false", "~", "0x1F", "1_0"] {
            let input = format!("value: !binary {text}\n");
            let parsed: Value = serde_yaml_ng::from_str(&normalize(&input)).unwrap();
            let Value::Tagged(tagged) = &parsed["value"] else {
                panic!("binary tag was lost: {input}");
            };
            assert_eq!(tagged.value.as_str(), Some(text));
        }
    }

    #[test]
    fn ruby_symbols_keep_quotes_tags_and_aliases_distinct() {
        // Psych の実出力で、引用したコロン付き String と Symbol が別物であることを確認した。
        let input = "values:\n  - :conservative\n  - \":conservative\"\n  - !ruby/symbol conservative\n  - !ruby/symbol :conservative\n  - !unknown :conservative\n  - !!str :conservative\n  - &mode :\"日本語\"\n  - *mode\n  - !unknown &first :conservative\n  - &second !unknown :conservative\n";
        let parsed: Value = serde_yaml_ng::from_str(&normalize(input)).unwrap();
        let values = parsed["values"].as_sequence().unwrap();
        for index in [0, 2, 4, 8, 9] {
            let Value::Tagged(tagged) = &values[index] else {
                panic!("expected Ruby Symbol at {index}")
            };
            assert_eq!(tagged.tag, "ruby/symbol");
            assert_eq!(tagged.value, Value::String("conservative".into()));
        }
        for index in [1, 5] {
            assert_eq!(values[index], Value::String(":conservative".into()));
        }
        let Value::Tagged(tagged) = &values[3] else {
            panic!("expected explicitly tagged Symbol")
        };
        assert_eq!(tagged.value, Value::String(":conservative".into()));
        for index in [6, 7] {
            let Value::Tagged(tagged) = &values[index] else {
                panic!("expected aliased Symbol")
            };
            assert_eq!(tagged.value, Value::String("日本語".into()));
        }
    }

    #[test]
    fn psych_symbol_fallbacks_preserve_the_original_value_type() {
        // Psych.unsafe_load の実出力を正解にし、未知タグの引用値と明示 Symbol を分ける。
        for (input, name) in [
            ("value: :\"a\"b\n", "a"),
            ("value: :\":a\"\n", "a"),
            ("value: !unknown \":conservative\"\n", "conservative"),
            ("value: !ruby/symbol \"false\"\n", "false"),
            ("value: !ruby/sym :a\n", ":a"),
            ("value: !!int :a\n", "a"),
            ("value: !!null :a\n", "a"),
            (
                "%TAG ! tag:yaml.org,2002:\n---\nvalue: :conservative\n",
                "conservative",
            ),
        ] {
            let parsed: Value = serde_yaml_ng::from_str(&normalize(input)).unwrap();
            let Value::Tagged(tagged) = &parsed["value"] else {
                panic!("Symbol was lost: {input}");
            };
            assert_eq!(tagged.tag, "ruby/symbol");
            assert_eq!(tagged.value, Value::String(name.into()));
        }
        let parsed: Value =
            serde_yaml_ng::from_str(&normalize("value: !unknown \"false\"\n")).unwrap();
        assert_eq!(parsed["value"], Value::Bool(false));
        for input in ["value: !str :a\n", "value: !ruby/string :a\n"] {
            let parsed: Value = serde_yaml_ng::from_str(&normalize(input)).unwrap();
            assert_eq!(parsed["value"], Value::String(":a".into()));
        }
    }

    #[test]
    fn legacy_booleans_preserve_quoted_and_block_strings() {
        let input = "values: [yes, Yes, YES, on, On, ON, no, No, NO, off, Off, OFF, 'no', \"off\"]\nblock: |\n  no\n  off\n";
        let parsed: Value = serde_yaml_ng::from_str(&normalize(input)).unwrap();
        let values = parsed["values"].as_sequence().unwrap();
        assert!(values[..6].iter().all(|value| value == &Value::Bool(true)));
        assert!(
            values[6..12]
                .iter()
                .all(|value| value == &Value::Bool(false))
        );
        assert_eq!(values[12], Value::String("no".into()));
        assert_eq!(values[13], Value::String("off".into()));
        assert_eq!(parsed["block"], Value::String("no\noff\n".into()));
    }

    #[test]
    fn anchors_tags_and_unicode_keep_their_yaml_meaning() {
        let input = "日本語: &flag nO\nalias: *flag\ntext: !!str no\nboolean: !!bool 'oFf'\ntagged: !example no\ncollection: !example [no]\nmixed: fAlSe\n";
        let parsed: Value = serde_yaml_ng::from_str(&normalize(input)).unwrap();
        assert_eq!(parsed["日本語"], Value::Bool(false));
        assert_eq!(parsed["alias"], Value::Bool(false));
        assert_eq!(parsed["text"], Value::String("no".into()));
        assert_eq!(parsed["boolean"], Value::Bool(false));
        assert_eq!(parsed["mixed"], Value::Bool(false));
        assert_eq!(parsed["tagged"], Value::Bool(false));
        let Value::Tagged(collection) = &parsed["collection"] else {
            panic!()
        };
        assert_eq!(collection.value[0], Value::Bool(false));
    }

    #[test]
    fn boolean_tag_uris_and_directives_resolve_quoted_values() {
        let input = "%TAG !core! tag:yaml.org,2002:\n---\nvalues: [!!bool 'off', !<tag:yaml.org,2002:bool> off, !core!bool \"nO\", !core!str no]\n";
        let parsed: Value = serde_yaml_ng::from_str(&normalize(input)).unwrap();
        let values = parsed["values"].as_sequence().unwrap();
        assert!(values[..3].iter().all(|value| value == &Value::Bool(false)));
        assert_eq!(values[3], Value::String("no".into()));
    }

    #[test]
    fn a_bom_document_is_not_rewritten() {
        let input = "\u{feff}日本語: no\n";
        assert!(matches!(normalize(input), std::borrow::Cow::Borrowed(_)));
        assert_eq!(normalize(input), input);
    }

    #[test]
    fn unchanged_and_lexically_invalid_documents_are_borrowed() {
        assert!(matches!(
            normalize("value: 'no'\n"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            normalize("value: no\nbad: \"\n"),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
