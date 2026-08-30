use std::collections::BTreeMap;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::blocks::BlockArgs;
use super::locals::LocalVariables;
use crate::rules::send_node::all_children_iter;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(methods) = context.setting::<BTreeMap<String, usize>>("Methods") else {
        return;
    };
    let locals = LocalVariables::new(context);
    for block in context.nodes_of_any(&["block", "do_block"]) {
        let Some(node) = block.parent_of(context) else {
            continue;
        };
        // `acceptable?`: only a call on something, since the arity of a receiverless call of the
        // same name is the author's own.
        let (Some(method), Some(_)) = (node.field("method"), node.field("receiver")) else {
            continue;
        };
        let name = context.source.node_text(method);
        let Some(&expected) = methods.get(name) else {
            continue;
        };
        let Some(actual) = arg_count(block, context, &locals) else {
            // A `restarg` takes as many as it is given.
            continue;
        };
        if actual >= expected {
            continue;
        }
        let message =
            format!("`{name}` expects at least {expected} positional arguments, got {actual}.");
        offenses.push(context.offense(message, node.byte_range()));
    }
}

/// `arg_count`: the positional parameters the block declares, or `None` for the splat that makes
/// the count infinite.
fn arg_count(
    block: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<usize> {
    let Some(parameters) = block.field("parameters") else {
        return Some(match BlockArgs::of(block, context, locals) {
            BlockArgs::Numbered(count) => count,
            BlockArgs::It => 1,
            BlockArgs::Written(_) => 0,
        });
    };
    let mut count = 0;
    let _cursor = parameters.walk();
    for parameter in all_children_iter(parameters, context) {
        // `{ |a; b| }`: what follows the `;` is a block-local variable. Upstream's parser never
        // puts it among the arguments; the grammar keeps it in the same node.
        if parameter.kind_str() == ";" {
            break;
        }
        match parameter.kind_str() {
            "splat_parameter" => return None,
            // `arg`, `optarg` and the `mlhs` a destructured parameter builds.
            "identifier" | "optional_parameter" | "destructured_parameter" => count += 1,
            _ => {}
        }
    }
    Some(count)
}
