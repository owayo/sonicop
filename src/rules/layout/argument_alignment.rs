//! `Layout/ArgumentAlignment`.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{AlignmentPass, GroupedArgument, display_column, grouped_arguments};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const ALIGN_PARAMS_MSG: &str =
    "Align the arguments of a method call if they span more than one line.";
const FIXED_INDENT_MSG: &str = "Use one level of indentation for arguments following the first \
                                line of a multi-line method call.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "with_first_argument".to_owned());
    let fixed = style == "with_fixed_indentation";
    // With the neighbouring cop aligning on separators there is no column both cops can agree on,
    // so this one stands down.
    if !fixed && hash_alignment_uses_separators(context) {
        return;
    }
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let message = if fixed {
        FIXED_INDENT_MSG
    } else {
        ALIGN_PARAMS_MSG
    };

    let mut pass = AlignmentPass::new();
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        if is_super(node) || is_index_assignment(context, node) {
            continue;
        }
        let arguments = grouped_arguments(node);
        if !multiple_arguments(&arguments) {
            continue;
        }
        let items = flattened_arguments(&arguments, fixed);
        if items.is_empty() {
            continue;
        }
        let base = base_column(context, node, &items, fixed, width);
        for (item, delta) in AlignmentPass::misaligned(context, &items, base) {
            pass.register(
                context,
                item.clone(),
                item,
                delta,
                |_| message.to_owned(),
                offenses,
            );
        }
    }
}

fn hash_alignment_uses_separators(context: &RuleContext<'_>) -> bool {
    ["EnforcedColonStyle", "EnforcedHashRocketStyle"]
        .into_iter()
        .any(|key| {
            let single = context.setting_of::<String>("Layout/HashAlignment", key);
            let many = context.setting_of::<Vec<String>>("Layout/HashAlignment", key);
            single.as_deref() == Some("separator")
                || many.is_some_and(|styles| styles.iter().any(|style| style == "separator"))
        })
}

/// `multiple_arguments?`.
fn multiple_arguments(arguments: &[GroupedArgument<'_>]) -> bool {
    if arguments.len() >= 2 {
        return true;
    }
    arguments
        .first()
        .is_some_and(|first| first.hash_run && first.parts.len() >= 2)
}

/// `super` is a node of its own upstream, which `on_send` never sees.
fn is_super(node: Node<'_>) -> bool {
    node.child(0).is_some_and(|child| child.kind_str() == "super")
}

/// `node.method?(:[]=)`: assigning through an index reaches the cop as one call whose arguments
/// mix the subscript with the value, which upstream leaves alone.
fn is_index_assignment(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    if call
        .field("method")
        .is_some_and(|method| &context.source.text()[method.byte_range()] == "[]=")
    {
        return true;
    }
    call.kind_str() == "element_reference"
        && call.parent_of(context).is_some_and(|parent| {
            parent.kind_str() == "assignment" && parent.field("left") == Some(call)
        })
}

/// `flattened_arguments`: a brace-less hash is looked at pair by pair, at whichever end of the
/// argument list the style cares about.
fn flattened_arguments<'a, 'tree>(
    arguments: &'a [GroupedArgument<'tree>],
    fixed: bool,
) -> Vec<Range<usize>> {
    let candidate = if fixed {
        arguments.last()
    } else {
        arguments.first()
    };
    let Some(candidate) = candidate else {
        return Vec::new();
    };
    if !candidate.hash_run {
        return arguments
            .iter()
            .map(|argument| argument.range.clone())
            .collect();
    }
    let pairs: Vec<Range<usize>> = candidate
        .parts
        .iter()
        .map(tree_sitter::Node::byte_range)
        .collect();
    if fixed {
        let mut items: Vec<Range<usize>> = arguments[..arguments.len() - 1]
            .iter()
            .map(|argument| argument.range.clone())
            .collect();
        items.extend(pairs);
        items
    } else {
        pairs
    }
}

fn base_column(
    context: &RuleContext<'_>,
    call: Node<'_>,
    items: &[Range<usize>],
    fixed: bool,
    width: i64,
) -> i64 {
    if !fixed {
        return display_column(context, items[0].start);
    }
    // `target_method_lineno`: the selector's line, or the opening parenthesis for `l.(1)`.
    let anchor = call
        .field("method")
        .map_or_else(|| call.start_byte(), |method| method.start_byte());
    let line = context.source.line_column(anchor).0;
    let text = context.source.line(line);
    let indentation = text.len() - text.trim_start().len();
    indentation as i64 + width
}
