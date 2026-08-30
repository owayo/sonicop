use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::ambiguity::scan;
use crate::rules::send_node::named_children_of;
use crate::rules::send_node::named_children_iter;

const MSG: &str = "Ambiguous regexp literal. Parenthesize the method arguments if it's surely a \
     regexp literal, or add a whitespace to the right of the `/` if it should be a division.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for ambiguity in scan(context, &["/"]) {
        let edits = match matches_a_static_regexp(ambiguity.owner, context) {
            true => ambiguity.wrap(),
            false => ambiguity.parenthesize(context),
        };
        offenses.push(
            context
                .offense(MSG, ambiguity.operator.clone())
                .corrections_anchored_at(ambiguity.owner.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// Whether the argument being parenthesized is `/re/ =~ str`, which the parser turns into a
/// `match_with_lvasgn` -- a node with no `arguments`, so `add_parentheses` wraps it whole rather
/// than moving the space after the selector.
fn matches_a_static_regexp(owner: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(arguments) = owner.field("arguments") else {
        return false;
    };
    let _cursor = arguments.walk();
    let Some(first) = named_children_iter(arguments, context).next() else {
        return false;
    };
    if first.kind_str() != "binary"
        || first
            .field("operator")
            .is_none_or(|operator| context.source.node_text(operator) != "=~")
    {
        return false;
    }
    // An interpolating regexp is not known until the program runs and stays an ordinary call.
    first
        .field("left")
        .filter(|left| left.kind_str() == "regex")
        .is_some_and(|left| {
            let _cursor = left.walk();
            !named_children_of(left, context)
                .into_iter()
                .any(|part| part.kind_str() == "interpolation")
        })
}
