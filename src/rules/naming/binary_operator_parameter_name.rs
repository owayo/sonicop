use tree_sitter::Node;

use super::support::{ParameterKind, Variables, parameters};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `EXCLUDED`: operators whose sole parameter is not the other operand. Indexing takes a key,
/// `<<` takes an element, and the unary forms take nothing meaningful at all.
const EXCLUDED: &[&str] = &["+@", "-@", "[]", "[]=", "<<", "===", "`", "=~"];

/// `OP_LIKE_METHODS`: word-spelled methods that still compare two operands.
const OPERATOR_LIKE: &[&str] = &["eql?", "equal?"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut variables = None;
    // Only `def` is handled upstream; `def self.+(x)` has no `on_defs` alias to reach it.
    for node in context.nodes_of("method") {
        let Some(name_node) = node.field("name") else {
            continue;
        };
        let name = context.source.node_text(name_node);
        if !operator_method(name) {
            continue;
        }
        let Some(list) = node.field("parameters") else {
            continue;
        };
        let arguments = parameters(list);
        let [argument] = arguments.as_slice() else {
            continue;
        };
        if argument.kind != ParameterKind::Arg {
            continue;
        }
        let parameter = argument.node;
        let text = context.source.node_text(parameter);
        if text == "other" || text == "_other" {
            continue;
        }
        let variables = variables.get_or_insert_with(|| context.variable_roles());
        offenses.push(
            context
                .offense(
                    format!("When defining the `{name}` operator, name its argument `other`."),
                    parameter.byte_range(),
                )
                .corrected_by_all(rename(context, variables, node, parameter)),
        );
    }
}

/// The replacements upstream makes, one edit each: the parameter and every later read or
/// assignment of it become `other`.
///
/// One edit spanning from the parameter to the last of them would hand back the text in between
/// unchanged, which reproduces the same output only while nothing else corrects inside it. That
/// span is the whole method body, so any other cop rewriting a line of it clobbers against this
/// one and is put off to the next pass.
fn rename(
    context: &RuleContext<'_>,
    variables: &Variables,
    definition: Node<'_>,
    parameter: Node<'_>,
) -> Vec<Edit> {
    let name = context.source.node_text(parameter);
    let mut sites = vec![parameter.byte_range()];
    for node in context.nodes_of("identifier") {
        if node.start_byte() <= parameter.start_byte()
            || node.end_byte() > definition.end_byte()
            || context.source.node_text(node) != name
            || !variables.is_variable(node)
        {
            continue;
        }
        sites.push(node.byte_range());
    }
    let safe = context.setting("Safe").unwrap_or(true);
    sites
        .into_iter()
        .map(|site| Edit {
            start: site.start,
            end: site.end,
            replacement: "other".to_owned(),
            safe,
        })
        .collect()
}

fn operator_method(name: &str) -> bool {
    if EXCLUDED.contains(&name) {
        return false;
    }
    !name
        .chars()
        .next()
        .is_some_and(|first| first.is_alphanumeric() || first == '_')
        || OPERATOR_LIKE.contains(&name)
}
