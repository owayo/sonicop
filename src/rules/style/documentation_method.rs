use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

use super::comments::{AnnotationKeywords, PrecedingComments};
use super::documentation::documentation_comment;

const MSG: &str = "Missing method documentation comment.";

/// `modifier_node?`: a definition handed to one of these is documented through the call.
const MODIFIERS: [&str; 2] = ["module_function", "ruby2_keywords"];

/// `non_public_modifier?`: `private def foo` and friends.
const NON_PUBLIC_MODIFIERS: [&str; 3] = ["private", "protected", "private_class_method"];

/// A public method with no prose above it.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let require_non_public = context
        .setting::<bool>("RequireForNonPublicMethods")
        .unwrap_or(false);
    let allowed = context
        .setting::<Vec<String>>("AllowedMethods")
        .unwrap_or_default();
    let keywords = AnnotationKeywords::new(context);
    let preceding = PrecedingComments::new(context);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name) = node.field("name") else {
            continue;
        };
        let method = context.source.node_text(name);
        if method == "initialize" {
            continue;
        }
        // `modifier_node?(parent) ? check(parent) : check(node)`: the reported node is the wrapping
        // call when there is one.
        let reported = wrapping_modifier(node, context, &MODIFIERS).unwrap_or(node);
        if !require_non_public && !is_public(node, reported, context) {
            continue;
        }
        if documentation_comment(context, &preceding, reported, &keywords) {
            continue;
        }
        if allowed.iter().any(|entry| entry == method) {
            continue;
        }
        offenses.push(context.offense(MSG, send_node::send_range(reported, context)));
    }
}

/// `non_public?`, inverted: a `private def` wrapper, or a bare `private` written above.
fn is_public(node: Node<'_>, reported: Node<'_>, context: &RuleContext<'_>) -> bool {
    if wrapping_modifier(node, context, &NON_PUBLIC_MODIFIERS).is_some() {
        return false;
    }
    // `node_visibility` through the block form: the nearest bare `private` or `protected` above it.
    let Some(parent) = reported.parent() else {
        return true;
    };
    let siblings = super::nodes::children(parent);
    let Some(position) = siblings.iter().position(|child| child.id() == reported.id()) else {
        return true;
    };
    !siblings[..position]
        .iter()
        .rev()
        .any(|sibling| is_visibility_marker(*sibling, context))
}

/// The call the definition was handed to, when its selector is one of `names`.
fn wrapping_modifier<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
    names: &[&str],
) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    let call = if parent.kind_str() == "argument_list" {
        parent.parent()?
    } else {
        parent
    };
    if call.kind_str() != "call" || call.field("receiver").is_some() {
        return None;
    }
    let selector = call.field("method")?;
    names
        .contains(&context.source.node_text(selector))
        .then_some(call)
}

/// `visibility_block?`: `(send nil? {:private :protected :public})`, which the grammar leaves as a
/// bare `identifier` when it takes neither receiver nor arguments.
fn is_visibility_marker(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let name = match node.kind_str() {
        "identifier" => context.source.node_text(node),
        "call" if node.field("receiver").is_none() && node.field("arguments").is_none() => {
            match node.field("method") {
                Some(selector) => context.source.node_text(selector),
                None => return false,
            }
        }
        _ => return false,
    };
    matches!(name, "private" | "protected")
}
