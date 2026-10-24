//! parser builder の静的 Regexp.new。Ruby VM は起動しない。

use crate::engine::LiteralEncoding;
use sonicop_onigmo_compat::Encoding;
use std::borrow::Cow;
#[path = "regexp_encoding_tables.rs"]
mod tables;

/// String#encode の変換不能エラーも RegexpError と同じ invalid_regexp になる。
pub(crate) fn validate(bytes: &[u8], encoding: LiteralEncoding, flags: &str) -> Result<(), String> {
    let Some(PreparedPattern { source, target }) = prepare(bytes, encoding, flags)? else {
        // parser が rescue しない InvalidByteSequenceError を Syntax 診断へ変換しない。
        return Ok(());
    };
    sonicop_onigmo_compat::check(&source, target, flags.contains('x'))
        .map_err(|message| decode_error(&message, target))
}

/// 未コンパイルの None と、正常成功した捕捉名 0 件を区別する。
pub(crate) fn compile(
    bytes: &[u8],
    encoding: LiteralEncoding,
    flags: &str,
) -> Result<Option<sonicop_onigmo_compat::CompiledRegexp>, String> {
    let Some(PreparedPattern { source, target }) = prepare(bytes, encoding, flags)? else {
        return Ok(None);
    };
    sonicop_onigmo_compat::compile(&source, target, flags.contains('x'))
        .map(Some)
        .map_err(|message| decode_error(&message, target))
}

/// Parser::Source::Buffer の識別子と同じ Symbol になる捕捉名だけを共有する。
/// 正規表現の option による変換後の符号化を、元の本文の符号化と取り違えない。
pub(crate) fn named_groups(
    bytes: &[u8],
    encoding: LiteralEncoding,
    flags: &str,
    identifier_encoding: LiteralEncoding,
) -> Option<Vec<String>> {
    let compiled = compile(bytes, encoding, flags).ok()??;
    referencable_names(
        compiled,
        match identifier_encoding {
            LiteralEncoding::Binary => Encoding::Binary,
            LiteralEncoding::SevenBit | LiteralEncoding::Text => Encoding::Utf8,
        },
    )
}

/// 診断互換の String#encode→Regexp.new と分けた、MRI のリテラル側の入口。
/// parse.y の rb_reg_fragment_setenc は、非 ASCII の本文を option の符号化へ変換しない。
/// また Unicode escape が固定した UTF-8 を、/e・/s・/n で上書きできない。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LiteralNames {
    Valid(Vec<String>),
    Invalid,
    Unavailable,
}

pub(crate) fn literal_names(
    bytes: &[u8],
    encoding: Encoding,
    flags: &str,
    identifier_encoding: Encoding,
) -> LiteralNames {
    let forced = option_encoding(flags);
    let target = if forced.is_none() && identifier_encoding == Encoding::UsAscii {
        // rb_reg_fragment_setenc は p->enc を見る。補間の Unicode が UTF-8 String を作っても、
        // US-ASCII ソースでは非 ASCII 本文を US-ASCII へ戻して拒否する。
        if bytes.is_ascii() {
            Encoding::Binary
        } else {
            Encoding::UsAscii
        }
    } else {
        forced.unwrap_or(encoding)
    };
    if !bytes.is_ascii() && target != encoding {
        return LiteralNames::Invalid;
    }
    let compiled = match sonicop_onigmo_compat::compile(bytes, target, flags.contains('x')) {
        Ok(compiled) => compiled,
        Err(reason) => {
            // 資源不足を、正常にコンパイルして名前が無かった結果へ読み替えない。
            return if reason.starts_with(b"failed to allocate memory") {
                LiteralNames::Unavailable
            } else {
                LiteralNames::Invalid
            };
        }
    };
    if forced.is_some() && compiled.encoding != Encoding::UsAscii && compiled.encoding != target {
        return LiteralNames::Invalid;
    }
    referencable_names(compiled, identifier_encoding)
        .map_or(LiteralNames::Unavailable, LiteralNames::Valid)
}

