//! `Layout/ClosingParenthesisIndentation`.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::support::{
    alignment_corrections, begins_its_line, character_column, grouped_arguments,
    holds_block_comment, line_indentation, string_interiors,
};
use crate::rules::node_ext::NodeExt;

const MSG_ALIGN: &str = "Align `)` with `(`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    // An index read is a `:[]` send upstream whose source map carries no `begin` or `end` at all,
    // so its brackets are never what this cop looks at.
    for node in context.nodes_of_any(&[
        "call",
        "interpolation",
        "parenthesized_statements",
        "method",
        "singleton_method",
    ]) {
        // `on_send` / `on_csend` / `on_begin` / `on_def` are all this cop handles, and **there is no
        // `on_super`** -- `super(...)` is a node of its own upstream, so its parentheses are never
        // looked at. The grammar writes it as a `call`, which would otherwise align the `)` of a
        // `super(<<~X` against the `(` and move a line upstream leaves alone.
        if crate::rules::send_node::is_super_call(node) {
            continue;
        }
        let Some(call) = delimited(node) else {
            continue;
        };
        // `begins_its_line?(right_paren)`: a closing delimiter sharing its line with code is
        // wherever the line put it.
        if !begins_its_line(context, call.close.start_byte()) {
            continue;
        }
        let actual = character_column(context, call.close.start_byte());
        let open_column = character_column(context, call.open.start_byte());
        let expected = match call.elements.first() {
            None => {
                // `correct_column_candidates`: any of the three columns will do, and the first is
                // what the correction aims at.
                let candidates = [
                    line_indentation(context, call.open.start_byte()),
                    open_column,
                    character_column(context, call.start),
                ];
                if candidates.contains(&actual) {
                    continue;
                }
                candidates[0]
            }
            Some(first) => expected_column(context, &call, *first, open_column, width),
        };
        let delta = expected - actual;
        if delta == 0 {
            continue;
        }
        let message = if expected == open_column {
            MSG_ALIGN.to_owned()
        } else {
            format!("Indent `)` to column {expected} (not {actual})")
        };
        let expr = call.close.byte_range();
        let mut offense = context.offense(message, expr.clone());
        if !holds_block_comment(context, &expr) {
            let taboo = string_interiors(context, &expr);
            offense = offense.corrected_by_all(alignment_corrections(context, expr, delta, &taboo));
        }
        offenses.push(offense);
    }
}

/// `expected_column`.
fn expected_column(
    context: &RuleContext<'_>,
    call: &Delimited<'_>,
    first: usize,
    open_column: i64,
    width: i64,
) -> i64 {
    // `line_break_after_left_paren?`
    if context.source.line_column(first).0 > context.source.line_column(call.open.start_byte()).0 {
        return (line_indentation(context, first) - width).max(0);
    }
    if all_elements_aligned(context, call) {
        return open_column;
    }
    line_indentation(context, first)
}

/// `all_elements_aligned?`: a leading hash argument is judged by its own pairs rather than by
/// itself.
fn all_elements_aligned(context: &RuleContext<'_>, call: &Delimited<'_>) -> bool {
    let columns: Vec<i64> = match &call.first_hash_parts {
        Some(parts) => parts
            .iter()
            .map(|part| character_column(context, *part))
            .collect(),
        None => call
            .elements
            .iter()
            .map(|element| character_column(context, *element))
            .collect(),
    };
    let Some(first) = columns.first() else {
        return false;
    };
    columns.iter().all(|column| column == first)
}

/// One construct written with a bracket pair, as this cop sees it: where upstream's node starts,
/// the two delimiters, and the elements between them.
struct Delimited<'tree> {
    start: usize,
    open: Node<'tree>,
    close: Node<'tree>,
    elements: Vec<usize>,
    /// The columns `all_elements_aligned?` reads when the first element is a hash.
    first_hash_parts: Option<Vec<usize>>,
}

fn delimited<'tree>(node: Node<'tree>) -> Option<Delimited<'tree>> {
    let (start, container, opener, closer) = match node.kind_str() {
        "call" => (
            node.start_byte(),
            child_of_kind(node, "argument_list")?,
            "(",
            ")",
        ),
        "parenthesized_statements" => (node.start_byte(), node, "(", ")"),
        // The parser wraps an interpolation's statements in a `begin` whose delimiters are the
        // `#{` and the `}`, which is what `on_begin` then measures.
        "interpolation" => (node.start_byte(), node, "#{", "}"),
        // `check(node.arguments, node.arguments)`: the parameter list stands in for the definition.
        _ => {
            let parameters = node.field("parameters")?;
            (parameters.start_byte(), parameters, "(", ")")
        }
    };
    let open = child_of_kind(container, opener)?;
    let close = last_child_of_kind(container, closer)?;

    let elements = match node.kind_str() {
        "call" => grouped(node),
        _ => {
            let mut cursor = container.walk();
            container
                .named_children(&mut cursor)
                .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
                .map(|child| (child.start_byte(), hash_parts(child)))
                .collect()
        }
    };
    let first_hash_parts = elements.first().and_then(|(_, parts)| parts.clone());
    Some(Delimited {
        start,
        open,
        close,
        elements: elements.into_iter().map(|(start, _)| start).collect(),
        first_hash_parts,
    })
}

/// The arguments of a call, with the parts of a leading hash kept alongside.
fn grouped(node: Node<'_>) -> Vec<(usize, Option<Vec<usize>>)> {
    grouped_arguments(node)
        .into_iter()
        .map(|argument| {
            let parts = if argument.hash_run {
                Some(
                    argument
                        .parts
                        .iter()
                        .map(tree_sitter::Node::start_byte)
                        .collect(),
                )
            } else {
                hash_parts(argument.parts[0])
            };
            (argument.range.start, parts)
        })
        .collect()
}

/// `elements.first.hash_type?`: the columns of a braced hash's own pairs.
fn hash_parts(node: Node<'_>) -> Option<Vec<usize>> {
    if node.kind_str() != "hash" {
        return None;
    }
    let mut cursor = node.walk();
    Some(
        node.named_children(&mut cursor)
            .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
            .map(|child| child.start_byte())
            .collect(),
    )
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind_str() == kind)
}

fn last_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.kind_str() == kind)
        .last()
}
