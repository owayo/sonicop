//! `Layout/SpaceBeforeFirstArg`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::alignment::Alignment;
use super::support::{final_pos, grouped_arguments};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Put one space between the method name and the first argument.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_for_alignment: bool = context.setting("AllowForAlignment").unwrap_or(true);
    let text = context.source.text();
    let mut alignment = None;

    for node in context.nodes_of("call") {
        // `regular_method_call_with_arguments?`: an operator or a setter is written the other way
        // round, and `super` is its own node upstream rather than a send.
        let Some(selector) = node
            .field("method")
            .filter(|method| method.kind_str() != "super")
            .filter(|method| is_name(context.source.node_text(*method)))
        else {
            continue;
        };
        // `node.parenthesized?` is `loc.begin&.is?('(')`, which only a parenthesis written right
        // against the selector satisfies: `foo ("x")` is a command call with a grouped argument.
        if text[selector.end_byte()..].starts_with('(') {
            continue;
        }
        let Some(argument) = grouped_arguments(node).into_iter().next() else {
            continue;
        };
        let first = argument.range.clone();
        // `foo&blk` written without a space is the binary operator upstream's parser builds, not a
        // call taking a block: only a blank in front of the `&` makes it an argument.
        if first.start == selector.end_byte()
            && argument.parts.first().is_some_and(|part| {
                matches!(
                    part.kind_str(),
                    "block_argument" | "splat_argument" | "hash_splat_argument"
                )
            })
        {
            continue;
        }

        let space = final_pos(text, first.start, false, true, false)..first.start;
        if text[space.clone()].chars().count() == 1 {
            continue;
        }
        if !expects_space(
            context,
            node,
            selector,
            &first,
            allow_for_alignment,
            &mut alignment,
        ) {
            continue;
        }
        offenses.push(context.offense(MSG, space.clone()).corrected_by(Edit {
            start: space.start,
            end: space.end,
            replacement: " ".to_owned(),
            safe: true,
        }));
    }
}

/// `expect_params_after_method_name?`
fn expects_space<'src>(
    context: &RuleContext<'src>,
    node: Node<'_>,
    selector: Node<'_>,
    first: &Range<usize>,
    allow_for_alignment: bool,
    alignment: &mut Option<Alignment<'src>>,
) -> bool {
    // `no_space_between_method_name_and_first_argument?`
    if selector.end_byte() == first.start {
        return true;
    }
    if context.source.line_column(node.start_byte()).0 != context.source.line_column(first.start).0
    {
        return false;
    }
    if !allow_for_alignment {
        return true;
    }
    let alignment = alignment.get_or_insert_with(|| Alignment::new(context));
    !alignment.aligned_with_something(first)
}

/// A method name rather than an operator, which is what `operator_method?` and `setter_method?`
/// between them rule out.
fn is_name(method: &str) -> bool {
    method.starts_with(|character: char| character.is_alphabetic() || character == '_')
        && !method.ends_with('=')
}
