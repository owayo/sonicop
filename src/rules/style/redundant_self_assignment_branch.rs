use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const MSG: &str = "Remove the self-assignment branch.";

/// `x = if cond then x else y end`, where one branch only assigns `x` to itself.
///
/// `IfNode#if_branch` is the branch that was *written* first, which for an `unless` is its body even
/// though the raw AST holds the two the other way round -- so the mapping is the same for `if`,
/// `unless` and the ternary alike. A ternary has no `else` location, so `!expression.else?` lets it
/// through.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("assignment") {
        // `on_lvasgn`: only a local variable.
        let Some(target) = node
            .field("left")
            .filter(|target| target.kind_str() == "identifier")
        else {
            continue;
        };
        let name = context.source.node_text(target);
        let Some(expression) = node.field("right") else {
            continue;
        };
        // `use_if_and_else_branch?`: an `if` that is not a ternary. A ternary always has an else.
        let Some(condition) = expression.field("condition") else {
            continue;
        };
        if !matches!(expression.kind_str(), "if" | "unless" | "conditional") {
            continue;
        }
        let consequence = expression.field("consequence");
        let alternative = expression.field("alternative");
        // `inconvertible_to_modifier?`: an `elsif` cannot become a modifier.
        if alternative.is_some_and(|branch| branch.kind_str() == "elsif") {
            continue;
        }
        let (Ok(if_branch), Ok(else_branch)) = (branch_of(consequence), branch_of(alternative))
        else {
            continue;
        };
        let (offending, opposite, keyword) = if self_assigns(if_branch, name, context) {
            (if_branch, else_branch, "unless")
        } else if self_assigns(else_branch, name, context) {
            (else_branch, if_branch, "if")
        } else {
            continue;
        };
        let Some(offending) = offending else {
            continue;
        };
        let value = match opposite {
            Some(branch) => context.source.node_text(branch).to_owned(),
            None => "nil".to_owned(),
        };
        let mut replacement = format!(
            "{value} {keyword} {}",
            context.source.node_text(condition)
        );
        // A heredoc's body lives after the statement, so it has to be carried along.
        if let Some(branch) = opposite
            && branch.kind_str() == "heredoc_beginning"
            && let Some(body) = send_node::heredoc_body(branch, context)
        {
            replacement.push_str(&context.source.text()[branch.end_byte()..body.end_byte()]);
        }
        offenses.push(
            context
                .offense(MSG, offending.byte_range())
                .corrected_by(Edit {
                    start: expression.start_byte(),
                    end: expression.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// The single statement of a branch: `Ok(None)` when the branch is absent or empty, and `Err` when
/// it holds more than one statement -- which upstream reads as a `begin` and refuses to fold.
fn branch_of<'tree>(branch: Option<Node<'tree>>) -> Result<Option<Node<'tree>>, ()> {
    let Some(branch) = branch else {
        return Ok(None);
    };
    // A ternary's branches are the expressions themselves; `if` and `unless` wrap theirs in a
    // `then` or `else` node.
    if !matches!(branch.kind_str(), "then" | "else") {
        return Ok(Some(branch));
    }
    let statements: Vec<Node<'tree>> = super::nodes::children(branch)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    match statements.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(*only)),
        _ => Err(()),
    }
}

/// `self_assign?`: the branch is written exactly as the variable's name.
fn self_assigns(branch: Option<Node<'_>>, name: &str, context: &RuleContext<'_>) -> bool {
    branch.is_some_and(|node| context.source.node_text(node) == name)
}
