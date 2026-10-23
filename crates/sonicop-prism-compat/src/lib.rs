//! RuboCop の仕様版が使う Prism 1.8.1 の診断。Ruby プロセスは起動しない。

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ops::Range;

#[derive(Debug)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub kind: String,
    pub message: String,
}

type LiteralCallback = unsafe extern "C" fn(*mut c_void, usize, usize, c_int);

type Callback = unsafe extern "C" fn(*mut c_void, usize, usize, *const c_char, *const c_char);

unsafe extern "C" {
    fn sonicop_prism_compat_parse(
        source: *const u8,
        length: usize,
        version: *const c_char,
        version_length: usize,
        binary: bool,
        main_script: bool,
        callback: Callback,
        data: *mut c_void,
    ) -> c_int;
    fn sonicop_prism_compat_literal_tokens(
        source: *const u8,
        length: usize,
        callback: LiteralCallback,
        data: *mut c_void,
    );
    fn sonicop_prism_compat_version() -> *const c_char;
}

unsafe extern "C" fn collect(
    data: *mut c_void,
    start: usize,
    end: usize,
    kind: *const c_char,
    message: *const c_char,
) {
    // C の呼び出し中だけ借りる。診断文字列は parser を解放する前に所有値へ複製する。
    let diagnostics = unsafe { &mut *data.cast::<Vec<Diagnostic>>() };
    diagnostics.push(Diagnostic {
        range: start..end,
        kind: unsafe { CStr::from_ptr(kind) }
            .to_string_lossy()
            .into_owned(),
        message: unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned(),
    });
}

pub fn parse(source: &[u8], version: &str, binary: bool) -> Result<Vec<Diagnostic>, &'static str> {
    parse_with_options(source, version, binary, false)
}

/// 書き戻し前の検査は部分式ではなく、Ruby が起動するスクリプトとして解析する。
pub fn parse_script(source: &[u8], version: &str) -> Result<Vec<Diagnostic>, &'static str> {
    parse_with_options(source, version, false, true)
}

fn parse_with_options(
    source: &[u8],
    version: &str,
    binary: bool,
    main_script: bool,
) -> Result<Vec<Diagnostic>, &'static str> {
    let mut diagnostics = Vec::<Diagnostic>::new();
    // pm_options_version_set の latest 分岐は終端の NUL まで比較する。
    let version = CString::new(version).map_err(|_| "invalid Prism target Ruby version")?;
    // C 側は同期的で、source・version・diagnostics の参照を戻り後に保持しない。
    let result = unsafe {
        sonicop_prism_compat_parse(
            source.as_ptr(),
            source.len(),
            version.as_ptr(),
            version.as_bytes().len(),
            binary,
            main_script,
            collect,
            (&mut diagnostics as *mut Vec<Diagnostic>).cast(),
        )
    };
    if result == 0 {
        Ok(diagnostics)
    } else {
        Err("unsupported Prism target Ruby version")
    }
}

pub fn version() -> &'static str {
    // pm_version は、ライブラリに埋め込まれた静的な NUL 終端文字列を返す。
    unsafe { CStr::from_ptr(sonicop_prism_compat_version()) }
        .to_str()
        .expect("Prism version must be ASCII")
}

/// 木の回復で葉の外に落ちる、character と percent literal 区切り内の制御文字。
#[derive(Debug)]
pub struct ControlLiteral {
    /// 読み直しで同じ演算子へ変わったとき、終了側も一緒に保護を撤回する。
    pub start: usize,
    /// 本文内の回復した宣言を、新しい束縛の根拠へ使わないための来歴。
    pub end: usize,
    pub positions: Vec<usize>,
}

unsafe extern "C" fn collect_literal_token(
    data: *mut c_void,
    start: usize,
    end: usize,
    kind: c_int,
) {
    // callback は同期的で、Rust の所有値だけを C の parser より長く残す。
    let tokens = unsafe { &mut *data.cast::<Vec<(Range<usize>, c_int)>>() };
    tokens.push((start..end, kind));
}

