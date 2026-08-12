//! `Style/OptionalArguments`: an argument with a default belongs at the end of the list.

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Optional arguments should appear at the end of the argument list.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_def` and its `on_defs` alias: a block's parameters are never inspected.
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.child_by_field_name("parameters") else {
            continue;
        };
        let arguments = super::nodes::children(parameters);
        let optional: Vec<usize> = positions(&arguments, "optional_parameter");
        // `arg_type?`: only a plain argument counts. A destructuring `(a, b)` is an `mlhs`
        // upstream, and a keyword or splat argument is a type of its own.
        let plain: Vec<usize> = positions(&arguments, "identifier");
        let (Some(&last_plain), false) = (plain.iter().max(), optional.is_empty()) else {
            continue;
        };
        for position in optional {
            // There can only be one group of optional arguments, so the run ends here.
            if position > last_plain {
                break;
            }
            offenses.push(context.offense(MSG, arguments[position].byte_range()));
        }
    }
}

fn positions(arguments: &[Node<'_>], kind: &str) -> Vec<usize> {
    arguments
        .iter()
        .enumerate()
        .filter(|(_, argument)| argument.kind() == kind)
        .map(|(index, _)| index)
        .collect()
}
