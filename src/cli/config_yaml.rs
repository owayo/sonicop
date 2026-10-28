use std::ffi::{CString, c_void};
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::LazyLock;

use anyhow::{Result, ensure};
use base64::Engine as _;
use serde_yaml_ng::Value;
use unsafe_libyaml as unsafe_yaml;

/// Psych::Visitors::YAMLTree の型と style の選択だけを行い、引用・折り返しは libyaml に任せる。
pub(super) fn dump(value: &Value) -> Result<String> {
    let mut storage = Box::<unsafe_yaml::yaml_emitter_t>::new_uninit();
    // emitter と出力先のアドレスは、最後の flush と delete まで固定する。
    let mut emitter = unsafe {
        ensure!(
            unsafe_yaml::yaml_emitter_initialize(storage.as_mut_ptr()).ok,
            "cannot initialize YAML emitter"
        );
        Emitter(storage.assume_init())
    };
    let mut output = Vec::<u8>::new();
    unsafe {
        unsafe_yaml::yaml_emitter_set_unicode(&mut *emitter.0, true);
        unsafe_yaml::yaml_emitter_set_indent(&mut *emitter.0, 2);
        unsafe_yaml::yaml_emitter_set_width(&mut *emitter.0, 80);
        unsafe_yaml::yaml_emitter_set_output(
            &mut *emitter.0,
            write_yaml,
            (&mut output as *mut Vec<u8>).cast(),
        );
    }
    emitter.event(|event| unsafe {
        unsafe_yaml::yaml_stream_start_event_initialize(event, unsafe_yaml::YAML_UTF8_ENCODING).ok
    })?;
    emitter.event(|event| unsafe {
        unsafe_yaml::yaml_document_start_event_initialize(
            event,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            true,
        )
        .ok
    })?;
    emitter.value(value)?;
    emitter.event(|event| unsafe {
        unsafe_yaml::yaml_document_end_event_initialize(event, true).ok
    })?;
    emitter.event(|event| unsafe { unsafe_yaml::yaml_stream_end_event_initialize(event).ok })?;
    drop(emitter);
    Ok(String::from_utf8(output)?)
}

unsafe fn write_yaml(data: *mut c_void, buffer: *mut u8, size: u64) -> i32 {
    // callback は dump の生存中に同期実行され、他の参照と出力先を共有しない。
    unsafe {
        (&mut *data.cast::<Vec<u8>>())
            .extend_from_slice(std::slice::from_raw_parts(buffer, size as usize));
    }
    1
}

struct Emitter(Box<unsafe_yaml::yaml_emitter_t>);

impl Drop for Emitter {
    fn drop(&mut self) {
        unsafe {
            unsafe_yaml::yaml_emitter_delete(&mut *self.0);
        }
    }
}

impl Emitter {
    fn event(
        &mut self,
        initialize: impl FnOnce(*mut unsafe_yaml::yaml_event_t) -> bool,
    ) -> Result<()> {
        let mut event = MaybeUninit::<unsafe_yaml::yaml_event_t>::uninit();
        ensure!(
            initialize(event.as_mut_ptr()),
            "cannot initialize YAML event"
        );
        // emit は成功・失敗のどちらでも event の所有権を引き取る。
        ensure!(
            unsafe { unsafe_yaml::yaml_emitter_emit(&mut *self.0, event.as_mut_ptr()).ok },
            "cannot emit YAML configuration"
        );
        Ok(())
    }

    fn scalar(
        &mut self,
        text: &str,
        tag: Option<&str>,
        style: unsafe_yaml::yaml_scalar_style_t,
    ) -> Result<()> {
        self.scalar_with_implicit(text, tag, style, tag.is_none())
    }

    fn scalar_with_implicit(
        &mut self,
        text: &str,
        tag: Option<&str>,
        style: unsafe_yaml::yaml_scalar_style_t,
        quoted_implicit: bool,
    ) -> Result<()> {
        let tag = tag.map(CString::new).transpose()?;
        let length = i32::try_from(text.len())?;
        self.event(|event| unsafe {
            unsafe_yaml::yaml_scalar_event_initialize(
                event,
                ptr::null(),
                tag.as_ref().map_or(ptr::null(), |tag| tag.as_ptr().cast()),
                text.as_ptr(),
                length,
                tag.is_none(),
                quoted_implicit,
                style,
            )
            .ok
        })
    }

