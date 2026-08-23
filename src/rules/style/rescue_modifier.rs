//! `Style/RescueModifier`: `x rescue nil` swallows every error, so write the block out.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const MSG: &str = "Avoid using `rescue` in its modifier form.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width = context
        .setting_of::<usize>("Layout/IndentationWidth", "Width")
        .unwrap_or(2);
    for node in context.nodes_of("rescue_modifier") {
        let (Some(operation), Some(handler)) = (node.field("body"), node.field("handler")) else {
            continue;
        };
        // `blah rescue 1 rescue 2` nests the same way in both trees, but upstream reports only the
        // inner one. The outer is left alone.
        if operation.kind_str() == "rescue_modifier" {
            continue;
        }
        // Upstream's parser puts the `rescue` **inside** the assignment (`(masgn (mlhs ..)
        // (rescue (array 1 2) ..))`), so what it reports and what it wraps is the right-hand side.
        // The grammar puts the modifier around the whole assignment, and taking the node as it
        // stands reports `a, b = 1, 2 rescue nil` where upstream reports `1, 2 rescue nil`.
        //
        // **多重代入のときだけ**である。`w = 1, 2 rescue nil` は `(rescue (lvasgn w (array 1 2))
        // ..)` で修飾子が外に出るが、`a, b = 1, 2 rescue nil` は `(masgn (mlhs ..) (rescue ..))`
        // で中に入る。左辺が代入の並びかどうかで分かれる。
        // その振り分けは **2.7 以降のパーサ**の話である。2.6 では多重代入もろとも `rescue` の
        // 中に入る (`(rescue (masgn ..) ..)`) ので、文法が作る形がそのまま本家の形になる。
        let splits_multiple_assignment = context.target_ruby_version() >= RubyVersion::new(2, 7);
        let operation = match splits_multiple_assignment
            && operation
                .field("left")
                .is_some_and(|left| left.kind_str() == "left_assignment_list")
        {
            true => match operation.field("right") {
                Some(right) => right,
                None => continue,
            },
            false => operation,
        };
        let reported = operation.start_byte()..node.end_byte();
        // `parenthesized?`: the parentheses around the whole expression go with the rewrite.
        let parenthesized = node
            .parent_of(context)
            .filter(|parent| parent.kind_str() == "parenthesized_statements");
        // The block is written where **upstream's rescue node** starts, which is the operation --
        // not the whole statement. For `a, b = 1, 2 rescue nil` the two differ by the width of
        // `a, b = `, and taking the statement's column indents the block 7 characters too far left.
        let (indentation, offset) =
            indentation_and_offset(context, operation, width, parenthesized);

        let mut edits = Vec::new();
        // A comma-separated list of values is one array upstream, and it needs brackets once it no
        // longer sits alone on its line.
        if operation.kind_str() == "right_assignment_list" {
            edits.push(insert(operation.start_byte(), "["));
            edits.push(insert(operation.end_byte(), "]"));
        }
        edits.push(Edit {
            start: operation.end_byte(),
            end: node.end_byte(),
            replacement: String::new(),
            safe: true,
        });
        edits.push(insert(
            operation.start_byte(),
            format!("begin\n{indentation}"),
        ));
        let clause = format!(
            "\n{offset}rescue\n{indentation}{}\n{offset}end",
            context.source.node_text(handler)
        );
        // A heredoc opened by the operation has its body written after the whole statement, so the
        // `end` goes after the terminator rather than after the call that opened it.
        let after = heredoc_end(context, operation).unwrap_or_else(|| operation.end_byte());
        edits.push(insert(after, clause));
        if let Some(parenthesized) = parenthesized {
            edits.extend(super::parens::correct(context, parenthesized));
        }
        offenses.push(
            context
                .offense(MSG, reported)
                .corrected_by_all(edits)
                // `insert_before` / `insert_after` are given the operation's range, not the range
                // this offense reports, and that range is what orders them against each other.
                .corrections_anchored_at(operation.byte_range()),
        );
    }
}

fn insert(at: usize, text: impl Into<String>) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text.into(),
        safe: true,
    }
}

/// `indentation_and_offset`: the block is written where the expression stood, one level deeper for
/// its body. Parentheses that are about to go take a column with them.
fn indentation_and_offset(
    context: &RuleContext<'_>,
    node: Node<'_>,
    width: usize,
    parenthesized: Option<Node<'_>>,
) -> (String, String) {
    let column = context.source.line_column(node.start_byte()).1 - 1;
    let column = match parenthesized.is_some() {
        true => column.saturating_sub(1),
        false => column,
    };
    (" ".repeat(column + width), " ".repeat(column))
}

/// `heredoc_end`: the end of the terminator of the last heredoc the operation opened as an
/// argument.
fn heredoc_end(context: &RuleContext<'_>, operation: Node<'_>) -> Option<usize> {
    if operation.kind_str() != "call" {
        return None;
    }
    let beginning = send_node::arguments(operation)
        .iter()
        .rev()
        .map(send_node::Argument::first)
        .find(|argument| argument.kind_str() == "heredoc_beginning")?;
    let body = send_node::heredoc_body(beginning, context)?;
    super::nodes::children(body)
        .into_iter()
        .find(|child| child.kind_str() == "heredoc_end")
        .map(|terminator| terminator.end_byte())
}
