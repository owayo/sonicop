use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::line_length_help::LineLengthHelp;

const MSG_COMPACT: &str = "Put empty method definitions on a single line.";
const MSG_EXPANDED: &str = "Put the `end` of empty method definitions on the next line.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let compact = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "compact");
    let mut line_length: Option<LineLengthHelp<'_, '_>> = None;

    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        // `node.body`: a definition with anything in it is not empty, and a comment inside the
        // range keeps the shape the author chose.
        if node.child_by_field_name("body").is_some() || contains_comment(context, node) {
            continue;
        }
        let single_line = node.start_position().row == node.end_position().row;
        if compact == single_line {
            continue;
        }

        let message = match compact {
            true => MSG_COMPACT,
            false => MSG_EXPANDED,
        };
        let offense = context.offense(message, node.byte_range());
        let correction = corrected(context, node, compact);
        // The rewritten one-liner is only worth making when it fits; the offense stands either way.
        let max = line_length
            .get_or_insert_with(|| LineLengthHelp::new(context))
            .max();
        let too_long = compact && max.is_some_and(|max| correction.chars().count() > max);
        offenses.push(match too_long {
            true => offense,
            false => offense.corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: correction,
                safe: true,
            }),
        });
    }
}

/// `contains_comment?`, which asks by *line* rather than by range: a comment trailing the `def` or
/// the `end` sits outside the node yet still keeps the definition from being rewritten.
fn contains_comment(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let lines = node.start_position().row..=node.end_position().row;
    context.comment_ranges().iter().any(|comment| {
        lines.contains(
            &context
                .source
                .line_column(comment.start)
                .0
                .saturating_sub(1),
        )
    })
}

/// `corrected`: the definition written back with the `end` where the style wants it.
fn corrected(context: &RuleContext<'_>, node: Node<'_>, compact: bool) -> String {
    let scope = node
        .child_by_field_name("object")
        .map(|receiver| format!("{}.", context.source.node_text(receiver)))
        .unwrap_or_default();
    let name = node
        .child_by_field_name("name")
        .map_or(String::new(), |name| {
            context.source.node_text(name).to_owned()
        });
    let arguments = parameters(context, node);
    let joint = match compact {
        true => "; ".to_owned(),
        false => format!("\n{}", " ".repeat(node.start_position().column)),
    };
    format!("def {scope}{name}{arguments}{joint}end")
}

/// `node.arguments` written back: an empty parameter list is no list at all upstream, so the
/// parentheses that held nothing are dropped.
fn parameters(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let Some(list) = node.child_by_field_name("parameters") else {
        return String::new();
    };
    let sources: Vec<&str> = super::nodes::children(list)
        .into_iter()
        .map(|parameter| context.source.node_text(parameter))
        .collect();
    if sources.is_empty() {
        return String::new();
    }
    let joined = sources.join(", ");
    match context.source.node_text(list).starts_with('(') {
        true => format!("({joined})"),
        false => format!(" {joined}"),
    }
}
