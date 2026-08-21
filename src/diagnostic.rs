use std::fmt;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::source::SourceFile;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Refactor,
    Convention,
    Warning,
    Error,
    Fatal,
}

impl Severity {
    /// The canonical RuboCop name (`Severity#to_s`). Every textual rendering derives from this so
    /// that renaming a variant cannot silently change user-visible output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Refactor => "refactor",
            Self::Convention => "convention",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Fatal => "fatal",
        }
    }

    pub fn code(self) -> char {
        match self {
            Self::Info => 'I',
            Self::Refactor => 'R',
            Self::Convention => 'C',
            Self::Warning => 'W',
            Self::Error => 'E',
            Self::Fatal => 'F',
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "info" | "i" => Some(Self::Info),
            "refactor" | "r" => Some(Self::Refactor),
            "convention" | "c" => Some(Self::Convention),
            "warning" | "w" => Some(Self::Warning),
            "error" | "e" => Some(Self::Error),
            "fatal" | "f" => Some(Self::Fatal),
            _ => None,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
    pub safe: bool,
}

/// Position and source line captured from the text an offense was found in. An autocorrect pass
/// rewrites that text, so a corrected offense carried into the final report can no longer resolve
/// its byte offsets against the report it travels with.
#[derive(Clone, Debug)]
pub struct OffenseSnapshot {
    pub location: Location,
    pub source_line: String,
}

#[derive(Clone, Debug)]
pub struct Offense {
    pub cop_name: &'static str,
    pub severity: Severity,
    pub message: String,
    pub start: usize,
    pub end: usize,
    pub corrected: bool,
    pub correctable: bool,
    pub suppressed: bool,
    pub justification: Option<String>,
    /// Every rewrite one offense asks for. RuboCop hands a cop a corrector it can call any number of
    /// times, and several cops do -- renaming a variable touches its declaration and each of its
    /// uses. Collapsing those into one span that swallows the text between them reproduces the same
    /// output only while nothing else wants to correct inside it, which stops being true as the
    /// registry fills out.
    pub corrections: Vec<Edit>,
    /// The range this offense's insertions hang off, when it is not the reported range.
    ///
    /// RuboCop's `insert_before` / `insert_after` are `wrap` on a range the cop chooses, and that
    /// range -- not the offset the text lands at -- is what decides where the action sits in the
    /// correction tree. A cop usually passes the range it reported, which is what
    /// [`Offense::start`] and [`Offense::end`] already say; this records the cases where it does
    /// not. `Style/FrozenStringLiteralComment` is one: it reports the first character but calls
    /// `insert_before(processed_source.buffer.source_range, ...)`, so its insertion is the parent
    /// of everything else corrected in the file rather than a child of whatever covers the head.
    pub correction_anchor: Option<(usize, usize)>,
    /// Set for edits the cop scheduled outside `add_offense`, which is where `Cop::Base#correct`
    /// puts a rewrite belonging to no single offense. The offense keeps the `:unsupported` status
    /// it was reported with -- neither `correctable` nor ever stamped corrected -- while the edits
    /// still reach the run's corrector. See [`Offense::corrected_without_status`].
    pub corrections_detached: bool,
    pub snapshot: Option<OffenseSnapshot>,
}

impl Offense {
    /// Cops do not call this: `RuleContext::offense` supplies the name and severity from the
    /// registry so that a cop never names itself. It stays reachable inside the crate for the
    /// engine's own offenses, which belong to no cop -- a file that is not valid UTF-8 never
    /// reaches one.
    pub(crate) fn new(
        cop_name: &'static str,
        severity: Severity,
        message: impl Into<String>,
        start: usize,
        end: usize,
    ) -> Self {
        Self {
            cop_name,
            severity,
            message: message.into(),
            start,
            end: end.max(start),
            corrected: false,
            correctable: false,
            suppressed: false,
            justification: None,
            corrections: Vec::new(),
            correction_anchor: None,
            corrections_detached: false,
            snapshot: None,
        }
    }

    pub fn corrected_by(self, edit: Edit) -> Self {
        self.corrected_by_all([edit])
    }

    /// Declare the range the cop handed its corrector, for the cops whose `insert_before` /
    /// `insert_after` range is not the range they reported. See [`Offense::correction_anchor`].
    pub fn corrections_anchored_at(mut self, range: Range<usize>) -> Self {
        self.correction_anchor = Some((range.start, range.end.max(range.start)));
        self
    }

    /// For a cop whose single offense rewrites more than one place at once.
    pub fn corrected_by_all(mut self, edits: impl IntoIterator<Item = Edit>) -> Self {
        self.corrections.extend(edits);
        self.correctable = !self.corrections.is_empty();
        self
    }

    /// For a cop that fills the run's corrector somewhere other than the block `add_offense` takes.
    ///
    /// `Cop::Base#correct` reaches the same corrector from anywhere, and a cop whose rewrite spans
    /// several offenses calls it once from a handler of its own -- `Style/BisectedAttrAccessor`
    /// rewrites a whole `attr_reader` in `after_class` while each bisected attribute was reported
    /// on its own in `on_class`. The offenses were reported with no corrector, so they keep the
    /// `:unsupported` status: they are reported `correctable: false` and an autocorrect run that
    /// applies the edits still does not count them corrected.
    pub fn corrected_without_status(mut self, edits: impl IntoIterator<Item = Edit>) -> Self {
        self.corrections.extend(edits);
        self.corrections_detached = true;
        self
    }

    /// RuboCop derives `correctable?` from the offense status, so an offense a directive comment
    /// suppressed is never correctable however the cop flagged it. Every count, filter and exit
    /// code decision goes through here rather than reading the raw flag.
    pub fn is_correctable(&self) -> bool {
        self.correctable && !self.suppressed
    }

    /// Freeze the position and source line so the offense keeps reporting against the text it was
    /// found in once autocorrect replaces that text.
    pub fn freeze_location(&mut self, source: &SourceFile) {
        if self.snapshot.is_some() {
            return;
        }
        let location = self.location(source);
        self.snapshot = Some(OffenseSnapshot {
            location,
            source_line: source.line(location.line).to_owned(),
        });
    }

    /// Where the offense starts. Ordering and identity only ever need this, and resolving it alone
    /// avoids touching the end of the range, which callers may not have placed on a char boundary.
    pub fn start_position(&self, source: &SourceFile) -> (usize, usize) {
        match &self.snapshot {
            Some(snapshot) => (snapshot.location.line, snapshot.location.column),
            None => source.line_column(self.start),
        }
    }

    /// The source line the offense points at, taken from the frozen snapshot when the report's own
    /// text has since been rewritten.
    pub fn source_line<'a>(&'a self, source: &'a SourceFile) -> &'a str {
        match &self.snapshot {
            Some(snapshot) => &snapshot.source_line,
            None => source.line(self.location(source).line),
        }
    }

    pub fn location(&self, source: &SourceFile) -> Location {
        if let Some(snapshot) = &self.snapshot {
            return snapshot.location;
        }
        let (start_line, start_column) = source.line_column(self.start);
        // RuboCop resolves the end of a range at the exclusive end offset rather than at the last
        // character it covers, so a range closing on a newline is reported on the following line and
        // an empty range yields a `last_column` one before its start. Its JSON formatter emits that
        // column zero-based — the one place the two ends of a location disagree on their base — and
        // maps a resulting 0 back to 1.
        let (last_line, end_column) = source.line_column(self.end);
        let last_column = (end_column - 1).max(1);
        Location {
            start_line,
            start_column,
            last_line,
            last_column,
            length: character_length(source, self.start, self.end),
            line: start_line,
            column: start_column,
        }
    }
}

/// The span's length in characters, which is the unit RuboCop reports.
///
/// Its parser addresses source by character, so a range over `なまえ` is 3 there and 9 here if the
/// byte length is handed out instead. Offsets that a cop derived by arithmetic can land inside a
/// character, so both ends are pulled back to a boundary rather than slicing and panicking.
fn character_length(source: &SourceFile, start: usize, end: usize) -> usize {
    let text = source.text();
    let mut start = start.min(text.len());
    let mut end = end.clamp(start, text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[start..end].chars().count()
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Location {
    pub start_line: usize,
    pub start_column: usize,
    pub last_line: usize,
    pub last_column: usize,
    pub length: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub struct FileReport {
    pub path: std::path::PathBuf,
    pub source: SourceFile,
    pub offenses: Vec<Offense>,
}

#[cfg(test)]
mod tests {
    use super::{Edit, Offense, Severity, character_length};
    use crate::source::SourceFile;

    fn source(text: &str) -> SourceFile {
        SourceFile::new("example.rb", text.to_owned())
    }

    fn offense(start: usize, end: usize) -> Offense {
        Offense::new("Test/Cop", Severity::Convention, "message", start, end)
    }

    fn edit(start: usize, end: usize) -> Edit {
        Edit {
            start,
            end,
            replacement: String::new(),
            safe: true,
        }
    }

    #[test]
    fn every_severity_round_trips_through_its_rubocop_name() {
        for severity in [
            Severity::Info,
            Severity::Refactor,
            Severity::Convention,
            Severity::Warning,
            Severity::Error,
            Severity::Fatal,
        ] {
            assert_eq!(Severity::parse(severity.as_str()), Some(severity));
            // 本家は頭文字 1 字でも受ける。大文字小文字は問わない。
            let initial = severity.code().to_string();
            assert_eq!(Severity::parse(&initial), Some(severity));
            assert_eq!(
                Severity::parse(&severity.as_str().to_uppercase()),
                Some(severity)
            );
        }
        assert_eq!(Severity::parse(""), None);
        assert_eq!(Severity::parse("critical"), None);
    }

    #[test]
    fn severities_order_from_least_to_most_serious() {
        assert!(Severity::Info < Severity::Convention);
        assert!(Severity::Convention < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Fatal);
    }

    /// 本家は範囲の終端を「最後の文字」ではなく排他的な終端オフセットで解決するので、空の範囲は
    /// `last_column` が開始より 1 手前になる。0 になったときだけ 1 に丸める。
    #[test]
    fn an_empty_range_reports_its_last_column_one_before_its_start() {
        let source = source("x = 1\n");
        let location = offense(0, 0).location(&source);
        assert_eq!((location.start_line, location.start_column), (1, 1));
        assert_eq!(location.last_column, 1);
        assert_eq!(location.length, 0);

        let location = offense(4, 4).location(&source);
        assert_eq!(location.start_column, 5);
        assert_eq!(location.last_column, 4);
    }

    /// 改行で閉じる範囲は次の行に載る。本家の `last_line` の取り方と同じ。
    #[test]
    fn a_range_closing_on_a_newline_is_reported_on_the_following_line() {
        let source = source("x = 1\ny = 2\n");
        let location = offense(0, 6).location(&source);
        assert_eq!(location.start_line, 1);
        assert_eq!(location.last_line, 2);
    }

    /// 長さは本家に合わせて文字数で数える。バイト数を渡すと全角で 3 倍になる。
    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        let source = source("x = \"なまえ\"\n");
        let start = "x = \"".len();
        let end = start + "なまえ".len();
        assert_eq!(character_length(&source, start, end), 3);
        assert_eq!(offense(start, end).location(&source).length, 3);
    }

    /// cop が算術で導いたオフセットは文字の途中に落ち得る。ここで panic すると実行全体が死ぬので、
    /// 両端とも文字境界まで引き戻す。
    #[test]
    fn an_offset_inside_a_character_is_pulled_back_instead_of_panicking() {
        let source = source("なまえ\n");
        for start in 0..source.text().len() {
            for end in start..=source.text().len() {
                // 落ちないことがこのテストの主張。
                let _ = character_length(&source, start, end);
            }
        }
        // 「な」の途中から「ま」の途中まで → 両端が丸まって「な」1 文字分。
        assert_eq!(character_length(&source, 1, 4), 1);
    }

    /// 範囲の終端を超えるオフセットでも切り詰めるだけで、落ちも巻き戻しもしない。
    #[test]
    fn offsets_past_the_end_are_clamped() {
        let source = source("abc");
        assert_eq!(character_length(&source, 0, 999), 3);
        assert_eq!(character_length(&source, 999, 999), 0);
        // `Offense::new` が終端を開始まで押し上げるので、逆転した範囲は空になる。
        assert_eq!(offense(2, 1).end, 2);
    }

    /// 本家は `correctable?` を offense の status から導くので、ディレクティブで抑止されたものは
    /// cop がどう報告していても訂正可能ではない。
    #[test]
    fn a_suppressed_offense_is_never_correctable() {
        let mut with_edit = offense(0, 1).corrected_by(edit(0, 1));
        assert!(with_edit.is_correctable());
        with_edit.suppressed = true;
        assert!(!with_edit.is_correctable());

        assert!(!offense(0, 1).is_correctable());
    }

    /// `Cop::Base#correct` 相当。offense に紐づかない書き換えは corrector には届くが、
    /// offense 自体は `:unsupported` のままで訂正可能にはならない。
    #[test]
    fn detached_corrections_reach_the_corrector_without_making_the_offense_correctable() {
        let detached = offense(0, 1).corrected_without_status([edit(0, 1)]);
        assert_eq!(detached.corrections.len(), 1);
        assert!(detached.corrections_detached);
        assert!(!detached.is_correctable());
    }

    /// 訂正で本文が置き換わっても、凍結した位置と行がそのまま報告に残らなければならない。
    #[test]
    fn a_frozen_location_survives_a_rewrite_of_the_text() {
        let before = source("x = 1  \ny = 2\n");
        let mut offense = offense(5, 7);
        offense.freeze_location(&before);
        let frozen = offense.location(&before);

        let after = source("y = 2\n");
        assert_eq!(offense.location(&after).start_line, frozen.start_line);
        assert_eq!(offense.source_line(&after), "x = 1  \n");
        assert_eq!(offense.start_position(&after), (frozen.line, frozen.column));

        // 二度目の凍結は最初のものを上書きしない。
        offense.freeze_location(&after);
        assert_eq!(offense.source_line(&after), "x = 1  \n");
    }
}
