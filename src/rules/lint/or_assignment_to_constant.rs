use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("operator_assignment") {
        let (Some(left), Some(operator)) = (node.field("left"), node.child(1)) else {
            continue;
        };
        if context.source.node_text(operator) != "||=" || !is_constant_target(left) {
            continue;
        }
        let range = operator.byte_range();
        let offense = context.offense("Avoid using or-assignment with constants.", range.clone());
        // A constant assigned inside a method body is re-run on every call, so making the write
        // unconditional would change what the program does rather than tidy it.
        offenses.push(if in_method(node, context) {
            offense
        } else {
            offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: "=".to_owned(),
                safe: true,
            })
        });
    }
}

/// `casgn_type?`: the two shapes a constant assignment target takes, `NAME` and `Scope::NAME`.
/// `Foo::bar` reads as a method call and `Foo[:a]` as an index, so both are left alone.
fn is_constant_target(left: Node<'_>) -> bool {
    match left.kind_str() {
        "constant" => true,
        "scope_resolution" => left
            .field("name")
            .is_some_and(|name| name.kind_str() == "constant"),
        _ => false,
    }
}

/// `each_ancestor(:any_def).any?`.
fn in_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "method" | "singleton_method") {
            return true;
        }
        current = ancestor.parent_of(context);
    }
    false
}