fn option_encoding(flags: &str) -> Option<Encoding> {
    if flags.contains('u') {
        Some(Encoding::Utf8)
    } else if flags.contains('e') {
        Some(Encoding::EucJp)
    } else if flags.contains('s') {
        Some(Encoding::Windows31J)
    } else if flags.contains('n') {
        Some(Encoding::Binary)
    } else {
        None
    }
}

/// ASCII の Symbol は全符号化で同一。非 ASCII の Symbol は byte と符号化がともに一致する。
fn referencable_names(
    compiled: sonicop_onigmo_compat::CompiledRegexp,
    expected: Encoding,
) -> Option<Vec<String>> {
    compiled
        .names
        .into_iter()
        .filter(|name| name.is_ascii() || compiled.encoding == expected)
        .map(|name| decode_name(&name, expected))
        .collect()
}

/// 識別子として再び参照するための復号なので、不正 byte を置換文字で成功させない。
fn decode_name(bytes: &[u8], encoding: Encoding) -> Option<String> {
    let codec = match encoding {
        Encoding::Binary => return Some(bytes.iter().copied().map(char::from).collect()),
        Encoding::Utf8 | Encoding::UsAscii => return String::from_utf8(bytes.to_vec()).ok(),
        Encoding::EucJp => encoding_rs::EUC_JP,
        Encoding::Windows31J => encoding_rs::SHIFT_JIS,
    };
    // 物理側の名前を SourceFile と同じ表示へ戻す。Ruby の error 表示用の変換とは分ける。
    let text = codec.decode_without_bom_handling_and_without_replacement(bytes)?;
    let (encoded, _, malformed) = codec.encode(&text);
    (!malformed && encoded.as_ref() == bytes).then(|| text.into_owned())
}

/// UTF-8 へ復号して保持するソースの通常文字を、元のリテラルの byte へ戻す。
/// byte escape は呼出側が直接保持し、ここへ通して二重に符号化しない。
pub(crate) fn encode_fragment(text: &str, encoding: Encoding) -> Option<Vec<u8>> {
    match encoding {
        Encoding::Binary => Some(
            text.chars()
                .flat_map(|character| {
                    u8::try_from(u32::from(character))
                        .map_or_else(|_| character.to_string().into_bytes(), |byte| vec![byte])
                })
                .collect(),
        ),
        Encoding::Utf8 => Some(text.as_bytes().to_vec()),
        Encoding::UsAscii => text.is_ascii().then(|| text.as_bytes().to_vec()),
        Encoding::EucJp | Encoding::Windows31J => {
            let codec = if encoding == Encoding::EucJp {
                encoding_rs::EUC_JP
            } else {
                encoding_rs::SHIFT_JIS
            };
            let (bytes, _, malformed) = codec.encode(text);
            if malformed
                || codec
                    .decode_without_bom_handling_and_without_replacement(&bytes)?
                    .as_ref()
                    != text
            {
                return None;
            }
            Some(bytes.into_owned())
        }
    }
}

struct PreparedPattern<'a> {
    source: Cow<'a, [u8]>,
    target: Encoding,
}

fn prepare<'a>(
    bytes: &'a [u8],
    encoding: LiteralEncoding,
    flags: &str,
) -> Result<Option<PreparedPattern<'a>>, String> {
    let original = match encoding {
        LiteralEncoding::Binary => Encoding::Binary,
        LiteralEncoding::SevenBit => Encoding::UsAscii,
        LiteralEncoding::Text => Encoding::Utf8,
    };
    let target = option_encoding(flags).unwrap_or(original);
    Ok(transcode(bytes, original, target)?.map(|source| PreparedPattern { source, target }))
}

