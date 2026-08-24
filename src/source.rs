use std::ops::Range;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

#[derive(Clone, Debug)]
pub struct SourceFile {
    path: PathBuf,
    text: String,
    line_starts: Vec<usize>,
    /// How long the file was before a NUL in code cut `text` short. RuboCop's buffer holds every
    /// byte of the file even past the point Ruby's lexer stopped reading, so a cop that asks about
    /// the file rather than about the program has to ask about this.
    length_as_read: usize,
}

/// The UTF-8 byte order mark, which `parser` strips before handing the source to a cop but which
/// RuboCop still writes back out when it corrects the file.
pub const BYTE_ORDER_MARK: &str = "\u{feff}";

impl SourceFile {
    pub fn new(path: impl Into<PathBuf>, text: String) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        );
        Self {
            path: path.into(),
            length_as_read: text.len(),
            text,
            line_starts,
        }
    }

    /// Records that the file was longer than `text` before a NUL in code cut it short.
    pub fn read_as_long_as(mut self, length: usize) -> Self {
        self.length_as_read = length;
        self
    }

    /// Whether the file itself held nothing, which is not the same as the program being empty: a
    /// file that opens with a NUL byte has no program and plenty of content.
    pub fn is_empty_as_read(&self) -> bool {
        self.length_as_read == 0
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn slice(&self, range: Range<usize>) -> &str {
        &self.text[range]
    }

    pub fn node_text<'a>(&'a self, node: Node<'_>) -> &'a str {
        &self.text[node.byte_range()]
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, one_based_line: usize) -> usize {
        self.line_starts
            .get(one_based_line.saturating_sub(1))
            .copied()
            .unwrap_or(self.text.len())
    }

    pub fn line_range(&self, one_based_line: usize) -> Range<usize> {
        let start = self.line_start(one_based_line);
        let end = self
            .line_starts
            .get(one_based_line)
            .copied()
            .unwrap_or(self.text.len());
        start..end
    }

    /// The line **including its terminator**, which is where this differs from upstream.
    ///
    /// RuboCop's `processed_source.lines` holds the line with the newline already stripped, so a
    /// cop ported straight across measures one character too many and never matches a
    /// `line.end_with?('\\')` test. Reach for [`Self::line_without_terminator`] whenever the length
    /// or the last character is what the check is about; this one is right when the range is being
    /// sliced or the leading whitespace counted.
    pub fn line(&self, one_based_line: usize) -> &str {
        let range = self.line_range(one_based_line);
        &self.text[range]
    }

    /// `processed_source.lines[n]`: the line without its `\n` or `\r\n`.
    pub fn line_without_terminator(&self, one_based_line: usize) -> &str {
        let line = self.line(one_based_line);
        line.strip_suffix('\n')
            .map_or(line, |line| line.strip_suffix('\r').unwrap_or(line))
    }

    /// An offset landing inside a multibyte character is rounded down to that character's start
    /// rather than panicking: a cop reporting a byte range it derived by arithmetic must not be
    /// able to abort the whole run.
    /// `effective_column`: the column as an editor shows it, which on line 1 of a file that opens
    /// with a byte order mark is one less than the column `parser` reports. Only the cops that
    /// measure one column against another go through this -- the columns a cop **reports** keep the
    /// mark, so `Layout/InitialIndentation` points at the same place upstream does.
    pub fn effective_column(&self, byte_offset: usize) -> usize {
        let (line, column) = self.line_column(byte_offset);
        match line == 1 && self.text.starts_with(BYTE_ORDER_MARK) {
            true => column.saturating_sub(1),
            false => column,
        }
    }

    pub fn line_column(&self, byte_offset: usize) -> (usize, usize) {
        let mut offset = byte_offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line_index = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let line_start = self.line_starts[line_index];
        let column = self.text[line_start..offset].chars().count() + 1;
        (line_index + 1, column)
    }
}

pub fn is_protected(offset: usize, ranges: &[Range<usize>]) -> bool {
    let index = ranges.partition_point(|range| range.start <= offset);
    index > 0 && ranges[index - 1].contains(&offset)
}

#[cfg(test)]
mod tests {
    use super::SourceFile;

    #[test]
    fn calculates_unicode_columns() {
        let source = SourceFile::new("test.rb", "あa\nxyz".to_owned());
        assert_eq!(source.line_column("あ".len()), (1, 2));
        assert_eq!(source.line_column(5), (2, 1));
    }

    #[test]
    fn an_offset_inside_a_multibyte_character_resolves_to_its_start() {
        let source = SourceFile::new("test.rb", "あ = 1\n".to_owned());

        for offset in 0.."あ".len() {
            assert_eq!(source.line_column(offset), (1, 1));
        }
        assert_eq!(source.line_column("あ".len()), (1, 2));
    }
}
