//! 本家 RuboCop の `expect_offense` キャレット注記の読み書き。
//!
//! 注記行の判定は本家 `RuboCop::RSpec::ExpectOffense::AnnotatedSource::ANNOTATION_PATTERN`
//! (`/\A\s*((?<!\\)\^+|\^{}) ?/`) に合わせてある。バックスラッシュで逃がした
//! `\^^^` はソース行として扱われる。

use sonicop::diagnostic::Offense;
use sonicop::source::SourceFile;

/// 期待メッセージの末尾を省略するための印。前方一致した実際のメッセージへ
/// 差し替えることで、差分に無関係な後半部分が出ないようにする。
pub const ABBREVIATION: &str = "[...]";

/// offense 1 件を注記 1 行として表した値。`line` は注記が指すソース行 (1-based)、
/// `column` は文字単位の開始カラム (1-based)、`length` はキャレットの本数
/// (0 は `^{}` = 空レンジ)。
///
/// 導出順が `line` → `column` → `length` → `message` なので、`sort` すれば
/// 注記の並びが一意に決まる。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Annotation {
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub message: String,
}

impl Annotation {
    pub fn new(line: usize, column: usize, length: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            column,
            length,
            message: message.into(),
        }
    }

    /// 注記 1 行を復元する。本家 `with_offense_annotations` と同じ書式。
    pub fn text(&self) -> String {
        let carets = if self.length == 0 {
            "^{}".to_owned()
        } else {
            "^".repeat(self.length)
        };
        let indent = " ".repeat(self.column.saturating_sub(1));
        format!("{indent}{carets} {}\n", self.message)
    }

    /// レンジだけの表現。差分の突き合わせキーになるので書式を変えないこと。
    pub fn span(&self) -> String {
        format!(
            "{}:{}-{} ({} chars)",
            self.line,
            self.column,
            self.column + self.length,
            self.length
        )
    }

    /// 人が読む一覧用の 1 行表現。
    pub fn summary(&self) -> String {
        format!(
            "{}:{}-{} ({} chars) {}",
            self.line,
            self.column,
            self.column + self.length,
            self.length,
            self.message
        )
    }
}

/// キャレット注記を取り除いたソースと、注記の一覧。
#[derive(Clone, Debug)]
pub struct Annotated {
    pub source: String,
    pub annotations: Vec<Annotation>,
}

/// 注記付きソースを、素のソースと注記一覧に分解する。
pub fn parse(annotated: &str) -> Annotated {
    let mut lines: Vec<&str> = Vec::new();
    let mut annotations: Vec<Annotation> = Vec::new();

    for raw in annotated.split_inclusive('\n') {
        match annotation_of(raw, lines.len()) {
            Some(annotation) => annotations.push(annotation),
            None => lines.push(raw),
        }
    }

    // ソース行が 1 行も無いときは全注記を 1 行目へ寄せる (本家と同じ)。
    if lines.is_empty() {
        for annotation in &mut annotations {
            annotation.line = 1;
        }
    }
    annotations.sort();

    Annotated {
        source: lines.concat(),
        annotations,
    }
}

/// 素のソースへ注記を挿し込んで、注記付きソースを復元する。期待と実際を
/// 同じ関数で描き直すので、並び順の違いは差分に出ない。
pub fn render(source: &str, annotations: &[Annotation]) -> String {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut sorted: Vec<&Annotation> = annotations.iter().collect();
    sorted.sort();

    let mut rendered = String::new();
    let mut pending = sorted.as_slice();
    for target in 0..=lines.len() {
        if let Some(line) = target.checked_sub(1).and_then(|index| lines.get(index)) {
            rendered.push_str(line);
            if !line.ends_with('\n') {
                rendered.push('\n');
            }
        }
        while let Some((annotation, rest)) = pending.split_first() {
            if annotation.line != target {
                break;
            }
            rendered.push_str(&annotation.text());
            pending = rest;
        }
    }
    // 行数を超える注記も落とさず末尾に出す (期待の書き間違いを見えるようにする)。
    for annotation in pending {
        rendered.push_str(&annotation.text());
    }
    rendered
}

