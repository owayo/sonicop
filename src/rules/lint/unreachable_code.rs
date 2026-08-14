use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::flow::{self, Flow};
use super::locals::LocalVariables;
use super::statements::begin_groups;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Unreachable code detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `@redefined` outlives one `on_begin`: a `def raise` seen in an earlier statement sequence
    // still excuses a later `raise` in the same file. The walk has to keep it in the order upstream
    // visits the sequences in, which is where they start.
    let mut flow = Flow::new();
    let locals = LocalVariables::new(context);
    for group in begin_groups(context) {
        let mut reached = false;
        for (index, expression) in group.iter().enumerate() {
            if reached {
                offenses.push(context.offense(MSG, expression.byte_range()));
            } else if index + 1 < group.len()
                && flow_expression(*expression, context, &locals, &mut flow)
            {
                reached = true;
            }
        }
    }
}

/// Whether reaching this expression means nothing after it runs. A `def` is not one, but the method
/// it defines may shadow `raise` from here on, which is what the walk records rather than answers.
fn flow_expression(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    flow: &mut Flow,
) -> bool {
    if flow::is_command(node, context, locals) {
        return flow.reports_command(node, context);
    }
    match node.kind_str() {
        "begin" => super::statements::body_children(node)
            .into_iter()
            .any(|child| flow_expression(child, context, locals, flow)),
        "parenthesized_statements" => super::statements::statements(node)
            .into_iter()
            .any(|child| flow_expression(child, context, locals, flow)),
        "if" | "unless" | "elsif" | "conditional" => flow::check_if(node, &mut |child| {
            flow_expression(child, context, locals, flow)
        }),
        "case" | "case_match" => flow::check_case(node, &mut |child| {
            flow_expression(child, context, locals, flow)
        }),
        "method" | "singleton_method" => {
            flow.register_redefinition(node, context);
            false
        }
        _ => false,
    }
}
