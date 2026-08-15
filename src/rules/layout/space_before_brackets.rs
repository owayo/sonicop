use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

const MSG: &str = "Remove the space before the opening brackets.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `RESTRICT_ON_SEND = %i[[] []=]` with `return if node.loc.dot`: a call written `a.[](1)` is
    // passed over, which leaves the bracket form alone.
    for node in context.nodes_of_any(&["element_reference", "call"]) {
        let Some((receiver_end, bracket_start)) = index_gap(node, context) else {
            continue;
        };
        if receiver_end >= bracket_start {
            continue;
        }
        let range = receiver_end..bracket_start;
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// Where the receiver ends and the `[` begins, for the nodes upstream reads as an index.
///
/// The grammar cannot tell `collection [0]` -- an index read written with a space -- from
/// `undefined_method [0]`, a call handed an array; Ruby settles it by whether the name is a local
/// variable, and writes the first as `element_reference` only once an `=` follows. So the second
/// shape has to be recovered from the variable analysis: a receiverless call whose name is a local
/// variable and whose sole argument is a bracketed array is the index `(send (lvar _) :[] _)`.
fn index_gap(node: Node<'_>, context: &RuleContext<'_>) -> Option<(usize, usize)> {
    match node.kind_str() {
        "element_reference" => {
            let object = node.field("object")?;
            let bracket = opening_bracket(node, context)?;
            Some((object.end_byte(), bracket.start_byte()))
        }
        _ => {
            if node.field("receiver").is_some() || node.field("block").is_some() {
                return None;
            }
            let method = node.field("method")?;
            if !context.variable_roles().names_a_local(method) {
                return None;
            }
            let arguments = node.field("arguments")?;
            let children = named_children(arguments);
            let [argument] = children.as_slice() else {
                return None;
            };
            // The argument has to open with the `[` that upstream reads as the index. That is the
            // array `collection [0]` was written with, and also the chain `collection [0][1]` or
            // `collection [0].foo` the grammar hangs off it -- all three begin at the same bracket.
            // `collection %w[a]` opens with a `%` and is no index.
            context.source.text()[argument.start_byte()..]
                .starts_with('[')
                .then(|| (method.end_byte(), argument.start_byte()))
        }
    }
}

/// `node.loc.selector`, which for an index read begins at the `[`.
fn opening_bracket<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && context.source.node_text(*child) == "[")
}
