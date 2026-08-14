use tree_sitter::Node;

use super::support::{HeredocEnds, LengthTarget, constructor_call, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(100);
    let heredocs = HeredocEnds::new(context);
    for node in context.nodes_of_any(&["module", "assignment"]) {
        let (measured, target) = if node.kind_str() == "module" {
            (node, LengthTarget::Classlike)
        } else {
            // `CONST = Module.new { ... }` defines a module, so `on_casgn` measures the block's
            // body here. The offense goes on the constant, not on the block: RuboCop hands
            // `check_code_length` the assignment itself.
            match module_definition_block(context, node) {
                Some((block, name)) => (
                    block,
                    LengthTarget::ConstantAssignment {
                        assignment: node,
                        name,
                    },
                ),
                None => continue,
            }
        };
        report_length(
            context, offenses, measured, max, "Module", target, &heredocs,
        );
    }
}

/// The block of a `Module.new` assigned to a constant, with the constant it is assigned to.
///
/// The pattern is stricter than `Metrics/ClassLength`'s: the constant must be unqualified, so
/// `A::B = Module.new { ... }` is not a match, and `Module.new` must take no arguments.
fn module_definition_block<'tree>(
    context: &RuleContext<'_>,
    assignment: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    let name = assignment
        .field("left")
        .filter(|left| left.kind_str() == "constant")?;
    let call = assignment
        .field("right")
        .filter(|right| right.kind_str() == "call")?;
    if call.field("arguments").is_some()
        || constructor_call(context, call)? != ("Module", "new")
    {
        return None;
    }
    let block = call
        .field("block")
        .filter(|block| matches!(block.kind_str(), "block" | "do_block"))?;
    Some((block, name))
}
