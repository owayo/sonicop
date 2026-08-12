//! `Layout/FirstParameterIndentation`.

use tree_sitter::Node;

use super::support::{
    alignment_corrections, character_column, definition_parameters, holds_block_comment,
    line_indentation, string_interiors,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "consistent".to_owned());
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);

    for definition in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parenthesis) = left_parenthesis(definition) else {
            continue;
        };
        let parameters = definition_parameters(definition);
        let Some(first) = parameters.first() else {
            continue;
        };
        if context.source.line_column(first.start).0 == parenthesis.start_position().row + 1 {
            continue;
        }
        // `indent_base` can only land on the start of the line here: the brace style compares
        // against the parenthesis itself, the parent-hash-key branch needs the list to be a hash
        // value, and the `special_inside_parentheses` branch needs an enclosing call.
        let base = if style == "align_parentheses" {
            character_column(context, parenthesis.start_byte())
        } else {
            line_indentation(context, parenthesis.start_byte())
        };
        let delta = base + width - character_column(context, first.start);
        if delta == 0 {
            continue;
        }
        let message = format!(
            "Use {width} spaces for indentation in method args, relative to {}.",
            base_description(&style)
        );
        let mut offense = context.offense(message, first.clone());
        if !holds_block_comment(context, first) {
            let taboo = string_interiors(context, first);
            offense = offense.corrected_by_all(alignment_corrections(
                context,
                first.clone(),
                delta,
                &taboo,
            ));
        }
        offenses.push(offense);
    }
}

/// `def_node.arguments.loc.begin`: the parameter list's own parenthesis, which a definition written
/// without one does not have.
fn left_parenthesis<'tree>(definition: Node<'tree>) -> Option<Node<'tree>> {
    definition
        .child_by_field_name("parameters")?
        .child(0)
        .filter(|child| child.kind() == "(")
}

fn base_description(style: &str) -> &'static str {
    if style == "align_parentheses" {
        "the position of the opening parenthesis"
    } else {
        "the start of the line where the left parenthesis is"
    }
}
