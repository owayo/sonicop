//! `Layout/ArrayAlignment`.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    alignment_corrections, begins_its_line, display_column, holds_block_comment, line_indentation,
    string_interiors,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const ALIGN_ELEMENTS_MSG: &str =
    "Align the elements of an array literal if they span more than one line.";
const FIXED_INDENT_MSG: &str =
    "Use one level of indentation for elements following the first line of a multi-line array.";

/// Every shape upstream's parser calls an `array`: the bracketed and percent literals, the
/// brace-less list of `a = 1, 2`, and the exception list of a `rescue`, which a `resbody` carries
/// as an `array` even though it is written without brackets. A `return 1, 2` is not one -- its
/// values stay children of the keyword.
const ARRAY_KINDS: [&str; 5] = [
    "array",
    "string_array",
    "symbol_array",
    "right_assignment_list",
    "exceptions",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let fixed = context
        .setting::<String>("EnforcedStyle")
        .as_deref()
        .unwrap_or("with_first_element")
        == "with_fixed_indentation";
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let message = if fixed {
        FIXED_INDENT_MSG
    } else {
        ALIGN_ELEMENTS_MSG
    };

    let mut reported: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&ARRAY_KINDS) {
        inspect(
            context,
            node,
            fixed,
            width,
            message,
            &mut reported,
            offenses,
        );
    }
}

fn inspect(
    context: &RuleContext<'_>,
    node: Node<'_>,
    fixed: bool,
    width: i64,
    message: &str,
    reported: &mut Vec<Range<usize>>,
    offenses: &mut Vec<Offense>,
) {
    let elements = elements(node);
    if elements.len() < 2 || is_multiple_assignment_value(node) {
        return;
    }
    let base = if fixed {
        // `target_method_lineno`: a bracketed literal counts from its own line, a brace-less one
        // from the line of whatever it belongs to.
        let anchor = if bracketed(node) {
            node
        } else {
            node.parent_of(context).unwrap_or(node)
        };
        line_indentation(context, anchor.start_byte()) + width
    } else {
        display_column(context, elements[0].start)
    };

    let mut previous_line = 0usize;
    for element in &elements {
        let line = context.source.line_column(element.start).0;
        if line > previous_line && begins_its_line(context, element.start) {
            let delta = base - display_column(context, element.start);
            if delta != 0 {
                report(context, element, delta, message, reported, offenses);
            }
        }
        previous_line = line;
    }
}

/// `node.children`, where a run of `key: value` pairs is a single `hash` value upstream.
fn elements(node: Node<'_>) -> Vec<Range<usize>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect();
    let mut elements = Vec::new();
    let mut index = 0;
    while index < children.len() {
        if matches!(children[index].kind_str(), "pair" | "hash_splat_argument") {
            let start = index;
            while index < children.len()
                && matches!(children[index].kind_str(), "pair" | "hash_splat_argument")
            {
                index += 1;
            }
            elements.push(children[start].start_byte()..children[index - 1].end_byte());
        } else {
            elements.push(children[index].byte_range());
            index += 1;
        }
    }
    elements
}

fn bracketed(node: Node<'_>) -> bool {
    node.child(0)
        .is_some_and(|child| matches!(child.kind_str(), "[" | "%w(" | "%i("))
}

/// `node.parent&.masgn_type?`: the right-hand side of `a, b = 1, 2`.
fn is_multiple_assignment_value(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind_str() == "assignment"
            && parent
                .field("left")
                .is_some_and(|left| left.kind_str() == "left_assignment_list")
    })
}

fn report(
    context: &RuleContext<'_>,
    element: &Range<usize>,
    delta: i64,
    message: &str,
    reported: &mut Vec<Range<usize>>,
    offenses: &mut Vec<Offense>,
) {
    let nested = reported
        .iter()
        .any(|outer| element.start >= outer.start && element.end <= outer.end);
    let mut offense = context.offense(message, element.clone());
    if !nested && !holds_block_comment(context, element) {
        let taboo = string_interiors(context, element);
        offense = offense.corrected_by_all(alignment_corrections(
            context,
            element.clone(),
            delta,
            &taboo,
        ));
    }
    reported.push(element.clone());
    offenses.push(offense);
}
