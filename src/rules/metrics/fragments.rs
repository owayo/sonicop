//! The code tree-sitter's lexer read as something other than code.
//!
//! Two shapes reach RuboCop as ordinary expressions but never reach the grammar here as such. A `#`
//! written immediately before an interpolation inside a heredoc opens a comment that swallows the
//! rest of the line, so `<<~X` holding `#{a}##{b}` loses everything after the first interpolation.
//! And `%` applied to a string is read as one more percent literal rather than as the operator, so
//! `"%d" % [n]` becomes two string literals side by side.
//!
//! Both are recovered by parsing what was swallowed. The recovered code is parsed in place -- in a
//! copy of the file with every other byte blanked out -- so its nodes carry the offsets they were
//! written at and can be read against the original source like any others.

use std::ops::Range;

use tree_sitter::{Node, Tree};

use super::locals::named_children;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

pub(in crate::rules) struct Fragments {
    /// The recovered parse, absent when the file holds nothing to recover.
    tree: Option<Tree>,
    /// The nodes whose text was swallowed, by node id.
    hosts: crate::rules::IdSet,
    /// Of those, the ones standing for a `%` applied to the string before them.
    operators: crate::rules::IdSet,
}

impl Fragments {
    pub(in crate::rules) fn new(context: &RuleContext<'_>) -> Self {
        let source = context.source.text();
        // Nothing is copied or parsed until something needs recovering, which almost no file does.
        let comments: Vec<Node<'_>> = context
            .nodes_of("comment")
            .filter(|comment| {
                comment
                    .parent_of(context)
                    .is_some_and(|parent| parent.kind_str() == "heredoc_body")
                    && source[comment.byte_range()].contains("#{")
            })
            .collect();
        let percents: Vec<Node<'_>> = context
            .nodes_of("chained_string")
            .flat_map(|chained| named_children_of(chained, context).into_iter().skip(1))
            .filter(|part| {
                part.kind_str() == "string" && source[part.byte_range()].starts_with('%')
            })
            .collect();
        if comments.is_empty() && percents.is_empty() {
            return Self {
                tree: None,
                hosts: crate::rules::IdSet::default(),
                operators: crate::rules::IdSet::default(),
            };
        }

        let mut hosts = crate::rules::IdSet::default();
        let mut operators = crate::rules::IdSet::default();
        let mut blanked = Blanked::new(source);
        for comment in comments {
            if blanked.write_interpolations(comment.byte_range()) {
                hosts.insert(comment.id());
            }
        }
        for part in percents {
            blanked.write_percent_argument(part);
            hosts.insert(part.id());
            operators.insert(part.id());
        }
        Self {
            tree: blanked.parse(),
            hosts,
            operators,
        }
    }

    /// Whether the node's own text is code the grammar failed to read as such.
    pub(super) fn swallowed(&self, node: Node<'_>) -> bool {
        self.hosts.contains(&node.id())
    }

    /// Whether the swallowed text is the argument of a `%` applied to the string before it, which
    /// is a call that no node here stands for.
    pub(super) fn is_operator(&self, node: Node<'_>) -> bool {
        self.operators.contains(&node.id())
    }

    /// The expressions recovered from the node's text, in the order they were written.
    pub(super) fn roots<'a>(&'a self, node: Node<'_>) -> Vec<Node<'a>> {
        let Some(tree) = self.tree.as_ref().filter(|_| self.swallowed(node)) else {
            return Vec::new();
        };
        let range = node.byte_range();
        named_children(tree.root_node())
            .into_iter()
            .filter(|child| range.contains(&child.start_byte()) && child.end_byte() <= range.end)
            .collect()
    }
}

/// A copy of the file with every byte blanked out but the ones being recovered, which keeps each
/// recovered expression at the offset it was written at.
struct Blanked<'a> {
    source: &'a str,
    bytes: Vec<u8>,
    used: bool,
}

impl<'a> Blanked<'a> {
    fn new(source: &'a str) -> Self {
        // Line breaks are kept so that two expressions recovered from different lines cannot run
        // together into one call taking the other as its argument.
        let bytes = source
            .bytes()
            .map(|byte| if byte == b'\n' { b'\n' } else { b' ' })
            .collect();
        Self {
            source,
            bytes,
            used: false,
        }
    }

    fn keep(&mut self, range: Range<usize>) {
        self.bytes[range.clone()].copy_from_slice(&self.source.as_bytes()[range]);
        self.used = true;
    }

    /// Copies every `#{…}` written in a swallowed comment into place, each on a line of its own.
    fn write_interpolations(&mut self, host: Range<usize>) -> bool {
        let bytes = self.source.as_bytes();
        let mut index = host.start;
        let mut found = false;
        while index + 1 < host.end {
            if bytes[index] != b'#' || bytes[index + 1] != b'{' {
                index += 1;
                continue;
            }
            let start = index + 2;
            let Some(end) = closing_brace(bytes, start, host.end) else {
                break;
            };
            self.keep(start..end);
            self.bytes[index] = b'\n';
            self.bytes[index + 1] = b' ';
            self.bytes[end] = b'\n';
            found = true;
            index = end + 1;
        }
        found
    }

    /// Copies the contents of `%[…]` into place, with brackets standing in for whichever pair of
    /// delimiters was written so that the argument reads as one array.
    fn write_percent_argument(&mut self, part: Node<'_>) {
        let content = named_children(part)
            .into_iter()
            .find(|child| child.kind_str() == "string_content");
        let (open, close) = match content {
            Some(content) => {
                self.keep(content.byte_range());
                (content.start_byte() - 1, content.end_byte())
            }
            None => (part.start_byte() + 1, part.end_byte() - 1),
        };
        self.bytes[open] = b'[';
        self.bytes[close] = b']';
        self.used = true;
    }

    fn parse(self) -> Option<Tree> {
        if !self.used {
            return None;
        }
        let text = String::from_utf8(self.bytes).ok()?;
        crate::parser::parse(&text)
    }
}

/// The `}` that closes an interpolation, counting the braces written inside it.
fn closing_brace(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in bytes.iter().enumerate().take(end).skip(start) {
        match byte {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(offset),
            b'}' => depth -= 1,
            _ => {}
        }
    }
    None
}
