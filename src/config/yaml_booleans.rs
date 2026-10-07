use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::CStr;
use std::mem::MaybeUninit;

use unsafe_libyaml as unsafe_yaml;

/// Psych は YAML 1.1 の `yes/no/on/off` を真偽値にする。
/// 値にしてからでは引用符の有無を失うため、同じ YAML スキャナの plain scalar だけを変換する。
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
                }
                unsafe_yaml::YAML_TAG_TOKEN => {
                    let tag = token.data.tag;
                    let handle = CStr::from_ptr(tag.handle.cast()).to_bytes();
                    let suffix = CStr::from_ptr(tag.suffix.cast()).to_bytes();
                    // !!、verbatim tag、%TAG で同じ URI を指す表記を揃える。
                    let mut uri = tag_handles
                        .get(handle)
                        .cloned()
                        .unwrap_or_else(|| handle.to_vec());
                    uri.extend_from_slice(suffix);
                    explicit_tag = if uri == b"tag:yaml.org,2002:bool" {
                        Some(true)
                    } else if uri.starts_with(b"tag:yaml.org,2002:")
                        || suffix == b"ruby/regexp"
                        || suffix == b"ruby/symbol"
                    {
                        Some(false)
                    } else {
                        None
                    };
                }
                unsafe_yaml::YAML_ANCHOR_TOKEN => {}
                unsafe_yaml::YAML_SCALAR_TOKEN => {
                    let scalar = token.data.scalar;
                    if (scalar.style == unsafe_yaml::YAML_PLAIN_SCALAR_STYLE
                        && explicit_tag != Some(false))
                        || explicit_tag == Some(true)
                    {
                        let value =
                            std::slice::from_raw_parts(scalar.value, scalar.length as usize);
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
                            replacements.push((
                                token.start_mark.index as usize,
                                token.end_mark.index as usize,
                                replacement,
                            ));
                        }
                    }
                    explicit_tag = None;
                }
                _ => explicit_tag = None,
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
        normalized.push_str(replacement);
        previous = end;
    }
    normalized.push_str(&contents[previous..]);
    Cow::Owned(normalized)
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
        let Value::Tagged(tagged) = &parsed["tagged"] else {
            panic!()
        };
        assert_eq!(tagged.value, Value::Bool(false));
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
