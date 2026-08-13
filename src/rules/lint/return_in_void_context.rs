use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::named_children;
use crate::rules::node_ext::NodeExt;

/// `SCOPE_CHANGING_METHODS`: a block handed to one of these gives the `return` a scope of its own.
const SCOPE_CHANGING_METHODS: [&str; 3] = ["lambda", "define_method", "define_singleton_method"];

/// `COMPARISON_OPERATORS`, which `assignment_method?` refuses however they end.
const COMPARISON_METHODS: [&str; 5] = ["==", "===", "!=", "<=", ">="];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("return") {
        // `return_node.descendants.any?`: a bare `return` hands back nothing to discard.
        if named_children(node).is_empty() {
            continue;
        }
        let Some(keyword) = node.child(0).filter(|child| child.kind_str() == "return") else {
            continue;
        };
        let Some(method) = enclosing_void_method(node, context) else {
            continue;
        };
        if in_scope_changing_block(node, context) {
            continue;
        }
        offenses.push(context.offense(
            format!("Do not return a value in `{method}`."),
            keyword.byte_range(),
        ));
    }
}

/// The name of the nearest enclosing definition, when `void_context?` holds for it: a constructor
/// or a setter answers with its argument whatever the body says.
fn enclosing_void_method<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if matches!(current.kind_str(), "method" | "singleton_method") {
            let name = context
                .source
                .node_text(current.field("name")?);
            let void = name == "initialize"
                || (name.ends_with('=') && !COMPARISON_METHODS.contains(&name));
            return void.then_some(name);
        }
        ancestor = current.parent();
    }
    None
}

/// `each_ancestor(:any_block).any?`, which upstream runs over every ancestor rather than stopping
/// at the definition the `return` belongs to.
fn in_scope_changing_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        ancestor = current.parent();
        if !matches!(current.kind_str(), "block" | "do_block") {
            continue;
        }
        // A `->() {}` reaches upstream as a receiverless `lambda` call.
        if current
            .parent()
            .is_some_and(|parent| parent.kind_str() == "lambda")
        {
            return true;
        }
        let method = current
            .parent()
            .filter(|parent| parent.kind_str() == "call")
            .and_then(|call| call.field("method"))
            .map(|method| context.source.node_text(method));
        if method.is_some_and(|method| SCOPE_CHANGING_METHODS.contains(&method)) {
            return true;
        }
    }
    false
}
