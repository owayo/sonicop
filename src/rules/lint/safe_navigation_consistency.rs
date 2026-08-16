use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::nil_methods::nil_methods;
use crate::rules::node_ext::NodeExt;

const USE_DOT_MSG: &str = "Use `.` instead of unnecessary `&.`.";
const USE_SAFE_NAVIGATION_MSG: &str = "Use `&.` for consistency with safe navigation.";

/// `OPERATOR_METHODS`.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

/// One collected operand of an `and`/`or` chain, in upstream's terms.
struct Operand<'tree> {
    node: Node<'tree>,
    /// The source of `receiver`, which is what the operands are grouped by.
    key: String,
    method: String,
    /// `loc.dot`, absent for a receiverless call.
    dot: Option<Range<usize>>,
    safe_navigation: bool,
    operator_method: bool,
    /// Whether the operand is written directly under an `and` rather than an `or`.
    in_and: bool,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = nil_methods(context);
    for node in context.nodes_of("binary") {
        if logical_operator(node, context).is_none() {
            continue;
        }
        let mut operands = Vec::new();
        collect_operands(node, context, &mut operands);
        // `group_by` keeps the order the operands were collected in.
        let mut keys: Vec<String> = Vec::new();
        for operand in &operands {
            if !keys.contains(&operand.key) {
                keys.push(operand.key.clone());
            }
        }
        for key in keys {
            let group: Vec<&Operand<'_>> = operands
                .iter()
                .filter(|operand| operand.key == key)
                .collect();
            let Some((dot, rest_from)) = consistent_parts(&group, &allowed) else {
                continue;
            };
            for operand in group.into_iter().skip(rest_from) {
                if already_appropriate_call(operand, dot) {
                    continue;
                }
                register_offense(operand, dot, context, offenses);
            }
        }
    }
}

/// `&&`/`and` and `||`/`or`, told apart for the two indices they feed.
fn logical_operator(node: Node<'_>, context: &RuleContext<'_>) -> Option<bool> {
    let operator = node.field("operator")?;
    match context.source.node_text(operator) {
        "&&" | "and" => Some(true),
        "||" | "or" => Some(false),
        _ => None,
    }
}

/// `collect_operands`, which walks through the nested `and`/`or` nodes and keeps the calls.
fn collect_operands<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
    found: &mut Vec<Operand<'tree>>,
) {
    let Some(in_and) = logical_operator(node, context) else {
        return;
    };
    for side in ["left", "right"] {
        let Some(operand) = node.field(side) else {
            continue;
        };
        if logical_operator(operand, context).is_some() {
            collect_operands(operand, context, found);
        } else if let Some(operand) = read_operand(operand, in_and, context) {
            found.push(operand);
        }
    }
}

/// `operand.call_type?`, in the node kinds tree-sitter writes a call as.
fn read_operand<'tree>(
    node: Node<'tree>,
    in_and: bool,
    context: &RuleContext<'_>,
) -> Option<Operand<'tree>> {
    let (receiver, method, dot) = match node.kind_str() {
        "call" => (
            node.field("receiver"),
            context
                .source
                .node_text(node.field("method")?)
                .to_owned(),
            node.field("operator")
                .map(|operator| operator.byte_range()),
        ),
        "binary" => (
            node.field("left"),
            context
                .source
                .node_text(node.field("operator")?)
                .to_owned(),
            None,
        ),
        "unary" => {
            let operator = node.field("operator")?;
            let text = context.source.node_text(operator);
            let method = match text {
                "-" => "-@".to_owned(),
                "+" => "+@".to_owned(),
                "!" | "~" | "not" | "defined?" => text.to_owned(),
                _ => return None,
            };
            (node.field("operand"), method, None)
        }
        "element_reference" => (node.child(0), "[]".to_owned(), None),
        // `foo&.baz = 1` is one `csend` upstream, with the method name `baz=`. The grammar spells
        // it as an assignment whose left is the call, so the operand has to be read through it --
        // otherwise a safe navigation written on the left of an assignment is never an operand.
        "assignment" | "operator_assignment" => {
            let left = node.field("left")?;
            if left.kind_str() != "call" {
                return None;
            }
            (
                left.field("receiver"),
                format!("{}=", context.source.node_text(left.field("method")?)),
                left.field("operator").map(|operator| operator.byte_range()),
            )
        }
        _ => return None,
    };
    let safe_navigation = dot
        .as_ref()
        .is_some_and(|dot| context.source.slice(dot.clone()) == "&.");
    Some(Operand {
        node,
        key: receiver.map_or_else(String::new, |receiver| {
            context.source.node_text(receiver).to_owned()
        }),
        operator_method: OPERATOR_METHODS.contains(&method.as_str()),
        method,
        dot,
        safe_navigation,
        in_and,
    })
}

/// `find_consistent_parts`: which operator the group should use, and where the run that has to be
/// rewritten begins.
fn consistent_parts(group: &[&Operand<'_>], allowed: &[String]) -> Option<(&'static str, usize)> {
    let mut csend_in_and = None;
    let mut csend_in_or = None;
    let mut send_in_and = None;
    let mut send_in_or = None;
    for (index, operand) in group.iter().enumerate() {
        let nilable = operand.safe_navigation || allowed.contains(&operand.method);
        if operand.in_and {
            if operand.safe_navigation {
                csend_in_and = csend_in_and.or(Some(index));
            }
            if !nilable {
                send_in_and = send_in_and.or(Some(index));
            }
        } else {
            if operand.safe_navigation {
                csend_in_or = csend_in_or.or(Some(index));
            }
            if !nilable {
                send_in_or = send_in_or.or(Some(index));
            }
        }
    }
    if let (Some(and), Some(or)) = (csend_in_and, csend_in_or)
        && and < or
    {
        return None;
    }
    if let Some(and) = csend_in_and {
        let first = send_in_and.map_or(and, |send| send.min(and));
        return Some((".", first + 1));
    }
    if let (Some(send), Some(csend)) = (send_in_or, csend_in_or) {
        return Some(if send < csend {
            (".", send + 1)
        } else {
            ("&.", csend + 1)
        });
    }
    if let (Some(send), Some(csend)) = (send_in_and, csend_in_or)
        && send < csend
    {
        return Some((".", csend));
    }
    None
}

/// `already_appropriate_call?`.
fn already_appropriate_call(operand: &Operand<'_>, dot: &str) -> bool {
    if operand.safe_navigation && dot == "&." {
        return true;
    }
    let plain_dot = operand.dot.is_some() && !operand.safe_navigation;
    (plain_dot || operand.operator_method) && dot == "."
}

fn register_offense(
    operand: &Operand<'_>,
    dot: &str,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let message = if dot == "." {
        USE_DOT_MSG
    } else {
        USE_SAFE_NAVIGATION_MSG
    };
    if operand.operator_method {
        offenses.push(context.offense(message, operand.node.byte_range()));
        return;
    }
    let Some(range) = operand.dot.clone() else {
        return;
    };
    offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: dot.to_owned(),
        safe: false,
    }));
}
