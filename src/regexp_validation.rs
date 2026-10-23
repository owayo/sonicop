//! parser builder の静的 Regexp.new。Ruby VM は起動しない。

use crate::engine::LiteralEncoding;
use sonicop_onigmo_compat::Encoding;
use std::borrow::Cow;
#[path = "regexp_encoding_tables.rs"]
mod tables;

/// String#encode の変換不能エラーも RegexpError と同じ invalid_regexp になる。
pub(crate) fn validate(bytes: &[u8], encoding: LiteralEncoding, flags: &str) -> Result<(), String> {
    let original = match encoding {
        LiteralEncoding::Binary => Encoding::Binary,
        LiteralEncoding::SevenBit => Encoding::UsAscii,
        LiteralEncoding::Text => Encoding::Utf8,
    };
    let target = if flags.contains('u') {
        Encoding::Utf8
    } else if flags.contains('e') {
        Encoding::EucJp
    } else if flags.contains('s') {
        Encoding::Windows31J
    } else if flags.contains('n') {
        Encoding::Binary
    } else {
        original
    };
    let Some(source) = transcode(bytes, original, target)? else {
        // parser が rescue しない InvalidByteSequenceError を Syntax 診断へ変換しない。
        return Ok(());
    };
    sonicop_onigmo_compat::check(&source, target, flags.contains('x'))
        .map_err(|message| decode_error(&message, target))
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
