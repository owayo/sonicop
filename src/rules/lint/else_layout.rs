use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::statements::{Branch, statements};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Odd `else` layout detected. Did you mean to use `elsif`?";

/// `Layout/IndentationWidth`'s `Width`, which is what `Alignment#indentation` adds to the column.
const DEFAULT_INDENTATION_WIDTH: usize = 2;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_if` reaches the nested `if` an `elsif` is as well as the one written with the keyword,
    // and `check` walks the chain from either end -- so the same `else` is reached twice and the
    // offense deduplicated by its range, the way `add_offense` does.
    let mut reported: Vec<usize> = Vec::new();
    for node in context.nodes_of_any(&["if", "unless", "elsif"]) {
        let alternative = node.field("alternative");
        // `node.then? && !node.else_branch&.begin_type?`: a body written after `then` is only odd
        // when it holds more than one statement.
        let single_statement = alternative
            .filter(|clause| clause.kind_str() == "else")
            .is_none_or(|clause| statements(clause).len() < 2);
        if has_then(node, context) && single_statement {
            continue;
        }
        if is_single_line(node, context) {
            continue;
        }
        walk(node, context, offenses, &mut reported);
    }
}

/// `check`: report an `else` written with a body on its own line, or step into the `elsif` chain.
fn walk(
    node: Node<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    reported: &mut Vec<usize>,
) {
    let Some(alternative) = node.field("alternative") else {
        return;
    };
    match alternative.kind_str() {
        "else" => check_else(node, alternative, context, offenses, reported),
        // `node.if?`: an `unless` has no `elsif` to step into, and neither has an `elsif` itself
        // -- but the parser makes one a nested `if`, which is what the recursion follows.
        "elsif" if node.kind_str() != "unless" => walk(alternative, context, offenses, reported),
        _ => {}
    }
}

fn check_else(
    node: Node<'_>,
    clause: Node<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    reported: &mut Vec<usize>,
) {
    let Some(keyword) = clause.child(0) else {
        return;
    };
    let first = match Branch::of(Some(clause)) {
        Branch::Missing => return,
        Branch::One(node) => node,
        Branch::Sequence(nodes) => nodes[0],
    };
    if context.source.line_column(first.start_byte()).0
        != context.source.line_column(keyword.start_byte()).0
    {
        return;
    }
    if reported.contains(&first.start_byte()) {
        return;
    }
    reported.push(first.start_byte());
    let width: usize = context
        .setting_of("Layout/IndentationWidth", "Width")
        .unwrap_or(DEFAULT_INDENTATION_WIDTH);
    let indentation = " ".repeat(context.source.line_column(node.start_byte()).1 - 1 + width);
    offenses.push(
        context
            .offense(MSG, first.byte_range())
            .corrections_anchored_at(keyword.byte_range())
            .corrected_by_all([
                Edit {
                    start: keyword.end_byte(),
                    end: keyword.end_byte(),
                    replacement: "\n".to_owned(),
                    safe: true,
                },
                Edit {
                    start: keyword.end_byte(),
                    end: first.start_byte(),
                    replacement: indentation,
                    safe: true,
                },
            ]),
    );
}

/// `node.then?`: the `then` keyword was written after the condition. The grammar puts it inside the
/// branch it introduces rather than beside the condition, so it is the branch's first token.
fn has_then(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("consequence")
        .and_then(|consequence| consequence.child(0))
        .is_some_and(|first| context.source.node_text(first) == "then")
}

fn is_single_line(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    context.source.line_column(node.start_byte()).0 == context.source.line_column(node.end_byte()).0
}
