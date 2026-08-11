//! Tree walks shared by cops in more than one department.

use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;

/// Pushes `node`'s named children so that popping the stack yields them in
/// source order, making a `pop`-driven loop reproduce depth-first pre-order.
pub(crate) fn push_named_children<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let start = stack.len();
    let mut cursor = node.walk();
    stack.extend(node.named_children(&mut cursor));
    stack[start..].reverse();
}

pub(crate) fn walk_named(node: Node<'_>, callback: &mut impl FnMut(Node<'_>)) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        callback(current);
        push_named_children(current, &mut stack);
    }
}

/// Node kinds whose span is literal text. The code inside a `#{...}` is not, even though the
/// string around it is, so these are what re-cover an offset once an interpolation has uncovered
/// it.
const LITERAL_KINDS: &[&str] = &[
    "comment",
    "string",
    "symbol",
    "simple_symbol",
    "delimited_symbol",
    "heredoc_body",
    "regex",
    "subshell",
    "bare_string",
    "character",
];

/// The `#{...}` spans of the file, which are code even though the string around them is not.
pub(crate) struct Interpolations {
    spans: Vec<Range<usize>>,
    literals: Vec<Range<usize>>,
}

impl Interpolations {
    pub(crate) fn new(context: &RuleContext<'_>) -> Self {
        Self {
            spans: context
                .nodes_of("interpolation")
                .map(|node| node.byte_range())
                .collect(),
            literals: context
                .nodes_of_any(LITERAL_KINDS)
                .map(|node| node.byte_range())
                .collect(),
        }
    }

    /// Whether `offset` sits in interpolated code rather than in the text around it.
    ///
    /// A literal opened inside the interpolation covers it again, which is what keeps the `;` of
    /// `"#{x.sub(/;/, '')}"` out of the token stream.
    pub(crate) fn holds_code(&self, offset: usize) -> bool {
        let Some(innermost) = self
            .spans
            .iter()
            .filter(|span| span.contains(&offset))
            .map(|span| span.start)
            .max()
        else {
            return false;
        };
        !self
            .literals
            .iter()
            .any(|literal| literal.start > innermost && literal.contains(&offset))
    }
}
