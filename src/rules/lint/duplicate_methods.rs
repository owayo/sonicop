use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::{RuleContext, push_named_children};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    inspect_method_scope(context.root_node(), context, offenses);
    for node in context.nodes_of_any(&["class", "module", "singleton_class"]) {
        inspect_method_scope(node, context, offenses);
    }
}

fn inspect_method_scope(scope: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut methods: HashMap<(bool, String), usize> = HashMap::new();
    collect_scope_methods(scope, scope, &mut |method| {
        if inside_ignored_method_context(method, scope) {
            return;
        }
        let Some(name) = method.child_by_field_name("name") else {
            return;
        };
        let singleton = method.kind() == "singleton_method";
        let key = (singleton, context.source.node_text(name).to_owned());
        if let Some(first_line) = methods.insert(key.clone(), name.start_position().row + 1) {
            offenses.push(context.offense(
                format!(
                    "Method `{}` is defined at both line {first_line} and line {}.",
                    key.1,
                    name.start_position().row + 1
                ),
                name.byte_range(),
            ));
        }
    });
}

fn inside_ignored_method_context(mut node: Node<'_>, scope: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if parent == scope {
            return false;
        }
        if matches!(
            parent.kind(),
            "block" | "do_block" | "if" | "unless" | "if_modifier" | "unless_modifier" | "rescue"
        ) {
            return true;
        }
        node = parent;
    }
    false
}

fn collect_scope_methods<'tree>(
    node: Node<'tree>,
    root: Node<'tree>,
    callback: &mut impl FnMut(Node<'tree>),
) {
    let mut stack = Vec::new();
    push_named_children(node, &mut stack);
    while let Some(current) = stack.pop() {
        if current != root && matches!(current.kind(), "class" | "module" | "singleton_class") {
            continue;
        }
        if matches!(current.kind(), "method" | "singleton_method") {
            callback(current);
            continue;
        }
        push_named_children(current, &mut stack);
    }
}
