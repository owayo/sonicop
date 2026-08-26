use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use parentheses for `super` with arguments.";

/// `on_super`: a `super` that forwards written-out arguments. The bare `super` is a `zsuper`
/// upstream and never reaches this cop, which is why only the calls that carry an argument list
/// are walked here.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // **`super \\\n  +bar` is a `super` with one argument upstream.** The parser reads the sign as
    // belonging to the operand; the grammar reads the whole line as a binary expression whose left
    // operand is the keyword, so the call never appears and the offense went unreported.
    for node in context.nodes_of_any(&["call", "binary"]) {
        let (keyword, arguments) = match node.kind_str() {
            "binary" => {
                let (Some(left), Some(right), Some(operator)) =
                    (node.field("left"), node.field("right"), node.field("operator"))
                else {
                    continue;
                };
                if left.kind_str() != "super" {
                    continue;
                }
                // Only a sign the parser folds into the operand makes the line an argument:
                // `super == bar` is a comparison, and `super + 1` an addition, wherever the
                // spacing puts them. The sign has to hug its operand and stand apart from the
                // keyword, which is the spelling `super \` + `-1` has after the continuation.
                let text = context.source.node_text(operator);
                let source = context.source.text();
                let hugs = !source[operator.end_byte()..].starts_with([' ', '\t']);
                if !matches!(text, "+" | "-") || !hugs {
                    continue;
                }
                (left, right)
            }
            _ => {
                let Some(keyword) = node.field("method") else {
                    continue;
                };
                if keyword.kind_str() != "super" {
                    continue;
                }
                let Some(arguments) = node.field("arguments") else {
                    continue;
                };
                (keyword, arguments)
            }
        };
        // `node.parenthesized?`: the argument list starts at the paren when there is one, so its
        // first byte is what decides.
        if context.source.node_text(arguments).starts_with('(') {
            continue;
        }
        // A binary's right operand is the single argument; a list holds them all.
        let written = match node.kind_str() {
            "binary" => vec![arguments],
            _ => super::nodes::children(arguments),
        };
        let (Some(first), Some(last)) = (written.first(), written.last()) else {
            continue;
        };
        // Upstream's `super` node ends where its arguments do -- a block written after it belongs
        // to the `block` node wrapped around it -- so the report stops at the last argument.
        let range = node.start_byte()..last.end_byte();
        offenses.push(context.offense(MSG, range).corrected_by_all([
            // `corrector.replace(keyword.end.join(first_argument.begin), '(')`: the space between
            // the keyword and the arguments becomes the opening paren.
            Edit {
                start: keyword.end_byte(),
                end: first.start_byte(),
                replacement: "(".to_owned(),
                safe: true,
            },
            // `corrector.insert_after(last_argument, ')')`.
            Edit {
                start: last.end_byte(),
                end: last.end_byte(),
                replacement: ")".to_owned(),
                safe: true,
            },
        ]));
    }
}
