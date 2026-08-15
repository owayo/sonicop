use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::expression_range;

/// The node kinds upstream reaches through `on_if`, minus the two `OnNormalIfUnless` turns away:
/// the modifier form and the ternary. An `elsif` is an `if` of its own there and is checked too.
const CONDITIONALS: &[&str] = &["if", "unless", "elsif"];

/// What `Style/EmptyElse` leaves an added `else` holding, which is also what this one asks for.
struct Filling {
    /// `MSG_NIL` / `MSG_EMPTY`, whichever that cop's style leaves room for.
    message: &'static str,
    replacement: &'static str,
}

/// A conditional written without the branch that says what to do when nothing matched.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "both".to_owned());
    // An `else` holding nothing is what the neighbouring cop reports unless it is configured to
    // report a `nil` instead, so which of the two to ask for is read off its style. Asking for
    // neither leaves the offense without a correction.
    let filling = match context
        .setting_of::<String>("Style/EmptyElse", "EnforcedStyle")
        .as_deref()
    {
        Some("empty") => Some(Filling {
            message: "`{}` condition requires an `else`-clause with `nil` in it.",
            replacement: "else; nil; ",
        }),
        Some("nil") => Some(Filling {
            message: "`{}` condition requires an empty `else`-clause.",
            replacement: "else; ",
        }),
        _ => None,
    };
    if style != "case" {
        // `unless_else_cop_enabled?`: the cop that turns an `unless ... else` around has the say
        // about an `unless` when it is switched on.
        let leave_unless = context.cop_enabled("Style/UnlessElse");
        for node in context.nodes_of_any(CONDITIONALS) {
            if (leave_unless && node.kind_str() == "unless") || node.field("alternative").is_some()
            {
                continue;
            }
            report(node, "if", filling.as_ref(), context, offenses);
        }
    }
    if style != "if" {
        // `on_case_match` does nothing: a `case ... in` raises when nothing matches, so it needs
        // no `else` to be complete.
        for node in context.nodes_of("case") {
            if super::nodes::children(node)
                .iter()
                .any(|child| child.kind_str() == "else")
            {
                continue;
            }
            report(node, "case", filling.as_ref(), context, offenses);
        }
    }
}

fn report(
    node: Node<'_>,
    kind: &str,
    filling: Option<&Filling>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let Some(filling) = filling else {
        offenses.push(context.offense(
            format!("`{kind}` condition requires an `else`-clause."),
            expression_range(node),
        ));
        return;
    };
    let offense = context.offense(filling.message.replace("{}", kind), expression_range(node));
    // `node.ancestors.find { |ancestor| ancestor.loc.end }`: an `elsif` is closed by the `end` of
    // the `if` it belongs to rather than by one of its own.
    let Some(end) = closing_end(node, context) else {
        offenses.push(offense);
        return;
    };
    offenses.push(offense.corrected_by(Edit {
        start: end,
        end,
        replacement: filling.replacement.to_owned(),
        safe: true,
    }));
}

/// Where the `end` that closes the node begins, looking outwards until one is found.
fn closing_end(node: Node<'_>, context: &RuleContext<'_>) -> Option<usize> {
    let mut current = Some(node);
    while let Some(candidate) = current {
        let last = u32::try_from(candidate.child_count())
            .ok()
            .and_then(|count| count.checked_sub(1))
            .and_then(|index| candidate.child(index));
        if let Some(last) = last
            && !last.is_named()
            && context.source.node_text(last) == "end"
        {
            return Some(last.start_byte());
        }
        current = candidate.parent();
    }
    None
}
