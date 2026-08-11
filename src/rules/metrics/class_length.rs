use tree_sitter::Node;

use super::support::{HeredocEnds, LengthTarget, report_length};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(100);
    let heredocs = HeredocEnds::new(context);
    for node in context.nodes_of_any(&["class", "singleton_class"]) {
        // A `class << self` inside a class body is part of that class's length rather than a
        // class of its own.
        if node.kind() == "singleton_class" && has_class_ancestor(node) {
            continue;
        }
        report_length(
            context,
            offenses,
            node,
            max,
            "Class",
            LengthTarget::Classlike,
            &heredocs,
        );
    }
}

fn has_class_ancestor(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if parent.kind() == "class" {
            return true;
        }
        node = parent;
    }
    false
}