/// Ruby の error String の符号化を UTF-8 表示へ戻す。JIS の別名も生成した decode 表を使う。
fn decode_error(bytes: &[u8], encoding: Encoding) -> String {
    let mapping = match encoding {
        Encoding::EucJp => tables::EUC_JP_DECODE,
        Encoding::Windows31J => tables::WINDOWS_31J_DECODE,
        _ => {
            return String::from_utf8(bytes.to_vec()).unwrap_or_else(|error| {
                error
                    .as_bytes()
                    .iter()
                    .map(|byte| {
                        if byte.is_ascii() {
                            char::from(*byte).to_string()
                        } else {
                            format!("\\x{byte:02X}")
                        }
                    })
                    .collect()
            });
        }
    };
    let mut result = String::new();
    let mut position = 0;
    while position < bytes.len() {
        let byte = bytes[position];
        if byte.is_ascii() {
            result.push(char::from(byte));
            position += 1;
            continue;
        }
        let width = match encoding {
            Encoding::EucJp if byte == 0x8f => 3,
            Encoding::EucJp => 2,
            Encoding::Windows31J if matches!(byte, 0x81..=0x9f | 0xe0..=0xfc) => 2,
            _ => 1,
        };
        if let Some(part) = bytes.get(position..position + width) {
            let packed = part
                .iter()
                .fold(0u32, |value, byte| (value << 8) | u32::from(*byte));
            if let Ok(index) = mapping.binary_search_by_key(&packed, |&(packed, _)| packed)
                && let Some(character) = char::from_u32(mapping[index].1)
            {
                result.push(character);
                position += width;
                continue;
            }
        }
        // 不正 byte を U+FFFD に置換せず、値を保つ明示的な表現へ戻す。
        result.push_str(&format!("\\x{byte:02X}"));
        position += 1;
    }
    result
}

fn encoding_name(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Utf8 => "UTF-8",
        Encoding::Binary => "ASCII-8BIT",
        Encoding::UsAscii => "US-ASCII",
        Encoding::EucJp => "EUC-JP",
        Encoding::Windows31J => "Windows-31J",
    }
}