/// 制御文字を含む literal の開始と終了を対応付ける。本文のコード補間まで保護しない。
pub fn control_literals(source: &[u8]) -> Vec<ControlLiteral> {
    let mut tokens = Vec::<(Range<usize>, c_int)>::new();
    unsafe {
        sonicop_prism_compat_literal_tokens(
            source.as_ptr(),
            source.len(),
            collect_literal_token,
            (&mut tokens as *mut Vec<(Range<usize>, c_int)>).cast(),
        );
    }
    let positions = |range: Range<usize>| {
        range
            .filter(|&position| matches!(source[position], b'\x0b' | b'\x0c'))
            .collect::<Vec<_>>()
    };
    let mut literals = Vec::new();
    let mut stack = Vec::new();
    // heredoc の token は本文の offset 順ではないので、対応を決める前に並べ替えない。
    for (range, kind) in tokens {
        match kind {
            0 => stack.push(ControlLiteral {
                start: range.start,
                end: source.len(),
                positions: positions(range),
            }),
            1 => {
                if let Some(mut literal) = stack.pop() {
                    literal.end = range.end;
                    literal.positions.extend(positions(range));
                    if !literal.positions.is_empty() {
                        literals.push(literal);
                    }
                }
            }
            2 => {
                let literal = ControlLiteral {
                    start: range.start,
                    end: range.end,
                    positions: positions(range),
                };
                if !literal.positions.is_empty() {
                    literals.push(literal);
                }
            }
            _ => unreachable!("C 側が通知する literal token は 3 種類"),
        }
    }
    // 不完全な literal の開始も文字のまま残し、診断のために空白へ書き換えない。
    literals.extend(
        stack
            .into_iter()
            .filter(|literal| !literal.positions.is_empty()),
    );
    literals
}

#[cfg(test)]
mod tests {
    use super::control_literals;

    fn protected(source: &str) -> Vec<(usize, Vec<usize>)> {
        control_literals(source.as_bytes())
            .into_iter()
            .map(|literal| (literal.start, literal.positions))
            .collect()
    }

    /// character の escape と percent 区切りを、grammar の ERROR 回復より前の範囲で守る。
    #[test]
    fn character_and_percent_control_delimiters_are_paired() {
        for control in ['\x0b', '\x0c'] {
            let source = format!("puts ?\\{control}\n");
            assert_eq!(protected(&source), vec![(5, vec![7])], "{source:?}");
            assert_eq!(control_literals(source.as_bytes())[0].end, source.len() - 1);
            for prefix in ["%", "%q", "%Q", "%r", "%s", "%x"] {
                let source = format!("{prefix}{control}a b{control}\n");
                assert_eq!(
                    protected(&source),
                    vec![(0, vec![prefix.len(), prefix.len() + 4])],
                    "{source:?}",
                );
                assert_eq!(control_literals(source.as_bytes())[0].end, source.len() - 1);
            }
        }
    }

    /// 裸symbol、引用したlabel、backtickのメソッド名は外側のliteralとの対応を壊さない。
    #[test]
    fn interpolation_tokens_do_not_steal_the_outer_delimiter() {
        for control in ['\x0b', '\x0c'] {
            for middle in [
                "#{ { \"a\": :b }; obj.`(1) }",
                "#{ def `(x); x; end; :bare }",
                "#{ `literal`; :bare }",
                "#{ puts <<A }tail",
            ] {
                let source = if middle.contains("<<A") {
                    format!("%Q{control}{middle}{control}\nbody\nA\n")
                } else {
                    format!("%Q{control}{middle}{control}\n")
                };
                assert_eq!(
                    protected(&source),
                    vec![(0, vec![2, 3 + middle.len()])],
                    "{source:?}",
                );
            }
        }
    }

    /// %w の空白区切りはliteral境界ではないので、通常のトークン外空白の判定へ残す。
    #[test]
    fn percent_word_separators_are_not_protected() {
        assert!(protected("%w[a\x0bb]\n").is_empty());
        assert!(protected("%w[a\x0cb]\n").is_empty());
    }
}
