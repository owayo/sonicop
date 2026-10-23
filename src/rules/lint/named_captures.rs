//! 正規表現リテラルを `=~` の左辺に置いたとき、名前付きキャプチャが作るローカル変数の名前。
//!
//! Ruby はリテラルの本文を字句解析で整えてから Onigmo に渡し、Onigmo がコンパイルしたグループの
//! 名前 (`Regexp#names`) で変数を宣言する。ここでは名前を取り出すことだけを、Onigmo の字句の規則に
//! 合わせて行う。文字クラス (`[(?<x>)]`)・コメント (`(?#…)` と `x` オプションの `#`)・
//! エスケープの中の `(?<…>` は名前にならない。条件付きの部分式 (`(?(1)…)`) のように意味を問わない
//! 構文は、グループの開閉だけを数えれば足りる。
//!
//! 変数解析と構文解析の束縛判定の両方がここを使い、同じ名前を宣言する。

/// `/…/flags` か `%r{…}flags` の名前付きキャプチャの名前を、最初に現れた順に重複なく。
/// リテラルでなければ空。
pub(crate) fn literal_named_groups(literal: &str) -> Vec<String> {
    let Some((pattern, extended)) = literal_pattern(literal) else {
        return Vec::new();
    };
    crate::regexp_names::named_groups(&pattern, extended)
}

/// Ruby が Onigmo に渡す本文と、`x` (空白と `#` のコメントを読み飛ばす) が付いているか。
///
/// 字句解析は `\` と改行 (行継続) を取り除き、閉じの区切り文字のエスケープを、その文字が正規表現の
/// メタ文字でなければ区切り文字そのものに戻す (`%r'(?\'name\'a)'` の名前は `name`)。制御文字と
/// メタ文字のエスケープ (`\cX`・`\C-X`・`\M-X`) は、後ろの文字ごと 1 文字の `\xHH` に置き換える
/// (`[\c](?<x>)]` の `]` は文字クラスを閉じない)。ほかのエスケープはそのまま渡す。
fn literal_pattern(literal: &str) -> Option<(String, bool)> {
    let flags_start = literal
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .len();
    let extended = literal[flags_start..].contains('x');
    let (open, close) = if literal.starts_with('/') {
        (1, '/')
    } else {
        let delimiter = literal.strip_prefix("%r")?.chars().next()?;
        let close = match delimiter {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            '<' => '>',
            other => other,
        };
        (2 + delimiter.len_utf8(), close)
    };
    let end = flags_start.checked_sub(close.len_utf8())?;
    if end < open || !literal[end..].starts_with(close) {
        return None;
    }
    let body: Vec<char> = literal[open..end].chars().collect();
    let mut pattern = String::with_capacity(end - open);
    let mut index = 0;
    while index < body.len() {
        if body[index] != '\\' {
            pattern.push(body[index]);
            index += 1;
            continue;
        }
        match body.get(index + 1) {
            Some('\n') => index += 2,
            Some('\r') if body.get(index + 2) == Some(&'\n') => index += 3,
            Some(&escaped) if escaped == close && !simple_re_meta(close) => {
                pattern.push(close);
                index += 2;
            }
            // 値は名前の取り出しに関わらないので、どの文字でも同じ置き換えにする。
            Some('c' | 'C' | 'M') => {
                pattern.push_str("\\x00");
                index = control_escape_end(&body, index + 1);
            }
            Some(&escaped) => {
                pattern.push('\\');
                pattern.push(escaped);
                index += 2;
            }
            None => {
                pattern.push('\\');
                index += 1;
            }
        }
    }
    Some((pattern, extended))
}

/// Ruby の `read_escape` が、`\` の後ろの `c`・`C-`・`M-` (位置 `at`) から読み終える位置。
/// 続きが `\` なら、そのエスケープも 1 文字として読む (`\c\M-a`)。修飾が何重に続いても再帰せず、
/// 位置を進めるだけにする。
fn control_escape_end(body: &[char], mut at: usize) -> usize {
    loop {
        let target = match (body.get(at), body.get(at + 1)) {
            (Some('c'), _) => at + 1,
            (Some('C' | 'M'), Some('-')) => at + 2,
            // 不正なエスケープは Ruby が構文エラーにする。英字だけを読んで先へ進む。
            _ => return at + 1,
        };
        match body.get(target) {
            Some('\\') => match body.get(target + 1) {
                Some('c' | 'C' | 'M') => at = target + 1,
                Some(_) => return target + 2,
                None => return target + 1,
            },
            Some(_) => return target + 1,
            None => return target,
        }
    }
}

/// Ruby の字句解析の `simple_re_meta`: エスケープを残す区切り文字。
fn simple_re_meta(character: char) -> bool {
    matches!(
        character,
        '$' | '*' | '+' | '.' | '?' | '^' | '|' | ')' | ']' | '}' | '>'
    )
}

#[cfg(test)]
mod tests {
    use super::literal_named_groups;

