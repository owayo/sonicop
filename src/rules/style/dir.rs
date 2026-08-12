use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Use `__dir__` to get an absolute path to the current file's directory.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some((outer, argument)) = file_call(context, node) else {
            continue;
        };
        let inner_name = match outer {
            "expand_path" => "dirname",
            "dirname" => "realpath",
            _ => continue,
        };
        let Some((inner, file)) = file_call(context, argument) else {
            continue;
        };
        // `file_keyword?`: the parser gives `__FILE__` a `str` node holding the path, so the cop
        // compares the source rather than the value.
        if inner != inner_name || context.source.node_text(file) != "__FILE__" {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: "__dir__".to_owned(),
            safe: true,
        }));
    }
}

/// `(send (const {nil? cbase} :File) $_ $_)`: the method name and the single argument of a call on
/// `File`, however the constant was qualified.
fn file_call<'a, 'tree>(
    context: &'a RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(&'a str, Node<'tree>)> {
    if node.kind() != "call" || node.child_by_field_name("block").is_some() {
        return None;
    }
    let receiver = node.child_by_field_name("receiver")?;
    let named = match receiver.kind() {
        "constant" => receiver,
        // `::File` is a `scope_resolution` with no scope of its own.
        "scope_resolution" if receiver.child_by_field_name("scope").is_none() => {
            receiver.child_by_field_name("name")?
        }
        _ => return None,
    };
    if context.source.node_text(named) != "File" {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    let arguments = node.child_by_field_name("arguments")?;
    match super::nodes::children(arguments).as_slice() {
        [only] => Some((context.source.node_text(method), *only)),
        _ => None,
    }
}