fn transcode(bytes: &[u8], from: Encoding, to: Encoding) -> Result<Option<Cow<'_, [u8]>>, String> {
    if from as i32 == to as i32 {
        return Ok(Some(Cow::Borrowed(bytes)));
    }
    if matches!(from, Encoding::Binary) {
        if let Some(byte) = bytes.iter().find(|byte| !byte.is_ascii()) {
            // Ruby の Binary→日本語符号化は UTF-8 を中継し、例外文にも変換経路を残す。
            let route = if matches!(to, Encoding::Utf8) {
                "from ASCII-8BIT to UTF-8".to_owned()
            } else {
                format!(
                    "to UTF-8 in conversion from ASCII-8BIT to UTF-8 to {}",
                    encoding_name(to)
                )
            };
            return Err(format!("\"\\x{byte:02X}\" {route}"));
        }
        return Ok(Some(Cow::Borrowed(bytes)));
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(None);
    };
    let mapping = match to {
        Encoding::EucJp => tables::EUC_JP,
        Encoding::Windows31J => tables::WINDOWS_31J,
        _ => &[],
    };
    let mut encoded = Vec::with_capacity(bytes.len());
    for character in text.chars() {
        if character.is_ascii() {
            encoded.push(character as u8);
            continue;
        }
        let code = u32::from(character);
        let Ok(index) = mapping.binary_search_by_key(&code, |&(code, _)| code) else {
            return Err(format!("U+{code:04X} from UTF-8 to {}", encoding_name(to)));
        };
        let packed = mapping[index].1;
        if packed > 0xffff {
            encoded.push((packed >> 16) as u8);
        }
        if packed > 0xff {
            encoded.push((packed >> 8) as u8);
        }
        encoded.push(packed as u8);
    }
    Ok(Some(Cow::Owned(encoded)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_literal_names_match_the_recorded_ruby27_symbols() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/capture_binding_encodings.json"
        ))
        .unwrap();
        assert_eq!(fixture["ruby"]["version"], "2.7.8");
        let mut checked = 0;
        let mut mismatches = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            let pattern = case["pattern_hex"].as_str().unwrap();
            let bytes: Vec<_> = pattern
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            // 静的補間の畳み込みは regexp_ast が行う。ここでは C 前処理後の符号化だけを検査する。
            if bytes.windows(2).any(|pair| pair == b"#{") {
                continue;
            }
            let source = case["source"].as_str().unwrap();
            let encoding = crate::engine::regexp_source_encoding(source).unwrap();
            let actual =
                match literal_names(&bytes, encoding, case["flags"].as_str().unwrap(), encoding) {
                    LiteralNames::Valid(names) => Some(names),
                    LiteralNames::Invalid => None,
                    LiteralNames::Unavailable => {
                        panic!("unavailable recorded case: {}", case["id"])
                    }
                };
            let expected = (case["mri27"]["status"] == "ok").then(|| {
                case["encoding_matched_names"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            });
            if actual != expected {
                mismatches.push(format!("{}: {actual:?} / {expected:?}", case["id"]));
            }
            checked += 1;
        }
        assert_eq!(checked, 190);
        assert!(
            mismatches.is_empty(),
            "{} differences:\n{}",
            mismatches.len(),
            mismatches.join("\n")
        );
    }

    #[test]
    fn ascii_binary_and_ruby_japanese_transcoding_keep_their_meaning() {
        assert!(validate(br"\x80", LiteralEncoding::Binary, "").is_ok());
        assert_eq!(
            validate(br"\x80", LiteralEncoding::Text, ""),
            Err(r"invalid multibyte escape: /\x80/".into())
        );
        // é は JIS X 0212。WHATWG の EUC-JP encoder の範囲に限定しない。
        assert!(validate("é".as_bytes(), LiteralEncoding::Text, "e").is_ok());
        assert_eq!(
            validate("é".as_bytes(), LiteralEncoding::Text, "n"),
            Err("U+00E9 from UTF-8 to ASCII-8BIT".into())
        );
        assert_eq!(
            validate("é".as_bytes(), LiteralEncoding::Text, "s"),
            Err("U+00E9 from UTF-8 to Windows-31J".into())
        );
    }
    #[test]
    fn binary_transcoding_error_preserves_rubys_intermediate_utf8_route() {
        // Ruby 4.0.8 の String#encode と実 RuboCop CLI の両方で経路付きの文言を確認した。
        for (flag, target) in [("e", "EUC-JP"), ("s", "Windows-31J")] {
            assert_eq!(
                validate("あ".as_bytes(), LiteralEncoding::Binary, flag),
                Err(format!(
                    "\"\\xE3\" to UTF-8 in conversion from ASCII-8BIT to UTF-8 to {target}"
                ))
            );
        }
        assert_eq!(
            validate("あ".as_bytes(), LiteralEncoding::Binary, "u"),
            Err("\"\\xE3\" from ASCII-8BIT to UTF-8".into())
        );
        assert!(validate("あ".as_bytes(), LiteralEncoding::Binary, "n").is_ok());
    }

    #[test]
    fn ruby_euc_jp_error_names_do_not_become_fullwidth_tildes() {
        assert_eq!(decode_error(&[0xa1, 0xc1], Encoding::EucJp), "〜");
        assert_eq!(decode_error(&[0x81, 0x60], Encoding::Windows31J), "～");
        assert_eq!(
            validate(r"\p{〜}".as_bytes(), LiteralEncoding::Text, "e"),
            Err(r"invalid character property name {〜}: /\p{\x{A1C1}}/".into())
        );
    }
    #[test]
    fn empty_join_encoding_changes_escapes_and_character_properties() {
        assert!(validate(br"\x80", LiteralEncoding::SevenBit, "").is_ok());
        assert_eq!(
            validate(br"\p{Hiragana}", LiteralEncoding::SevenBit, ""),
            Err(r"invalid character property name {Hiragana}: /\p{Hiragana}/".into())
        );
    }
}
