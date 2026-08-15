//! `1..` on one line and its end on the next.
//!
//! Inside brackets, where a line break separates nothing, tree-sitter reads those two lines as one
//! range and the cop's own question can be asked of it directly. Everywhere else it reads them as
//! an endless range followed by a statement of its own, while upstream's parser reads one `irange`
//! -- and since the cop is about exactly that ambiguity, the range has to be put back together
//! here before anything can be asked of it at all.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::statements::{holds_statements, statements};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("range") {
        // `node.begin && node.end`: a beginless range says nothing about this.
        let (Some(begin), Some(operator)) = (node.field("begin"), operator(node, context)) else {
            continue;
        };
        // A range put back together swallowed the statement after it, so the sequence around it
        // holds one fewer than the grammar counted.
        let (end, absorbed) = match node.field("end") {
            Some(end) => (end, 0),
            None => match continued_end(node, context) {
                Some(end) => (end, 1),
                None => continue,
            },
        };
        // `same_line?(node.loc.operator, node.end)`.
        if context.source.line_column(operator.start_byte()).0
            == context.source.line_column(end.start_byte()).0
        {
            continue;
        }
        if inside_begin(node, absorbed, context) {
            continue;
        }
        offenses.push(context.offense(
            format!(
                // `"#{node.begin.source}#{node.loc.operator.source}"`: the two pieces joined, so
                // `1 ..` is quoted as `1..`.
                "Wrap the range literal `{}{}` in parentheses to avoid confusion with an endless \
                 range.",
                context.source.node_text(begin),
                context.source.node_text(operator)
            ),
            node.start_byte()..end.end_byte(),
        ));
    }
}

/// `node.loc.operator`.
fn operator<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && matches!(context.source.node_text(*child), ".." | "..."))
}

/// What upstream's parser would have taken as the range's end, if anything.
///
/// Ruby carries a range across a line break whenever an expression is what comes next, and stops
/// at anything that cannot start one -- a `,`, a closing bracket, `then`, `end`. Rather than
/// listing those, the next expression the grammar found is looked up and the text in between is
/// checked: only blanks and comments mean the two really were one range.
fn continued_end<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    let leaf = next_leaf(node, context)?;
    if !only_blank(context.source.slice(node.end_byte()..leaf.start_byte())) {
        return None;
    }
    // The end is the whole expression that leaf opens: `1..` before `8 + 9` ends at the `9`.
    let mut end = leaf;
    while let Some(parent) = end.parent_of(context) {
        if parent.start_byte() != end.start_byte() || holds_statements(parent) {
            break;
        }
        end = parent;
    }
    Some(end)
}

/// The first expression written after the node, wherever the grammar hung it.
fn next_leaf<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    let mut current = node;
    loop {
        if let Some(sibling) = following(current) {
            return Some(innermost(sibling));
        }
        current = current.parent_of(context)?;
    }
}

fn following<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut sibling = node.next_named_sibling();
    while sibling.is_some_and(|next| skippable(next)) {
        sibling = sibling.and_then(|next| next.next_named_sibling());
    }
    sibling
}

/// The node a span opens with, stepping past the container the grammar wrapped it in -- the `then`
/// of a `when` starts where its pattern ended, well before the keyword.
fn innermost<'tree>(node: Node<'tree>) -> Node<'tree> {
    let mut current = node;
    loop {
        let mut cursor = current.walk();
        let first = current
            .named_children(&mut cursor)
            .find(|child| !skippable(*child));
        match first {
            Some(child) => current = child,
            None => return current,
        }
    }
}

fn skippable(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "comment" | "heredoc_body")
}

/// Whether the text is nothing but blanks and comments.
fn only_blank(text: &str) -> bool {
    let mut rest = text;
    // Nothing in the gap can be a string, so a `#` there always opens a comment.
    while let Some(index) = rest.find('#') {
        if !rest[..index].chars().all(char::is_whitespace) {
            return false;
        }
        rest = match rest[index..].find('\n') {
            Some(offset) => &rest[index + offset..],
            None => "",
        };
    }
    rest.chars().all(char::is_whitespace)
}

/// `node.parent&.begin_type?`, asked of the tree upstream would have built.
fn inside_begin(node: Node<'_>, absorbed: usize, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    match parent.kind_str() {
        // `(…)` and `"#{…}"` are a `begin` however little they hold. `begin … end` is a `kwbegin`,
        // which is a type of its own and no reason to stay quiet.
        "parenthesized_statements" | "interpolation" => true,
        "begin" => false,
        // Any other sequence is wrapped only once it holds more than one statement.
        _ if holds_statements(parent) => statements(parent).len() - absorbed > 1,
        _ => false,
    }
}
