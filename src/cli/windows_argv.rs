// SPDX-License-Identifier: Artistic-1.0-Perl
// Copyright (c) 1993 Intergraph Corporation.
//
// 2026-10-25: Ruby 4.0.7 win32/win32.c の argv 字句処理を Rust/UTF-16 へ移植。
// 元版: https://github.com/ruby/ruby/blob/229531a6cfbf07e3caef30dbac24a2a3f3fed482/win32/win32.c
// 変更: LF/CR/VT/FF のある OS argv だけに適用し、glob を除外。不正 UTF-16 は拒否する。
// 原本・公開修正版・ライセンス全文の取得先は NOTICE と licenses/RUBY_WIN32_ARTISTIC.txt。

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;

fn needs_ruby_boundaries(raw: &[u16]) -> bool {
    raw.iter().any(|unit| matches!(*unit, 10..=13))
}

fn valid_utf16(raw: &[u16]) -> bool {
    char::decode_utf16(raw.iter().copied()).all(|unit| unit.is_ok())
}

#[cfg(windows)]
pub(super) fn arguments() -> anyhow::Result<Vec<OsString>> {
    let raw = raw_command_line()?;
    if !needs_ruby_boundaries(&raw) {
        return Ok(std::env::args_os().collect());
    }
    // quote除去で不正なsurrogateが偶然結合しても、原本の別名へ読み替えない。
    if !valid_utf16(&raw) {
        let index = std::env::args_os()
            .position(|argument| argument.into_string().is_err())
            .unwrap_or(0);
        anyhow::bail!("command-line argument {index} is not valid Unicode");
    }
    Ok(parse(&raw)
        .iter()
        .map(|word| OsString::from_wide(word))
        .collect())
}

#[cfg(windows)]
fn raw_command_line() -> anyhow::Result<Vec<u16>> {
    use windows_sys::Win32::System::Environment::GetCommandLineW;
    // GetCommandLineW はプロセスが保持する NUL 終端 UTF-16 を返す。変更も解放もしない。
    // CreateProcessW の上限は終端込み32,767単位。無制限走査や切り詰めで別argvにしない。
    // https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw
    let pointer = unsafe { GetCommandLineW() };
    if pointer.is_null() {
        anyhow::bail!("failed to obtain Windows command line");
    }
    let mut raw = Vec::new();
    for offset in 0..32_767 {
        // OSのNUL終端契約に従い、終端を見つけた後の領域は一度も読まない。
        let unit = unsafe { pointer.add(offset).read() };
        if unit == 0 {
            return Ok(raw);
        }
        raw.push(unit);
    }
    anyhow::bail!("Windows command line is not terminated within the OS limit")
}

fn leading_space(unit: u16) -> bool {
    matches!(unit, 9..=13 | 32)
}

fn unquote(token: &[u16]) -> Vec<u16> {
    let mut result = Vec::with_capacity(token.len());
    let mut quote = 0;
    let mut slashes = 0usize;
    let mut cursor = 0;
    while cursor < token.len() {
        let unit = token[cursor];
        match unit {
            92 => {
                result.push(unit);
                if quote != 39 {
                    slashes += 1;
                }
            }
            34 | 39 => {
                if slashes.is_multiple_of(2) && quote != 0 && quote != unit {
                    result.push(unit);
                } else {
                    result.truncate(result.len() - slashes.div_ceil(2));
                    if !slashes.is_multiple_of(2) {
                        result.push(unit);
                    } else if quote == 0 {
                        quote = unit;
                    } else {
                        if quote == 34 && token.get(cursor + 1) == Some(&34) {
                            result.push(34);
                            cursor += 1;
                        }
                        quote = 0;
                    }
                }
                slashes = 0;
            }
            _ => {
                result.push(unit);
                slashes = 0;
            }
        }
        cursor += 1;
    }
    result
}

fn parse(raw: &[u16]) -> Vec<Vec<u16>> {
    let mut words = Vec::new();
    let mut cursor = 0;
    while cursor < raw.len() {
        while raw.get(cursor).is_some_and(|unit| leading_space(*unit)) {
            cursor += 1;
        }
        if raw.get(cursor).is_none_or(|unit| *unit == 0) {
            break;
        }
        let start = cursor;
        let mut quote = 0;
        let mut slashes = 0usize;
        while cursor < raw.len() && raw[cursor] != 0 {
            match raw[cursor] {
                92 => {
                    if quote != 39 {
                        slashes += 1;
                    }
                }
                9 | 10 | 32 => {
                    if quote == 0 {
                        break;
                    }
                    // 4.0.7 は引用内の改行で slashes をここでは reset しない。
                }
                unit @ (34 | 39) => {
                    if slashes.is_multiple_of(2) {
                        if quote == 0 {
                            quote = unit;
                        } else if quote == unit {
                            if quote == 34 && raw.get(cursor + 1) == Some(&34) {
                                cursor += 1;
                            }
                            quote = 0;
                        }
                    }
                    slashes = 0;
                }
                _ => slashes = 0,
            }
            cursor += 1;
        }
        let word = unquote(&raw[start..cursor]);
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::{needs_ruby_boundaries, parse, valid_utf16};

    /// 候補の値を期待値へせず、実4.0.7の開始時ARGVを単語ごと照合する。
    #[test]
    fn windows_words_match_the_actual_ruby_revision() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/windows_argv.json")).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 238);
        for case in cases {
            let raw: Vec<_> = case["tail"].as_str().unwrap().encode_utf16().collect();
            assert!(valid_utf16(&raw));
            let actual: Vec<_> = parse(&raw)
                .iter()
                .map(|word| String::from_utf16(word).unwrap())
                .collect();
            let expected: Vec<_> = case["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|word| word.as_str().unwrap())
                .collect();
            assert_eq!(actual, expected, "{}", case["name"]);
        }
    }

    /// 通常起動と非ASCII空白は既存args_osを通り、全quote規則を便乗変更しない。
    #[test]
    fn only_the_requested_control_boundary_uses_the_ruby_path() {
        for raw in ["sonicop --only Known", "a\tb", "a b", "a\u{2028}b"] {
            assert!(!needs_ruby_boundaries(
                &raw.encode_utf16().collect::<Vec<_>>()
            ));
        }
        for raw in ["a\nb", "a\rb", "a\u{b}b", "a\u{c}b", "a \"\n\""] {
            assert!(needs_ruby_boundaries(
                &raw.encode_utf16().collect::<Vec<_>>()
            ));
        }
    }

    /// #100の拒否契約。引用除去でsurrogateが結合しても原本の不正を隠さない。
    #[test]
    fn malformed_original_utf16_is_not_repaired_by_quote_removal() {
        for raw in [
            vec![0xd800],
            vec![0xdc00],
            vec![10, 0xd800],
            vec![34, 0xd800, 34],
            vec![0xd800, 34, 34, 0xdc00],
        ] {
            assert!(!valid_utf16(&raw));
        }
        assert!(valid_utf16(&"日本語💎é".encode_utf16().collect::<Vec<_>>()));
    }
}