    #[test]
    fn named_groups_are_read_off_the_literal() {
        for (literal, expected) in [
            ("/(?<year>\\d+)-(?<month>\\d+)/", vec!["year", "month"]),
            ("/(?'year'\\d+)/", vec!["year"]),
            ("%r{(?<year>\\d+)}x", vec!["year"]),
            ("/(?<outer>(?<inner>a))/", vec!["outer", "inner"]),
            ("/(?<a>x)|(?<a>y)/", vec!["a"]),
            ("/\\\\(?<x>a)/", vec!["x"]),
            ("/(?<=a)(?<!b)(?<real>c)/", vec!["real"]),
            ("/(?~abc)(?<x>d)/", vec!["x"]),
        ] {
            assert_eq!(literal_named_groups(literal), expected, "{literal:?}");
        }
    }

    /// 文字クラス・コメント・エスケープの中の `(?<…>` は名前を作らない。期待値は Ruby 4.0.8 の
    /// `Regexp#names` で確かめた。
    #[test]
    fn lookalikes_are_not_named_groups() {
        for (literal, expected) in [
            ("/[(?<ghost>)]/", vec![]),
            ("/[[:alpha:](?<ghost>)]/", vec![]),
            ("/a(?#(?<ghost>)/", vec![]),
            ("/(?#a\\)b(?<ghost>c)(?<real>d)/", vec!["real"]),
            ("/a # (?<ghost>)\n/x", vec![]),
            ("/(?x) # (?<ghost>)\n(?<real>a)/", vec!["real"]),
            ("/(?x:#(?<ghost>)\n)#(?<real>a)/", vec!["real"]),
            ("/(?x-x) # (?<real>a)/", vec!["real"]),
            ("/(?x)[#](?<real>a)/", vec!["real"]),
            ("/\\(?<ghost>/", vec![]),
            ("/(a)(?(1)b|c)[(?<ghost>)]/", vec![]),
            ("/(?<real>a)(?(<real>)b|c)[(?<ghost>)]/", vec!["real"]),
        ] {
            assert_eq!(literal_named_groups(literal), expected, "{literal:?}");
        }
    }

    /// 文字クラスの境目は Onigmo と同じに読む。`[` か `[^` の直後の `]` は文字、`[:` は `:]` が
    /// 次の `]` より前にあるときだけ POSIX ブラケット、入れ子と積集合も数える。
    #[test]
    fn character_classes_close_where_onigmo_closes_them() {
        for (literal, expected) in [
            ("/[]a](?<x>b)/", vec!["x"]),
            ("/[^]a](?<x>b)/", vec!["x"]),
            ("/[[:a]](?<x>b:])/", vec!["x"]),
            ("/[a[b]](?<x>c)/", vec!["x"]),
            ("/[a&&[^b]](?<x>c)/", vec!["x"]),
            ("/[\\]](?<x>a)/", vec!["x"]),
        ] {
            assert_eq!(literal_named_groups(literal), expected, "{literal:?}");
        }
    }

    /// Ruby の字句解析が整えた本文で読む。行継続は消え、閉じの区切り文字のエスケープはメタ文字で
    /// なければ区切り文字に戻る。
    #[test]
    fn the_literal_is_read_as_ruby_hands_it_to_onigmo() {
        for (literal, expected) in [
            ("%r'(?\\'name\\'a)'", vec!["name"]),
            ("/(?<na\\\nme>a)/", vec!["name"]),
            ("/(?<na\\\r\nme>a)/", vec!["name"]),
            ("%r!(?<bang>a)\\!!", vec!["bang"]),
            ("%r{(?<brace>a\\})}", vec!["brace"]),
            ("\"(?<string>a)\"", vec![]),
        ] {
            assert_eq!(literal_named_groups(literal), expected, "{literal:?}");
        }
    }

    /// 制御文字とメタ文字のエスケープは後ろの文字ごと 1 文字。その文字が `]` や `[` でも、文字
    /// クラスの境目にはならない。期待値は Ruby 4.0.8 の `=~` による変数の宣言で確かめた。
    #[test]
    fn control_and_meta_escapes_take_the_next_character() {
        for (literal, expected) in [
            ("/[\\c](?<ghost>)]/", vec![]),
            ("/\\c[(?<real>a)/", vec!["real"]),
            ("/\\C-[(?<real>a)/", vec!["real"]),
            ("/\\M-\\C-[(?<real>a)/n", vec!["real"]),
            ("/[\\M-](?<ghost>)]/n", vec![]),
            ("/\\c\\M-[(?<real>a)/n", vec!["real"]),
            ("/[\\c?](?<real>a)/", vec!["real"]),
        ] {
            assert_eq!(literal_named_groups(literal), expected, "{literal:?}");
        }
    }

    /// 修飾が長く続く形や途中で切れたエスケープ (Ruby は構文エラーにする) でも、最後まで読んで
    /// 終わる。
    #[test]
    fn long_or_truncated_escapes_finish() {
        let long = format!("/{}A(?<x>a)/", "\\c".repeat(200_000));
        assert_eq!(literal_named_groups(&long), ["x"]);
        let meta = format!("/{}A(?<y>a)/n", "\\M-\\C-".repeat(100_000));
        assert_eq!(literal_named_groups(&meta), ["y"]);
        for literal in ["/\\c/", "/\\C-/", "/\\M-/", "/\\M/", "/a\\/"] {
            assert_eq!(
                literal_named_groups(literal),
                Vec::<String>::new(),
                "{literal:?}"
            );
        }
    }
}
