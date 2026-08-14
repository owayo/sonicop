use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::parameters::defaulted;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for parameter in context.nodes_of_any(&["optional_parameter", "keyword_parameter"]) {
        let arguments = match parameter.kind_str() {
            "optional_parameter" => defaulted(parameter),
            _ => super::parameters::keyword(parameter).into_iter().collect(),
        };
        for argument in arguments {
            let name = context.source.node_text(argument.name);
            // The name is already in scope while its own default is parsed, so a bare mention of
            // it reads the argument being defined rather than a method of the same name.
            if argument.value.kind_str() == "identifier"
                && context.source.node_text(argument.value) == name
            {
                offenses.push(offense(name, argument.value, context));
                continue;
            }
            if let Some(circular) = assignment_chain(argument.value, name, context) {
                offenses.push(offense(name, circular, context));
            }
        }
    }
}

fn offense(name: &str, node: Node<'_>, context: &RuleContext<'_>) -> Offense {
    context.offense(
        format!("Circular argument reference - `{name}`."),
        node.byte_range(),
    )
}

/// `check_assignment_chain`: an assignment whose innermost value reads a name assigned along the
/// way, or the argument itself.
fn assignment_chain<'tree>(
    value: Node<'tree>,
    name: &str,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if !is_local_assignment(value, context) {
        return None;
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut current = value;
    while is_local_assignment(current, context) {
        let left = current.field("left")?;
        seen.push(context.source.node_text(left));
        current = current.field("right")?;
    }
    if current.kind_str() != "identifier" {
        return None;
    }
    let read = context.source.node_text(current);
    (seen.contains(&read) || read == name).then_some(current)
}

/// `node.lvasgn_type?`: an assignment to a bare name, which is the only shape the chain walks.
fn is_local_assignment(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| left.kind_str() == "identifier")
        && node
            .child(1)
            .is_some_and(|operator| context.source.node_text(operator) == "=")
}
