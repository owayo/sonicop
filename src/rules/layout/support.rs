//! Scanning and node grouping shared by more than one Layout cop.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Edit;
use crate::rules::RuleContext;

/// The run of spaces and tabs ending at `offset`.
pub(super) fn whitespace_before(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start..offset
}

/// The run of spaces and tabs starting at `offset`.
pub(super) fn whitespace_after(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut end = offset;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    offset..end
}

/// The hash literals of a file, each as the run of elements upstream's parser folds into one
/// `hash` node.
///
/// A braced hash is a node of its own here as well, but a brace-less one -- `foo(a: 1, b: 2)`,
/// `[a: 1]`, `foo[a: 1]` -- is not: the grammar leaves its pairs as siblings of whatever was
/// written before them, while upstream's parser wraps the trailing run of `key: value` pairs and
/// `**splat`s into a single `hash`. A cop written against `on_hash` has to see that run as one
/// literal or it measures alignment against the wrong first pair.
pub(super) fn hash_literals<'tree>(
    context: &'tree RuleContext<'tree>,
) -> Vec<Vec<Node<'tree>>> {
    let mut literals: Vec<(usize, Vec<Node<'tree>>)> = Vec::new();
    for node in context.nodes_of("hash") {
        let mut cursor = node.walk();
        let elements: Vec<Node<'tree>> = node
            .named_children(&mut cursor)
            .filter(|child| is_hash_element(*child))
            .collect();
        if !elements.is_empty() {
            literals.push((node.start_byte(), elements));
        }
    }
    for container in context.nodes_of_any(&["argument_list", "array", "element_reference"]) {
        let mut cursor = container.walk();
        let children: Vec<Node<'tree>> = container.named_children(&mut cursor).collect();
        let mut index = 0;
        while index < children.len() {
            if !is_hash_element(children[index]) {
                index += 1;
                continue;
            }
            let start = index;
            while index < children.len() && is_hash_element(children[index]) {
                index += 1;
            }
            literals.push((children[start].start_byte(), children[start..index].to_vec()));
        }
    }
    literals.sort_by_key(|(start, _)| *start);
    literals.into_iter().map(|(_, elements)| elements).collect()
}

fn is_hash_element(node: Node<'_>) -> bool {
    matches!(node.kind(), "pair" | "hash_splat_argument")
}

/// `Util.begins_its_line?`: the first non-blank character of the line is where the node starts.
pub(super) fn begins_its_line(context: &RuleContext<'_>, offset: usize) -> bool {
    let line = context.source.line_column(offset).0;
    let start = context.source.line_start(line);
    context.source.text()[start..offset]
        .chars()
        .all(char::is_whitespace)
}

/// A set of `insert_before` and `remove` corrections over one node, collapsed into the single
/// replacement `Edit` carries.
pub(super) struct Edits<'a> {
    text: &'a str,
    /// `(start, end, replacement)` triples, in the order they were recorded.
    parts: Vec<(usize, usize, String)>,
}

impl<'a> Edits<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        Self {
            text,
            parts: Vec::new(),
        }
    }

    /// `HashAlignment#adjust`: a positive delta pads before `offset`, a negative one eats that
    /// many characters off the padding already there.
    pub(super) fn adjust(&mut self, offset: usize, delta: i64) {
        match delta.cmp(&0) {
            std::cmp::Ordering::Greater => {
                let width = usize::try_from(delta).unwrap_or(0);
                self.parts.push((offset, offset, " ".repeat(width)));
            }
            std::cmp::Ordering::Less => {
                let width = usize::try_from(-delta).unwrap_or(0);
                let mut start = offset;
                for _ in 0..width {
                    if start == 0 {
                        break;
                    }
                    start -= 1;
                    while start > 0 && !self.text.is_char_boundary(start) {
                        start -= 1;
                    }
                }
                self.parts.push((start, offset, String::new()));
            }
            std::cmp::Ordering::Equal => {}
        }
    }

    /// The one edit that spans every recorded correction, with the source between them carried
    /// through unchanged.
    pub(super) fn finish(mut self) -> Option<Edit> {
        self.parts.retain(|(start, end, replacement)| {
            *start != *end || !replacement.is_empty()
        });
        if self.parts.is_empty() {
            return None;
        }
        self.parts.sort_by_key(|(start, end, _)| (*start, *end));
        let start = self.parts[0].0;
        let end = self.parts.iter().map(|(_, end, _)| *end).max()?;
        let mut replacement = String::new();
        let mut cursor = start;
        for (part_start, part_end, text) in &self.parts {
            // Two corrections that eat into the same padding would clobber each other upstream,
            // which leaves the offense uncorrected rather than half-corrected.
            if *part_start < cursor {
                return None;
            }
            replacement.push_str(&self.text[cursor..*part_start]);
            replacement.push_str(text);
            cursor = *part_end;
        }
        replacement.push_str(&self.text[cursor..end]);
        Some(Edit {
            start,
            end,
            replacement,
            safe: true,
        })
    }
}
