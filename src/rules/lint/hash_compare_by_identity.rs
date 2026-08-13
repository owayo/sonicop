use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range};

use super::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `Hash#compare_by_identity` instead of using `object_id` for keys.";

/// `RESTRICT_ON_SEND`. `[]` and `[]=` only reach a `call` node when they were written with a dot;
/// the bracket form is an `element_reference` instead.
const METHODS: [&str; 5] = ["key?", "has_key?", "fetch", "[]", "[]="];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some((key, range)) = keyed_call(node, context) else {
            continue;
        };
        if object_id_call(key, context, &locals) {
            offenses.push(context.offense(MSG, range));
        }
    }
}

/// The first argument of a call that looks up or stores a hash entry, and the range upstream
/// reports it at.
fn keyed_call<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, std::ops::Range<usize>)> {
    if node.kind_str() == "element_reference" {
        let object = node.field("object")?;
        let mut cursor = node.walk();
        let key = node
            .named_children(&mut cursor)
            .find(|child| child.id() != object.id())?;
        // `hash[k] = v` is one `[]=` send upstream, spanning the assignment rather than the
        // brackets. Written in a multiple assignment the send holds no value and stops at the
        // brackets, which is where the node already ends.
        let range = node
            .parent()
            .filter(|parent| parent.kind_str() == "assignment")
            .filter(|parent| {
                parent
                    .field("left")
                    .is_some_and(|left| left.id() == node.id())
            })
            .map_or_else(|| node.byte_range(), |parent| parent.byte_range());
        return Some((key, range));
    }
    let method = node.field("method")?;
    if !METHODS.contains(&context.source.node_text(method)) {
        return None;
    }
    let key = arguments(node).first()?.first();
    Some((key, send_range(node, context)))
}

/// Whether the node is `(send _ :object_id)`: a call spelled with a dot rather than `&.`, taking no
/// arguments and no block, or the receiverless form written as a bare name.
fn object_id_call(node: Node<'_>, context: &RuleContext<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    match node.kind_str() {
        "call" => {
            node.field("method")
                .is_some_and(|method| context.source.node_text(method) == "object_id")
                && is_plain_send(node, context)
                && node.field("block").is_none()
                && arguments(node).is_empty()
        }
        "identifier" => context.source.node_text(node) == "object_id" && !locals.is_lvar(node),
        _ => false,
    }
}
