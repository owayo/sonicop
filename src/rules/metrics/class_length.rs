use tree_sitter::Node;

use super::support::{HeredocEnds, LengthTarget, constructor_call, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(100);
    let heredocs = HeredocEnds::new(context);
    for node in context.nodes_of_any(&["class", "singleton_class", "assignment"]) {
        let (measured, target) = match node.kind_str() {
            // A `class << self` inside a class body is part of that class's length rather than a
            // class of its own. Only `class` and `module` are classlike to `CodeLengthCalculator`,
            // so a singleton class is measured over its body the way a method is.
            "singleton_class" if !has_class_ancestor(node) => (node, LengthTarget::Body),
            "class" => (node, LengthTarget::Classlike),
            // `CONST = Class.new { ... }` and `CONST = Struct.new(...) { ... }` are class
            // definitions written as expressions, which `Node#class_definition?` recognises and
            // `on_casgn` measures here rather than as blocks.
            "assignment" => match class_definition_block(context, node) {
                Some(block) => (block, LengthTarget::Block),
                None => continue,
            },
            _ => continue,
        };
        report_length(context, offenses, measured, max, "Class", target, &heredocs);
    }
}

fn has_class_ancestor(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if parent.kind_str() == "class" {
            return true;
        }
        node = parent;
    }
    false
}

/// The block of a `Class.new`/`Struct.new` assigned to a constant. Unlike `Metrics/ModuleLength`,
/// the pattern puts no condition on the constant, so a namespaced `A::B = Class.new { ... }`
/// counts too, and arguments such as a superclass or struct members are allowed.
fn class_definition_block<'tree>(
    context: &RuleContext<'_>,
    assignment: Node<'tree>,
) -> Option<Node<'tree>> {
    let target = assignment.field("left")?;
    if !matches!(target.kind_str(), "constant" | "scope_resolution") {
        return None;
    }
    let call = assignment
        .field("right")
        .filter(|right| right.kind_str() == "call")?;
    if !matches!(
        constructor_call(context, call)?,
        ("Class" | "Struct", "new")
    ) {
        return None;
    }
    call.field("block")
        .filter(|block| matches!(block.kind_str(), "block" | "do_block"))
}