    fn value(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Null => self.scalar("", None, unsafe_yaml::YAML_ANY_SCALAR_STYLE),
            Value::Bool(value) => self.scalar(
                if *value { "true" } else { "false" },
                None,
                unsafe_yaml::YAML_ANY_SCALAR_STYLE,
            ),
            Value::Number(value) => {
                self.scalar(&value.to_string(), None, unsafe_yaml::YAML_ANY_SCALAR_STYLE)
            }
            Value::String(value) => self.string(value),
            Value::Sequence(values) => {
                self.event(|event| unsafe {
                    unsafe_yaml::yaml_sequence_start_event_initialize(
                        event,
                        ptr::null(),
                        ptr::null(),
                        true,
                        unsafe_yaml::YAML_BLOCK_SEQUENCE_STYLE,
                    )
                    .ok
                })?;
                for value in values {
                    self.value(value)?;
                }
                self.event(|event| unsafe {
                    unsafe_yaml::yaml_sequence_end_event_initialize(event).ok
                })
            }
            Value::Mapping(values) => {
                self.event(|event| unsafe {
                    unsafe_yaml::yaml_mapping_start_event_initialize(
                        event,
                        ptr::null(),
                        ptr::null(),
                        true,
                        unsafe_yaml::YAML_BLOCK_MAPPING_STYLE,
                    )
                    .ok
                })?;
                for (key, value) in values {
                    self.value(key)?;
                    self.value(value)?;
                }
                self.event(|event| unsafe {
                    unsafe_yaml::yaml_mapping_end_event_initialize(event).ok
                })
            }
            Value::Tagged(value) => {
                let tag = value.tag.to_string();
                if tag == "!ruby/symbol"
                    && let Some(name) = value.value.as_str()
                {
                    if name.is_empty() {
                        return self.scalar(
                            "",
                            Some("!ruby/symbol"),
                            unsafe_yaml::YAML_ANY_SCALAR_STYLE,
                        );
                    }
                    // 引用に落ちた Symbol にも非特定タグ ! を残し、String への変化を防ぐ。
                    return self.scalar_with_implicit(
                        &format!(":{name}"),
                        None,
                        unsafe_yaml::YAML_ANY_SCALAR_STYLE,
                        false,
                    );
                }
                if let Some(text) = value.value.as_str() {
                    if tag == "!binary" {
                        let engine = base64::engine::general_purpose::GeneralPurpose::new(
                            &base64::alphabet::STANDARD,
                            base64::engine::general_purpose::GeneralPurposeConfig::new()
                                .with_decode_padding_mode(
                                    base64::engine::DecodePaddingMode::Indifferent,
                                )
                                .with_decode_allow_trailing_bits(true),
                        );
                        let encoded = text
                            .bytes()
                            .filter(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                            })
                            .collect::<Vec<_>>();
                        let decoded = engine.decode(encoded)?;
                        if decoded.is_ascii() {
                            return self.string(std::str::from_utf8(&decoded)?);
                        }
                        return self.scalar(
                            &engine.encode(decoded),
                            Some("!binary"),
                            unsafe_yaml::YAML_LITERAL_SCALAR_STYLE,
                        );
                    }
                    self.scalar(text, Some(&tag), unsafe_yaml::YAML_ANY_SCALAR_STYLE)
                } else {
                    self.value(&value.value)
                }
            }
        }
    }

    fn string(&mut self, text: &str) -> Result<()> {
        let tag = (text == "<<").then_some("tag:yaml.org,2002:str");
        self.scalar(text, tag, string_style(text))
    }
}

fn string_style(text: &str) -> unsafe_yaml::yaml_scalar_style_t {
    // YAML 1.1 は NEL 等を改行へ正規化するため、引用だけでなく escape を選ぶ。
    if text.chars().any(|c| {
        matches!(
            c,
            '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}' | '\u{fffe}' | '\u{ffff}'
        )
    }) {
        return unsafe_yaml::YAML_DOUBLE_QUOTED_SCALAR_STYLE;
    }
    if text.strip_suffix('\n').unwrap_or(text).contains('\n') {
        return unsafe_yaml::YAML_LITERAL_SCALAR_STYLE;
    }
    if text == "<<" {
        return unsafe_yaml::YAML_SINGLE_QUOTED_SCALAR_STYLE;
    }
    if matches!(text, "y" | "Y" | "n" | "N")
        || (text
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric() && c != '_')
            && !text.contains('"'))
    {
        return unsafe_yaml::YAML_DOUBLE_QUOTED_SCALAR_STYLE;
    }
    static IMPLICIT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"(?ix)\A(?:null|~|yes|no|true|false|on|off|[-+]?\.(?:inf|nan)|[-+]?(?:[0-9][0-9_,]*|0x[0-9a-f_,]+|0b[01_,]+)|[-+]?(?:[0-9][0-9_,]*)?\.[0-9]*(?:e[-+][0-9]+)?|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9]){1,2}(?:\.[0-9_]*)?|[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}(?:[Tt ].*)?)\z|\A0[0-7]*[89]"
    ).expect("valid Psych scalar pattern")
    });
    if text.is_empty() || IMPLICIT.is_match(text) || text.starts_with(':') {
        unsafe_yaml::YAML_SINGLE_QUOTED_SCALAR_STYLE
    } else {
        unsafe_yaml::YAML_PLAIN_SCALAR_STYLE
    }
}
