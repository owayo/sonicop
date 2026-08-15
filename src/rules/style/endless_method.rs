//! `Style/EndlessMethod`: how much of a definition an endless one may hold.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Avoid endless method definitions.";
const MSG_MULTI_LINE: &str = "Avoid endless method definitions with multiple lines.";
const MSG_REQUIRE_SINGLE: &str = "Use endless method definitions for single line methods.";
const MSG_REQUIRE_ALWAYS: &str = "Use endless method definitions.";

/// `minimum_target_ruby_version 3.0`: endless methods arrived in 3.0.
const MINIMUM: RubyVersion = RubyVersion::new(3, 0);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "allow_single_line".to_owned());
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        // A setter cannot be written endlessly, and a heredoc body would leave its text stranded.
        if is_assignment_method(node, context) || uses_heredoc(node, context) {
            continue;
        }
        let endless = super::endless::is_endless(node);
        let single_line = node.start_position().row == node.end_position().row;
        match style.as_str() {
            "allow_single_line" if endless && !single_line => {
                push_multiline(context, node, MSG_MULTI_LINE, offenses);
            }
            "disallow" if endless => push_multiline(context, node, MSG, offenses),
            "require_single_line" => {
                if endless && !single_line {
                    push_multiline(context, node, MSG_MULTI_LINE, offenses);
                } else if !endless && body_fits_on_one_line(node) {
                    push_endless(context, node, MSG_REQUIRE_SINGLE, offenses);
                }
            }
            "require_always" if !endless => {
                push_endless(context, node, MSG_REQUIRE_ALWAYS, offenses);
            }
            _ => {}
        }
    }
}

/// `add_offense(node) { correct_to_multiline }`.
fn push_multiline(
    context: &RuleContext<'_>,
    node: Node<'_>,
    message: &str,
    offenses: &mut Vec<Offense>,
) {
    let offense = context.offense(message, node.byte_range());
    offenses.push(match super::endless::correct_to_multiline(context, node) {
        Some(replacement) => offense.corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement,
            safe: true,
        }),
        None => offense,
    });
}

/// `add_offense(node) { corrector.replace(node, endless_replacement(node)) }`.
fn push_endless(
    context: &RuleContext<'_>,
    node: Node<'_>,
    message: &str,
    offenses: &mut Vec<Offense>,
) {
    let Some(replacement) = endless_replacement(context, node) else {
        return;
    };
    if too_long_when_made_endless(context, node, &replacement) {
        return;
    }
    offenses.push(
        context
            .offense(message, node.byte_range())
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }),
    );
}

/// `can_be_made_endless?` together with `node.body.single_line?`.
fn body_fits_on_one_line(node: Node<'_>) -> bool {
    single_statement(node).is_some_and(|body| body.start_position().row == body.end_position().row)
}

/// `endless_replacement`.
fn endless_replacement(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let body = single_statement(node)?;
    Some(format!(
        "def {}{}{} = {}",
        super::endless::receiver(context, node),
        context.source.node_text(node.field("name")?),
        super::endless::arguments(context, node),
        context.source.node_text(body),
    ))
}

/// `can_be_made_endless?`: one statement, and not a `begin ... end` holding several.
fn single_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let body = node.field("body")?;
    if body.kind_str() != "body_statement" {
        return None;
    }
    match super::nodes::children(body).as_slice() {
        [only] if !matches!(only.kind_str(), "begin" | "rescue" | "ensure" | "else") => Some(*only),
        _ => None,
    }
}

/// `too_long_when_made_endless?`.
///
/// Upstream measures the replacement alone -- the definition's own indentation is not counted --
/// plus the column a modifier written on the same line would push it out by.
fn too_long_when_made_endless(
    context: &RuleContext<'_>,
    node: Node<'_>,
    replacement: &str,
) -> bool {
    if !context.cop_enabled("Layout/LineLength") {
        return false;
    }
    let max = context
        .setting_of::<i64>("Layout/LineLength", "Max")
        .unwrap_or(120)
        .max(0) as usize;
    let offset = context.parent(node).map_or(0, |parent| {
        if parent.start_position().row == node.start_position().row {
            node.start_position()
                .column
                .saturating_sub(parent.start_position().column)
        } else {
            0
        }
    });
    replacement.chars().count() + offset > max
}

/// `node.assignment_method?`: a setter, whose name the grammar writes as a `setter` node.
fn is_assignment_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("name").is_some_and(|name| {
        name.kind_str() == "setter" || context.source.node_text(name).ends_with('=')
    })
}

/// `use_heredoc?`: a heredoc anywhere in the body leaves its text on the lines below.
fn uses_heredoc(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(body) = node.field("body") else {
        return false;
    };
    context.nodes_of("heredoc_beginning").any(|heredoc| {
        body.start_byte() <= heredoc.start_byte() && heredoc.end_byte() <= body.end_byte()
    })
}
