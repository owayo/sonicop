//! Tree walks shared by cops in more than one department.

use std::ops::Range;

use tree_sitter::{Node, Parser};

use crate::diagnostic::Edit;
use crate::rules::RuleContext;

/// `ReparsedEquivalence#correction_parses?`: whether the exact correction a cop is about to offer
/// leaves source that still parses.
///
/// A cop that rewrites a construct into a differently shaped one cannot assert that the result
/// means the same thing, but it can insist that the result is Ruby at all. Upstream turns that into
/// the gate an offense is reported behind, which is what keeps a corrector that cannot handle an
/// unusual shape from emitting broken code rather than staying quiet.
pub(crate) fn correction_parses(context: &RuleContext<'_>, edits: &[Edit]) -> bool {
    // `Parser::ClobberingError`: a rewrite whose parts collide is no correction to begin with.
    let Some(corrected) = apply_edits(context.source.text(), edits) else {
        return false;
    };
    parses(&corrected)
}

/// The source with every edit applied, or `None` when two of them overlap.
///
/// Sorting by span puts an insertion at a span's start before the span itself and one at its end
/// after it, which is the order `insert_before` and `insert_after` schedule them in.
fn apply_edits(text: &str, edits: &[Edit]) -> Option<String> {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.start, edit.end));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for edit in ordered {
        if edit.start < cursor || edit.end < edit.start || edit.end > text.len() {
            return None;
        }
        out.push_str(text.get(cursor..edit.start)?);
        out.push_str(&edit.replacement);
        cursor = edit.end;
    }
    out.push_str(text.get(cursor..)?);
    Some(out)
}

/// `ProcessedSource#valid_syntax?` for a source the run did not start from.
fn parses(text: &str) -> bool {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .is_err()
    {
        return false;
    }
    parser
        .parse(text, None)
        .is_some_and(|tree| !tree.root_node().has_error())
}

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
