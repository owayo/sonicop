use tree_sitter::Node;

use super::support::{Parameter, ParameterKind, parameters};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG_READER: &str = "Do not prefix reader method names with `get_`.";
const MSG_WRITER: &str = "Do not prefix writer method names with `set_`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name_node) = node.child_by_field_name("name") else {
            continue;
        };
        let name = context.source.node_text(name_node);
        // A method whose name already ends in `!`, `?` or `=` is not the accessor this cop is
        // about. A `setter` node carries the `=`, so the text alone answers all three.
        if name.ends_with(['!', '?', '=']) {
            continue;
        }
        let arguments = arguments(node);
        let message = if name.starts_with("get_") && arguments.is_empty() {
            MSG_READER
        } else if name.starts_with("set_")
            && arguments.len() == 1
            && arguments[0].kind == ParameterKind::Arg
        {
            MSG_WRITER
        } else {
            continue;
        };
        offenses.push(context.offense(message, name_node.byte_range()));
    }
}

fn arguments<'tree>(node: Node<'tree>) -> Vec<Parameter<'tree>> {
    node.child_by_field_name("parameters")
        .map(parameters)
        .unwrap_or_default()
}
