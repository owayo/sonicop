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

/// 本家の `insert_before(range, text)` / `insert_after(range, text)` に渡す範囲と向き。
///
/// 挿入が補正の木のどこに入るかは、文字が落ちる位置ではなく cop が渡した範囲で決まる。向きも
/// 範囲からは決まらない。`Util#args_end` は空の範囲を返し、そこへの `insert_after` は、同じ
/// 空の範囲への別の挿入と並ぶ順序が `insert_before` と逆になる。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InsertionAnchor {
    Before(Range<usize>),
    After(Range<usize>),
}

impl InsertionAnchor {
    /// この範囲への挿入。文字は `Before` なら範囲の先頭、`After` なら末尾に落ちるので、
    /// 位置とアンカーを別々に書いて食い違わせることがない。
    pub fn insert(self, text: &str) -> (Edit, Option<InsertionAnchor>) {
        let at = match &self {
            Self::Before(range) => range.start,
            Self::After(range) => range.end,
        };
        let edit = Edit {
            start: at,
            end: at,
            replacement: text.to_owned(),
            safe: true,
        };
        (edit, Some(self))
    }

    /// 挿入が掛かる範囲。
    pub fn range(&self) -> &Range<usize> {
        match self {
            Self::Before(range) | Self::After(range) => range,
        }
    }
}

/// Position and source line captured from the text an offense was found in. An autocorrect pass
/// rewrites that text, so a corrected offense carried into the final report can no longer resolve
/// its byte offsets against the report it travels with.
#[derive(Clone, Debug)]
pub struct OffenseSnapshot {
    pub location: Location,
    pub source_line: String,
}

/// リテラルの評価と、非 ASCII を同じ順序・個数で複写する補正を区別する。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CorrectionCharacterSource {
    Candidates(usize, usize),
    Copied(usize, usize),
}

impl CorrectionCharacterSource {
    pub(crate) fn range(self) -> (usize, usize) {
        match self {
            Self::Candidates(start, end) | Self::Copied(start, end) => (start, end),
        }
    }
}

/// 診断は残すが、値を保存できないためファイル全体で取り消す補正。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CorrectionRejection {
    WordArrayInvalidBytes,
}

impl CorrectionRejection {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::WordArrayInvalidBytes => {
                "Style/WordArray would change the bytes of a string literal"
            }
        }
    }
}

