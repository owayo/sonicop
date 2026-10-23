//! Onigmo に渡す正規表現の本文から、ローカル変数を宣言する名前付きキャプチャを読む。
//!
//! 呼び出し側が Ruby のリテラルの区切り・エスケープを解釈した本文を渡す。文字クラス・コメント・
//! インラインの `x` オプションは Onigmo と同じに扱う。正規表現全体のコンパイル可否は判定しない。

/// 名前を最初に現れた順で重複なく返す。`extended` はリテラル全体の `x` オプション。
pub(crate) fn named_groups(pattern: &str, extended: bool) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut names = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // 開いているグループごとの `x` の状態。`(?x:…)` はその中だけに、`(?x)` は囲むグループの
    // 残りに効く。
    let mut free_spacing = vec![extended];
    let mut class_depth = 0usize;
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == '\\' {
            index += 2;
            continue;
        }
        if class_depth > 0 {
            match character {
                // `[:alpha:]` は次のエスケープされていない `]` より前に `:]` があるときだけ。
                // 無ければ入れ子の文字クラスを開く。
                '[' if chars.get(index + 1) == Some(&':') => {
                    if let Some(end) = posix_bracket_end(&chars, index + 2) {
                        index = end;
                        continue;
                    }
                    class_depth += 1;
                    index = class_body_start(&chars, index + 1);
                    continue;
                }
                '[' => {
                    class_depth += 1;
                    index = class_body_start(&chars, index + 1);
                    continue;
                }
                ']' => class_depth -= 1,
                _ => {}
            }
            index += 1;
            continue;
        }
        let spacing = *free_spacing.last().expect("最上位は残る");
        match character {
            '[' => {
                class_depth = 1;
                index = class_body_start(&chars, index + 1);
                continue;
            }
            '#' if spacing => {
                index = (index..chars.len())
                    .find(|&position| chars[position] == '\n')
                    .map_or(chars.len(), |end| end + 1);
                continue;
            }
            '(' if chars.get(index + 1) == Some(&'?') => {
                match chars.get(index + 2) {
                    // コメントは最初のエスケープされていない `)` で終わる。
                    Some('#') => {
                        index = comment_end(&chars, index + 3);
                        continue;
                    }
                    Some(&quote @ ('<' | '\''))
                        if !(quote == '<' && matches!(chars.get(index + 3), Some('=' | '!'))) =>
                    {
                        let close = if quote == '<' { '>' } else { '\'' };
                        let start = index + 3;
                        match (start..chars.len()).find(|&position| chars[position] == close) {
                            Some(end) => {
                                let name: String = chars[start..end].iter().collect();
                                if seen.insert(name.clone()) {
                                    names.push(name);
                                }
                                index = end + 1;
                            }
                            None => index = start,
                        }
                        free_spacing.push(spacing);
                        continue;
                    }
                    _ => {
                        let (switched, end) = options(&chars, index + 2, spacing);
                        match chars.get(end) {
                            // `(?x)` は囲むグループの残りに効き、グループを開かない。
                            Some(')') => {
                                *free_spacing.last_mut().expect("最上位は残る") = switched;
                                index = end + 1;
                            }
                            Some(':') => {
                                free_spacing.push(switched);
                                index = end + 1;
                            }
                            // 先読み・後読み・アトミック・不在・条件付きは、開閉だけを数える。
                            _ => {
                                free_spacing.push(spacing);
                                index += 2;
                            }
                        }
                        continue;
                    }
                }
            }
            '(' => free_spacing.push(spacing),
            ')' if free_spacing.len() > 1 => {
                free_spacing.pop();
            }
            _ => {}
        }
        index += 1;
    }
    names
}

/// 文字クラスを開いた `[` の後ろで、中身を読み始める位置。否定の `^` と、その直後の `]` は文字。
fn class_body_start(chars: &[char], mut index: usize) -> usize {
    if chars.get(index) == Some(&'^') {
        index += 1;
    }
    if chars.get(index) == Some(&']') {
        index += 1;
    }
    index
}

/// `[:` の後ろから、POSIX ブラケットを閉じる `:]` の後ろの位置。エスケープを飛ばして探し、先に
/// `]` が来たら POSIX ブラケットではない。
fn posix_bracket_end(chars: &[char], mut index: usize) -> Option<usize> {
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            ':' if chars.get(index + 1) == Some(&']') => return Some(index + 2),
            ']' => return None,
            _ => index += 1,
        }
    }
    None
}

/// `(?#` の後ろから、コメントを閉じる `)` の後ろの位置。
fn comment_end(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            ')' => return index + 1,
            _ => index += 1,
        }
    }
    chars.len()
}

/// `(?imx-imx` のオプションを読み、`x` の新しい状態と、オプションの後ろの位置を返す。
fn options(chars: &[char], start: usize, spacing: bool) -> (bool, usize) {
    let mut switched = spacing;
    let mut off = false;
    let mut index = start;
    while let Some(&character) = chars.get(index) {
        match character {
            '-' => off = true,
            'x' => switched = !off,
            'i' | 'm' | 'a' | 'd' | 'u' => {}
            _ => break,
        }
        index += 1;
    }
    (switched, index)
}
