use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::spurious_assignment_list;


/// `MISTAKES`: the operator each two-character run was probably meant to be.
const MISTAKES: [(u8, &str); 4] = [(b'-', "-="), (b'+', "+="), (b'*', "*="), (b'!', "!=")];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let bytes = context.source.text().as_bytes();
    for node in context.nodes_of_any(&["assignment", "operator_assignment"]) {
        let (Some(operator), Some(right)) = (node.child(1), node.field("right")) else {
            continue;
        };
        if is_folded_parameter_default(node, context) {
            continue;
        }
        // `range_between(operator.end_pos - 1, rhs.source_range.begin_pos + 1)` is two characters
        // only where the right-hand side starts the moment the operator ends; anything else, a
        // space above all, makes the span longer than the runs the cop knows.
        let end = operator.end_byte();
        if end == 0 || end != right.start_byte() || end >= bytes.len() || bytes[end - 1] != b'=' {
            continue;
        }
        let Some((_, meant)) = MISTAKES
            .iter()
            .find(|(character, _)| *character == bytes[end])
        else {
            continue;
        };
        let message = format!("Suspicious assignment detected. Did you mean `{meant}`?");
        offenses.push(context.offense(message, end - 1..end + 1));
    }
}

/// Whether the assignment is one the grammar invented out of `def r(a = nil, b = nil)`.
///
/// Upstream's parser reads those as two `optarg`s, which `CheckAssignment` has no handler for, so
/// the write the grammar folded into a multiple assignment must not be checked. The same folding
/// inside an argument list *is* an `lvasgn` upstream and stays.
fn is_folded_parameter_default(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(left) = node.field("left") else {
        return false;
    };
    if left.kind_str() != "left_assignment_list" || !spurious_assignment_list(left) {
        return false;
    }
    let mut current = node;
    while let Some(parent) = current.parent_of(context) {
        if matches!(
            parent.kind_str(),
            "optional_parameter" | "keyword_parameter"
        ) {
            return true;
        }
        let continues = parent.kind_str() == "assignment"
            && parent
                .field("right")
                .is_some_and(|right| right.id() == current.id());
        if !continues {
            return false;
        }
        current = parent;
    }
    false
}