#[derive(Clone, Debug)]
struct RejectedCorrections {
    reason: CorrectionRejection,
    edits: Vec<Edit>,
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
    /// 危険な置換は公開編集列へ返さず、選択・衝突判定だけに渡す。補正時は再検査するため、
    /// CachedOffenseには保存しない。採用された場合だけengineが原本全体へ戻す。
    rejected_corrections: Option<RejectedCorrections>,
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
    /// 編集ごとに cop が指定した挿入のアンカー。[`Offense::corrections`] と同じ添字で引き、列が
    /// 短くて届かない編集と `None` の編集は [`Offense::correction_anchor`] に従う。
    ///
    /// 本家の cop は 1 回の補正の中で、`insert_before` と `insert_after` に別々の範囲を渡せる。
    /// `Lint/AmbiguousBlockAssociation` はメソッド名の後ろの空白に `remove` と
    /// `insert_before(空白, '(')` を、最後の引数に `insert_after(')')` を呼ぶ。空白への 2 つを
    /// `(` への置換 1 つに畳むと、同じ空白を `(` に置換する `Style/NestedParenthesizedCalls` と
    /// **同じ置換**になって衝突しなくなる。本家では置換が `""` と `"("` で食い違うので後の cop が
    /// そのパスで捨てられるが、畳んだ形では両方の `)` が入って構文を壊していた。
    ///
    /// 列は [`Offense::corrections`] と揃っていなければならないので、builder と
    /// [`Offense::clear_corrections`] でしか書き換えない。
    edit_anchors: Vec<Option<InsertionAnchor>>,
    /// 置換後の文字が実際に由来するソース範囲。補間の先行文など、捨てる範囲を除く。
    pub(crate) correction_characters_from: Option<CorrectionCharacterSource>,
    /// Set for edits the cop scheduled outside `add_offense`, which is where `Cop::Base#correct`
    /// puts a rewrite belonging to no single offense. The offense keeps the `:unsupported` status
    /// it was reported with -- neither `correctable` nor ever stamped corrected -- while the edits
    /// still reach the run's corrector. See [`Offense::corrected_without_status`].
    pub corrections_detached: bool,
    pub snapshot: Option<OffenseSnapshot>,
    /// RuboCop's JSON `length` when it is measured in display columns rather than by the source
    /// range itself. Tabs are the only current caller: the caret range stays on source characters
    /// while the length includes the extra columns those tabs occupy.
    length_override: Option<usize>,
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
            rejected_corrections: None,
            correction_anchor: None,
            edit_anchors: Vec::new(),
            correction_characters_from: None,
            corrections_detached: false,
            snapshot: None,
            length_override: None,
        }
    }

    pub fn with_length(mut self, length: usize) -> Self {
        self.length_override = Some(length);
        self
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

    /// 本家の corrector の呼び出しを 1 つずつ写して積む。挿入には [`InsertionAnchor::insert`] で
    /// 範囲と向きを添え、置換と削除は `None` にする (空でない範囲は置換なので、アンカーは効かない)。
    /// `None` の挿入は [`Offense::corrected_by_all`] で積んだものと同じ扱いになる。
    /// See [`Offense::edit_anchors`].
    pub fn corrected_by_all_anchored(
        mut self,
        edits: impl IntoIterator<Item = (Edit, Option<InsertionAnchor>)>,
    ) -> Self {
        for (edit, anchor) in edits {
            // 通常の builder と交互に呼んでも添字がずれないよう、先に既存の編集数まで埋める。
            self.edit_anchors.resize(self.corrections.len(), None);
            self.edit_anchors.push(anchor);
            self.corrections.push(edit);
        }
        self.correctable = !self.corrections.is_empty();
        self
    }

    /// `index` 番目の編集に cop が指定したアンカー。指定が無ければ `None` で、その編集は
    /// [`Offense::correction_anchor`] か報告した範囲に掛かる。
    pub fn edit_anchor(&self, index: usize) -> Option<&InsertionAnchor> {
        self.edit_anchors.get(index).and_then(Option::as_ref)
    }

    /// 編集を、それに添えたアンカーごと捨てる。
    pub fn clear_corrections(&mut self) {
        self.corrections.clear();
        self.edit_anchors.clear();
        self.rejected_corrections = None;
    }

    /// 診断用のlossy値を実編集として公開せず、採用可否を既存correctorで判定する。
    pub(crate) fn reject_corrections(mut self, reason: CorrectionRejection) -> Self {
        self.rejected_corrections = Some(RejectedCorrections {
            reason,
            edits: std::mem::take(&mut self.corrections),
        });
        self
    }

    pub(crate) fn correction_rejection(&self) -> Option<CorrectionRejection> {
        self.rejected_corrections
            .as_ref()
            .map(|blocked| blocked.reason)
    }

    /// 拒否予定も同じ配置条件で照合するが、この列を直接書き出してはいけない。
    pub(crate) fn planned_corrections(&self) -> &[Edit] {
        match &self.rejected_corrections {
            Some(blocked) => &blocked.edits,
            None => &self.corrections,
        }
    }

    pub(crate) fn planned_corrections_mut(&mut self) -> &mut [Edit] {
        match &mut self.rejected_corrections {
            Some(blocked) => &mut blocked.edits,
            None => &mut self.corrections,
        }
    }

    /// 値を作る補正では、置換範囲と値の出典の範囲が一致しないことがある。
    pub(crate) fn correction_characters_from(mut self, range: Range<usize>) -> Self {
        self.correction_characters_from = Some(CorrectionCharacterSource::Candidates(
            range.start,
            range.end.max(range.start),
        ));
        self
    }

    /// ASCII の構文だけを変え、範囲内の非 ASCII をすべてそのまま複写する補正。
    pub(crate) fn correction_copies_characters_from(mut self, range: Range<usize>) -> Self {
        self.correction_characters_from = Some(CorrectionCharacterSource::Copied(
            range.start,
            range.end.max(range.start),
        ));
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
            length: self
                .length_override
                .unwrap_or_else(|| character_length(source, self.start, self.end)),
            line: start_line,
            column: start_column,
        }
    }
}

