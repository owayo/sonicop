use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// The constructors `class_constructor?` accepts, which is what `IgnoreModules` exempts.
const CONSTRUCTORS: [(&str, &str); 4] = [
    ("Class", "new"),
    ("Module", "new"),
    ("Struct", "new"),
    ("Data", "define"),
];

/// `on_casgn`: a constant assigned in a class or module body, with no `public_constant` or
/// `private_constant` beside it saying which it is.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_modules = context.setting::<bool>("IgnoreModules").unwrap_or(false);
    for node in context.nodes_of("assignment") {
        let Some(target) = node.field("left") else {
            continue;
        };
        let Some(name) = constant_name(target, context) else {
            continue;
        };
        let Some(statements) = class_or_module_statements(node) else {
            continue;
        };
        if declares_visibility(statements, name, context) {
            continue;
        }
        if ignore_modules
            && node
                .field("right")
                .is_some_and(|value| is_class_constructor(value, context))
        {
            continue;
        }
        offenses.push(context.offense(
            format!(
                "Explicitly make `{name}` public or private using either `#public_constant` or \
                 `#private_constant`."
            ),
            node.byte_range(),
        ));
    }
}

/// `node.name` of a `casgn`: the last segment, so `Foo::BAR` is named `BAR`.
fn constant_name<'a>(target: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match target.kind_str() {
        "constant" => Some(context.source.node_text(target)),
        "scope_resolution" => Some(context.source.node_text(target.field("name")?)),
        _ => None,
    }
}

/// `class_or_module_scope?`, and the statement list the assignment shares with its siblings.
///
/// Upstream steps through `begin` nodes on the way up; the same list is a `body_statement` here, and
/// a class holding a single statement has no list at all.
fn class_or_module_statements<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node;
    loop {
        let parent = current.parent()?;
        match parent.kind_str() {
            "class" | "module" => return Some(current),
            "body_statement" => current = parent,
            _ => return None,
        }
    }
}

/// `visibility_declaration?`: a `public_constant` or `private_constant` among the siblings that
/// names this constant.
fn declares_visibility(statements: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    let siblings = if statements.kind_str() == "body_statement" {
        super::nodes::children(statements)
    } else {
        // The class holds one statement, which is the assignment itself.
        Vec::new()
    };
    siblings.iter().any(|sibling| {
        if sibling.kind_str() != "call" || sibling.field("receiver").is_some() {
            return false;
        }
        if sibling.field("method").is_none_or(|selector| {
            !matches!(
                context.source.node_text(selector),
                "public_constant" | "private_constant"
            )
        }) {
            return false;
        }
        let arguments = sibling
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        // `arguments.first.children.first.to_a if arguments.first&.splat_type?`: a splatted array
        // literal is unwrapped, and only then.
        let written = match arguments.first() {
            Some(first) if first.kind_str() == "splat_argument" => {
                match super::nodes::children(*first).first() {
                    Some(inner) if inner.kind_str() == "array" => super::nodes::children(*inner),
                    _ => Vec::new(),
                }
            }
            _ => arguments,
        };
        written
            .iter()
            .any(|argument| names(*argument, context) == Some(name))
    })
}

/// `argument.value.to_sym if argument.type?(:sym, :str)`.
fn names<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    if let Some(name) = send_node::symbol_name(node, context) {
        return Some(name);
    }
    send_node::is_string(node, context).then(|| send_node::string_text(node, context))
}

/// `class_constructor?`.
fn is_class_constructor(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method")) else {
        return false;
    };
    let method = context.source.node_text(selector);
    CONSTRUCTORS.iter().any(|(constant, wanted)| {
        method == *wanted && send_node::top_level_constant(receiver, constant, context)
    })
}