/// offense を注記へ変換する。複数行に跨るレンジは、本家 `Offense#column_length`
/// と同じく先頭行の末尾までしかキャレットを引かない。
pub fn from_offense(offense: &Offense, source: &SourceFile) -> Annotation {
    let location = offense.location(source);
    let length = if location.length == 0 {
        0
    } else if location.start_line == location.last_line {
        (location.last_column + 1).saturating_sub(location.start_column)
    } else {
        line_width(source, location.start_line).saturating_sub(location.start_column - 1)
    };
    Annotation {
        line: location.start_line,
        column: location.start_column,
        length,
        message: offense.message.clone(),
    }
}

/// Ruby の `<<~HEREDOC` と同じ整形を行う。先頭の改行 1 個を落としてから、
/// 空行以外に共通する字下げを削る。本家 spec がすべて `<<~RUBY` で書かれている
/// ため、これがあるとケースをほぼそのまま移植できる。
///
/// 字下げを削る量は注記行にも等しく効くので、キャレットのカラムはずれない。
/// 共通字下げを削った結果はもう共通字下げを持たないので、二重適用しても
/// 結果は変わらない。ただし先頭が空行のソースは 1 度しか通してはいけない。
pub fn dedent(text: &str) -> String {
    let body = text
        .strip_prefix("\r\n")
        .or_else(|| text.strip_prefix('\n'))
        .unwrap_or(text);
    let indent = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(leading_indent)
        .min()
        .unwrap_or(0);
    if indent == 0 {
        return body.to_owned();
    }
    body.split_inclusive('\n')
        .map(|line| &line[indent.min(leading_indent(line))..])
        .collect()
}

/// 行頭の空白の**バイト数**。Ruby の `<<~` と同じく ASCII の空白とタブだけを
/// 字下げとして数える。多バイト空白を含めると削る位置が文字境界を割る。
fn leading_indent(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count()
}

/// `%{key}` / `^{key}` / `_{key}` を展開する。本家 `format_offense` と同じ用途で、
/// 同じ形の期待を長さの違う識別子について並べるときに使う。
pub fn expand(annotated: &str, replacements: &[(&str, &str)]) -> String {
    let mut expanded = annotated.to_owned();
    for (key, value) in replacements {
        let width = value.chars().count();
        expanded = expanded
            .replace(&format!("%{{{key}}}"), value)
            .replace(&format!("^{{{key}}}"), &"^".repeat(width))
            .replace(&format!("_{{{key}}}"), &" ".repeat(width));
    }
    expanded
}

/// `[...]` で省略された期待メッセージを、前方一致する実際のメッセージへ差し替える。
/// 本家 `match_annotations?` と同じ扱い。
pub fn resolve_abbreviations(expected: &mut [Annotation], actual: &[Annotation]) {
    for annotation in expected {
        let Some(prefix) = annotation.message.strip_suffix(ABBREVIATION) else {
            continue;
        };
        let matched = actual.iter().find(|candidate| {
            (candidate.line, candidate.column, candidate.length)
                == (annotation.line, annotation.column, annotation.length)
                && candidate.message.starts_with(prefix)
        });
        if let Some(matched) = matched {
            annotation.message = matched.message.clone();
        }
    }
}

fn annotation_of(raw: &str, preceding_lines: usize) -> Option<Annotation> {
    let text = raw.strip_suffix('\n').unwrap_or(raw);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let rest = text.trim_start();
    if !rest.starts_with('^') {
        return None;
    }

    // `^{}` を先に見る。長さ 0 のレンジをキャレット 1 本と区別するため。
    let (length, message) = match rest.strip_prefix("^{}") {
        Some(message) => (0, message),
        None => {
            let carets = rest
                .chars()
                .take_while(|character| *character == '^')
                .count();
            (carets, &rest[carets..])
        }
    };
    let message = message.strip_prefix(' ').unwrap_or(message);

    Some(Annotation {
        line: preceding_lines,
        column: text.chars().count() - rest.chars().count() + 1,
        length,
        message: message.to_owned(),
    })
}

fn line_width(source: &SourceFile, one_based_line: usize) -> usize {
    source
        .line(one_based_line)
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .chars()
        .count()
}
