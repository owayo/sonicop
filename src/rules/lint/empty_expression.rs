use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::statements::statements;

const MSG: &str = "Avoid empty expressions.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_begin` reaches the `begin` node that `(...)` builds and the one an interpolation holds,
    // but never the `kwbegin` of `begin ... end`, which is a type of its own upstream.
    for node in context.nodes_of_any(&[
        "parenthesized_statements",
        "interpolation",
        // **`return()` writes its parentheses as an argument list here.** Upstream builds the
        // same empty `begin` a bare `()` does, so the jump's empty parentheses are an empty
        // expression too -- the grammar just files them under a different kind.
        "argument_list",
    ]) {
        if node.kind_str() == "argument_list"
            && node
                .parent()
                .is_none_or(|parent| !matches!(parent.kind_str(), "return" | "break" | "next"))
        {
            continue;
        }
        if statements(node).is_empty() {
            offenses.push(context.offense(MSG, node.byte_range()));
        }
    }
}
