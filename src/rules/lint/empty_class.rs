use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::contains_comment;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_comments = context.setting::<bool>("AllowComments").unwrap_or(false);
    for node in context.nodes_of_any(&["class", "singleton_class"]) {
        if body_or_allowed_comment_lines(node, context, allow_comments) {
            continue;
        }
        let (message, empty) = match node.kind_str() {
            // `node.parent_class`: a subclass with no body of its own is still doing something.
            "class" => ("Empty class detected.", node.field("superclass").is_none()),
            _ => ("Empty metaclass detected.", true),
        };
        if empty {
            offenses.push(context.offense(message, node.byte_range()));
        }
    }
}

fn body_or_allowed_comment_lines(
    node: Node<'_>,
    context: &RuleContext<'_>,
    allow_comments: bool,
) -> bool {
    node.field("body").is_some() || (allow_comments && contains_comment(context, node.byte_range()))
}