/// 本家が報告する文字単位の範囲長。CRLF は改行 1 文字として数える。
/// 算術で求めたオフセットは文字の途中に落ち得るため、両端を文字境界まで戻す。
pub(crate) fn character_length(source: &SourceFile, start: usize, end: usize) -> usize {
    let text = source.text();
    let mut start = start.min(text.len());
    let mut end = end.clamp(start, text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    let span = &text[start..end];
    let crlf_count = span
        .as_bytes()
        .windows(2)
        .filter(|pair| *pair == b"\r\n")
        .count();
    span.chars().count() - crlf_count
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
    use super::{Edit, InsertionAnchor, Offense, Severity, character_length};
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

    /// 本家は CRLF を改行 1 文字として扱い、複数行の offense 長に CR を足さない。
    #[test]
    fn crlf_counts_as_one_character_in_a_range() {
        let source = source("begin\r\nrescue Exception\r\nend\r\n");
        let end = "begin\r\nrescue Exception\r\n".len();
        assert_eq!(
            character_length(&source, 0, end),
            "begin\nrescue Exception\n".len()
        );
        assert_eq!(character_length(&source, 5, 6), 1);
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

    /// アンカーはそれを添えた編集にだけ効き、通常の builder と交互に積んでも添字がずれない。
    #[test]
    fn an_edit_anchor_belongs_to_its_own_edit() {
        let anchored = offense(0, 9)
            .corrected_by_all([edit(1, 2)])
            .corrected_by_all_anchored([
                (edit(2, 3), None),
                InsertionAnchor::After(4..9).insert(")"),
            ])
            .corrected_by_all([edit(9, 9)])
            .corrected_by_all_anchored([InsertionAnchor::Before(0..0).insert("(")]);
        assert_eq!(anchored.corrections.len(), 5);
        assert!(anchored.is_correctable());
        assert_eq!(anchored.edit_anchor(0), None);
        assert_eq!(anchored.edit_anchor(1), None);
        assert_eq!(anchored.edit_anchor(2), Some(&InsertionAnchor::After(4..9)));
        assert_eq!(anchored.edit_anchor(3), None);
        assert_eq!(
            anchored.edit_anchor(4),
            Some(&InsertionAnchor::Before(0..0))
        );
        assert_eq!(anchored.edit_anchor(5), None);
    }

    /// 挿入の位置は向きで決まる。`After` は範囲の末尾、`Before` は先頭に落ちる。
    #[test]
    fn an_anchored_insertion_lands_where_its_side_says() {
        let (after, _) = InsertionAnchor::After(4..9).insert(")");
        assert_eq!(
            (after.start, after.end, after.replacement.as_str()),
            (9, 9, ")")
        );
        let (before, _) = InsertionAnchor::Before(4..9).insert("(");
        assert_eq!(
            (before.start, before.end, before.replacement.as_str()),
            (4, 4, "(")
        );
        assert!(after.safe && before.safe);
    }

    /// 編集を捨てるときはアンカーも捨てる。後から積んだ編集が古いアンカーを引き継がない。
    #[test]
    fn clearing_the_corrections_drops_their_anchors() {
        let mut anchored =
            offense(0, 9).corrected_by_all_anchored([InsertionAnchor::After(4..9).insert(")")]);
        anchored.clear_corrections();
        assert!(anchored.corrections.is_empty());
        assert_eq!(anchored.edit_anchor(0), None);
        let anchored = anchored.corrected_by_all([edit(9, 9)]);
        assert_eq!(anchored.edit_anchor(0), None);
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
