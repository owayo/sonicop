//! `Layout/MultilineAssignmentLayout`: which line the right hand side of an assignment opens on.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const NEW_LINE_OFFENSE: &str = "Right hand side of multi-line assignment is on the same line as \
                                the assignment operator `=`.";
const SAME_LINE_OFFENSE: &str = "Right hand side of multi-line assignment is not on the same line \
                                 as the assignment operator `=`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let same_line_style = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "same_line");
    let types: Vec<String> = context.setting("SupportedTypes").unwrap_or_else(|| {
        ["block", "case", "class", "if", "kwbegin", "module"]
            .map(str::to_owned)
            .to_vec()
    });
    for node in context.nodes_of_any(&["assignment", "operator_assignment"]) {
        let (Some(operator), Some(rhs)) = (assignment_operator(node), node.field("right")) else {
            continue;
        };
        if !types.iter().any(|kind| is_type(rhs, kind)) {
            continue;
        }
        // A right hand side that fits on one line only counts when it is a block whose brace was
        // pushed onto another line.
        let single_line = rhs.start_position().row == rhs.end_position().row;
        if single_line
            && (!is_type(rhs, "block")
                || block_opening(rhs)
                    .is_some_and(|open| open.start_position().row == node.start_position().row))
        {
            continue;
        }
        if same_line_style {
            if operator.start_position().row == rhs.start_position().row {
                continue;
            }
            let range = operator.end_byte()..rhs.start_byte();
            offenses.push(
                context
                    .offense(SAME_LINE_OFFENSE, node.byte_range())
                    .corrections_anchored_at(range.clone())
                    .corrected_by(Edit {
                        start: range.start,
                        end: range.end,
                        replacement: " ".to_owned(),
                        safe: true,
                    }),
            );
        } else {
            if operator.start_position().row != rhs.start_position().row {
                continue;
            }
            let at = operator.end_byte();
            offenses.push(
                context
                    .offense(NEW_LINE_OFFENSE, node.byte_range())
                    .corrections_anchored_at(operator.byte_range())
                    .corrected_by(Edit {
                        start: at,
                        end: at,
                        replacement: "\n".to_owned(),
                        safe: true,
                    }),
            );
        }
    }
}

/// `node.loc.operator`: the `=` or the `+=` the write is spelled with.
fn assignment_operator<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let left = node.field("left")?;
    let operator = left.next_sibling()?;
    (!operator.is_named()).then_some(operator)
}

/// The node types `SupportedTypes` names, as the grammar spells them.
fn is_type(node: Node<'_>, kind: &str) -> bool {
    match kind {
        // `block`, `numblock` and `itblock` are all a call carrying a block here.
        "block" => node.kind_str() == "call" && node.field("block").is_some(),
        "case" => matches!(node.kind_str(), "case" | "case_match"),
        "class" => node.kind_str() == "class",
        "if" => matches!(node.kind_str(), "if" | "unless"),
        "kwbegin" => node.kind_str() == "begin",
        "module" => node.kind_str() == "module",
        _ => false,
    }
}

/// `rhs.loc.begin`: the `{` or `do` the block opens with.
fn block_opening<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("block")
}
