use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::literals::is_literal;
use super::locals::LocalVariables;
use super::statements::body_children;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;
use crate::rules::send_node::named_children_iter;

/// `ASSIGNMENT_TYPES`: the variable kinds the tracker follows. A constant is deliberately missing.
const ASSIGNMENT_TARGETS: &[&str] = &[
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(body) = node.field("body") else {
            continue;
        };
        let Some(last) = last_expression(body) else {
            continue;
        };
        let Some(receiver) = setter_call_to_local_variable(last, &locals) else {
            continue;
        };
        let name = context.source.node_text(receiver);
        let mut tracker = Tracker {
            local: HashMap::new(),
        };
        tracker.scan_body(body, context, &locals);
        if !tracker.local.get(name).copied().unwrap_or(false) {
            continue;
        }
        let (_, column) = context.source.line_column(last.start_byte());
        let indent = " ".repeat(column - 1);
        offenses.push(
            context
                .offense(
                    format!("Useless setter call to local variable `{name}`."),
                    receiver.byte_range(),
                )
                .corrections_anchored_at(last.byte_range())
                .corrected_by(Edit {
                    start: last.end_byte(),
                    end: last.end_byte(),
                    replacement: format!("\n{indent}{name}"),
                    safe: false,
                }),
        );
    }
}

/// `last_expression`: the last statement of the body, or the body itself when it holds one.
///
/// A body split by a `rescue` or an `ensure` is that clause upstream rather than a sequence, and a
/// clause is no send, so such a method never reports.
fn last_expression<'tree>(body: Node<'tree>) -> Option<Node<'tree>> {
    body_children(body).into_iter().next_back()
}

/// `[(send (lvar _) ...) setter_method?]`: an assignment written as a call on a local variable.
fn setter_call_to_local_variable<'tree>(
    node: Node<'tree>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Node<'tree>> {
    if node.kind_str() != "assignment" {
        return None;
    }
    let left = node.field("left")?;
    let receiver = match left.kind_str() {
        "call" => left.field("receiver")?,
        "element_reference" => left.child(0)?,
        _ => return None,
    };
    (receiver.kind_str() == "identifier" && locals.is_lvar(receiver)).then_some(receiver)
}

/// `MethodVariableTracker`: which names hold an object this method built.
struct Tracker {
    local: HashMap<String, bool>,
}

impl Tracker {
    fn scan_body(
        &mut self,
        body: Node<'_>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) {
        for statement in body_children(body) {
            self.scan(statement, context, locals);
        }
    }

    /// `scan`, whose `throw :skip_children` stops the walk at the assignments that consume their
    /// own children.
    fn scan(&mut self, node: Node<'_>, context: &RuleContext<'_>, locals: &LocalVariables<'_, '_>) {
        if self.process_assignment_node(node, context, locals) {
            return;
        }
        for child in named_children_of(node, context) {
            self.scan(child, context, locals);
        }
    }

    /// Whether the node's children were consumed.
    fn process_assignment_node(
        &mut self,
        node: Node<'_>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) -> bool {
        match node.kind_str() {
            "assignment" => {
                let (Some(left), Some(right)) = (
                    node.field("left"),
                    node.field("right"),
                ) else {
                    return false;
                };
                if left.kind_str() == "left_assignment_list" {
                    self.process_multiple_assignment(left, right, context, locals);
                    return true;
                }
                if ASSIGNMENT_TARGETS.contains(&left.kind_str()) {
                    self.process_assignment(left, right, context, locals);
                }
                false
            }
            "operator_assignment" => {
                let (Some(left), Some(right), Some(operator)) = (
                    node.field("left"),
                    node.field("right"),
                    node.field("operator"),
                ) else {
                    return false;
                };
                if !ASSIGNMENT_TARGETS.contains(&left.kind_str()) {
                    return false;
                }
                if matches!(context.source.node_text(operator), "||=" | "&&=") {
                    self.process_assignment(left, right, context, locals);
                } else {
                    self.local.insert(text(left, context), true);
                }
                true
            }
            _ => false,
        }
    }

    /// `process_multiple_assignment`: a target that takes its value from a listed element follows
    /// that element, while one taking a slice of an unknown right-hand side is assumed local.
    fn process_multiple_assignment(
        &mut self,
        targets: Node<'_>,
        right: Node<'_>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) {
        let listed = matches!(right.kind_str(), "right_assignment_list" | "array");
        let values = if listed {
            named_children_of(right, context)
        } else {
            Vec::new()
        };
        for (index, target) in named_children_iter(targets, context).enumerate() {
            if !ASSIGNMENT_TARGETS.contains(&target.kind_str()) {
                continue;
            }
            match values.get(index) {
                Some(value) if listed => {
                    self.process_assignment(target, *value, context, locals);
                }
                _ => {
                    self.local.insert(text(target, context), true);
                }
            }
        }
    }

    /// `process_assignment`: a variable copies what it was read from, anything else is local when
    /// it is a literal or a constructor call.
    fn process_assignment(
        &mut self,
        target: Node<'_>,
        value: Node<'_>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) {
        let local = if is_variable(value, locals) {
            self.local
                .get(context.source.node_text(value))
                .copied()
                .unwrap_or(false)
        } else {
            is_constructor(value, context)
        };
        self.local.insert(text(target, context), local);
    }
}

/// `node.variable?`: `lvar`, `ivar`, `cvar` or `gvar`.
fn is_variable(node: Node<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    match node.kind_str() {
        "identifier" => locals.is_lvar(node),
        kind => matches!(
            kind,
            "instance_variable" | "class_variable" | "global_variable"
        ),
    }
}

/// `constructor?`.
fn is_constructor(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if is_literal(node, context) {
        return true;
    }
    node.kind_str() == "call"
        && node
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "new")
}

fn text(node: Node<'_>, context: &RuleContext<'_>) -> String {
    context.source.node_text(node).to_owned()
}
