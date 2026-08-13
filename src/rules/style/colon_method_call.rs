use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not use `::` for method calls.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(operator) = node.field("operator") else {
            continue;
        };
        if context.source.node_text(operator) != "::"
            || node.field("receiver").is_none()
        {
            continue;
        }
        let Some(method) = node.field("method") else {
            continue;
        };
        // `camel_case_method?`: `Foo::Bar()` reads as a constant lookup, not as a call.
        if context
            .source
            .node_text(method)
            .starts_with(|character: char| character.is_ascii_uppercase())
        {
            continue;
        }
        if java_interop(context, node) {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, operator.byte_range())
                .corrected_by(Edit {
                    start: operator.start_byte(),
                    end: operator.end_byte(),
                    replacement: ".".to_owned(),
                    safe: true,
                }),
        );
    }
}

/// `java_interop?`: the innermost receiver of the chain is the bare constant `Java`, which is how
/// JRuby spells `Java::int` and `Java::com::method`.
fn java_interop(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let mut receiver = match node.field("receiver") {
        Some(receiver) => receiver,
        None => return false,
    };
    while let Some(inner) = receiver.field("receiver") {
        receiver = inner;
    }
    receiver.kind_str() == "constant" && context.source.node_text(receiver) == "Java"
}
