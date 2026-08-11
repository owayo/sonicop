use std::fmt;

use serde::Serialize;

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
    pub correction: Option<Edit>,
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
            correction: None,
            snapshot: None,
        }
    }

    pub fn corrected_by(mut self, edit: Edit) -> Self {
        self.correctable = true;
        self.correction = Some(edit);
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
        // Stepping back one byte from the exclusive end can land inside a multibyte character,
        // which `line_column` cannot slice at; walk back to the character that byte belongs to.
        let mut inclusive_end = self.end.saturating_sub(1).max(self.start);
        while inclusive_end > self.start && !source.text().is_char_boundary(inclusive_end) {
            inclusive_end -= 1;
        }
        let (last_line, last_column) = source.line_column(inclusive_end);
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

#[derive(Clone, Copy, Debug, Serialize)]
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
