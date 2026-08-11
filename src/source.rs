use std::ops::Range;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

#[derive(Clone, Debug)]
pub struct SourceFile {
    path: PathBuf,
    text: String,
    line_starts: Vec<usize>,
    /// Whether the file opened with a UTF-8 byte order mark, which `text` no longer carries.
    byte_order_mark: bool,
}

/// The UTF-8 byte order mark, which `parser` strips before handing the source to a cop but which
/// RuboCop still writes back out when it corrects the file.
pub const BYTE_ORDER_MARK: &str = "\u{feff}";

impl SourceFile {
    pub fn new(path: impl Into<PathBuf>, text: String) -> Self {
        // Ruby's parser re-encodes the source but leaves its content alone, so a leading byte order
        // mark stays in the buffer and every column on the first line counts it. Only the *tokens*
        // exclude it, which is why a cop reading comment tokens sees a magic comment where one
        // reading raw lines does not.
        let byte_order_mark = text.starts_with(BYTE_ORDER_MARK);
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        );
        Self {
            path: path.into(),
            text,
            line_starts,
            byte_order_mark,
        }
    }

    /// Whether the source opens with a byte order mark. A cop that reads a raw line rather than a
    /// token has to step over it the way the tokenizer already has.
    pub fn starts_with_byte_order_mark(&self) -> bool {
        self.byte_order_mark
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

    pub fn line(&self, one_based_line: usize) -> &str {
        let range = self.line_range(one_based_line);
        &self.text[range]
    }

    /// An offset landing inside a multibyte character is rounded down to that character's start
    /// rather than panicking: a cop reporting a byte range it derived by arithmetic must not be
    /// able to abort the whole run.
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
