//! `Layout/BlockEndNewline`.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{begins_its_line, heredoc_terminators};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for node in context.nodes_of_any(&["block", "do_block"]) {
        if node.start_position().row == node.end_position().row {
            continue;
        }
        let Some(closing) = closing_keyword(node) else {
            continue;
        };
        if begins_its_line(context, closing.start_byte()) {
            continue;
        }
        // `node.children.compact.last`: the body when the block has one, and the parameter list
        // when it does not -- the receiver is not a child of the block upstream.
        let Some(last) = last_child(node) else {
            continue;
        };
        let offense_range = parser_end(last)..closing.end_byte();
        let source = &text[offense_range.clone()];
        if source.trim_start().starts_with(';') {
            continue;
        }
        let (line, column) = context.source.line_column(closing.start_byte());
        let replacement = format!("\n{}", source.trim_start());
        let offense = context.offense(
            format!("Expression at {line}, {column} should be on its own line."),
            closing.byte_range(),
        );
        offenses.push(match last_heredoc_argument(context, node) {
            // The `end` cannot move up past a heredoc body that was written after it, so the text
            // it is replaced with goes after the terminator instead.
            Some(terminator) => offense
                .corrected_by_all([
                    Edit {
                        start: terminator.end,
                        end: terminator.end,
                        replacement,
                        safe: true,
                    },
                    Edit {
                        start: offense_range.start,
                        end: offense_range.end,
                        replacement: String::new(),
                        safe: true,
                    },
                ])
                .corrections_anchored_at(terminator),
            None => offense.corrected_by(Edit {
                start: offense_range.start,
                end: offense_range.end,
                replacement,
                safe: true,
            }),
        });
    }
}

/// `node.loc.end`: the `}` or `end` the block closes with.
fn closing_keyword<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let last = node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)?;
    matches!(last.kind_str(), "}" | "end").then_some(last)
}

/// Where the node upstream builds for a body ends. The grammar's container reaches past the last
/// statement to take in a trailing `;`, which the parser's `begin` node does not.
fn parser_end(node: Node<'_>) -> usize {
    let mut current = node;
    while matches!(current.kind_str(), "body_statement" | "block_body") {
        let Some(last) = last_child(current) else {
            break;
        };
        current = last;
    }
    current.end_byte()
}

fn last_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body" | "empty_statement"))
        .last()
}

/// `last_heredoc_argument`: the terminator of the last heredoc handed to the call the block body
/// consists of, following the receiver chain when the call itself was given none.
///
/// Only a heredoc upstream's parser calls a `str` counts, which leaves out the ones whose body is
/// empty, spans more than one line or interpolates.
fn last_heredoc_argument(
    context: &RuleContext<'_>,
    block: Node<'_>,
) -> Option<std::ops::Range<usize>> {
    let mut node = body_call(block)?;
    let bodies: Vec<Node<'_>> = context.nodes_of("heredoc_body").collect();
    let terminators = heredoc_terminators(context);
    loop {
        if let Some(list) = argument_list(node) {
            let mut cursor = list.walk();
            let found = list
                .named_children(&mut cursor)
                .filter(|child| child.kind_str() == "heredoc_beginning")
                .filter_map(|child| single_line_heredoc(context, &bodies, &terminators, child))
                .last();
            if found.is_some() {
                return found;
            }
        }
        node = node.field("receiver")?;
        if node.kind_str() != "call" {
            return None;
        }
    }
}

/// The single statement a block body consists of, when that statement is a call.
fn body_call<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    let body = last_child(block).filter(|child| {
        matches!(child.kind_str(), "body_statement" | "block_body") && child.named_child_count() > 0
    })?;
    let mut cursor = body.walk();
    let statements: Vec<Node<'tree>> = body
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect();
    match statements.as_slice() {
        [only] if only.kind_str() == "call" => Some(*only),
        _ => None,
    }
}

fn argument_list<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = call.walk();
    call.children(&mut cursor)
        .find(|child| child.kind_str() == "argument_list")
}

fn single_line_heredoc(
    context: &RuleContext<'_>,
    bodies: &[Node<'_>],
    terminators: &[(usize, Range<usize>)],
    opener: Node<'_>,
) -> Option<Range<usize>> {
    let index = terminators
        .iter()
        .position(|(offset, _)| *offset == opener.start_byte())?;
    let terminator = terminators[index].1.clone();
    let content = &context.source.text()[bodies.get(index)?.start_byte()..terminator.start];
    // `str` rather than `dstr`: one line of body, and nothing in it the parser has to evaluate.
    (content.lines().count() == 1 && !content.contains("#{")).then_some(terminator)
}
