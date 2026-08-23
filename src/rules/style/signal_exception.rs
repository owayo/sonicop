use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{is_plain_send, top_level_constant};

const FAIL_MSG: &str = "Use `fail` instead of `raise` to signal exceptions.";
const RAISE_MSG: &str = "Use `raise` instead of `fail` to rethrow exceptions.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "only_raise".to_owned());
    let (looked_for, preferred) = match style.as_str() {
        "only_raise" => ("fail", "raise"),
        "only_fail" => ("raise", "fail"),
        "semantic" => return check_semantic(context, offenses),
        _ => return,
    };
    // `custom_fail_defined?`: a file that defines its own `fail` is not talking about `Kernel#fail`.
    if style == "only_raise" && defines_fail(context) {
        return;
    }
    let locals = LocalVariables::new(context);
    let message = format!("Always use `{preferred}` to signal exceptions.");
    for node in context.nodes_of_any(&["call", "identifier"]) {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        let Some(selector) = signal_selector(node, looked_for, context, &locals) else {
            continue;
        };
        offenses.push(
            context
                .offense(message.clone(), selector.byte_range())
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: preferred.to_owned(),
                    safe: true,
                }),
        );
    }
}

/// `:semantic`: `raise` belongs inside a rescue clause and `fail` everywhere else.
///
/// Upstream reaches the same split from two directions -- `on_rescue` walks the body for `raise`
/// and each `resbody` for `fail`, while `on_send` catches every remaining `raise` -- with
/// `ignore_node` keeping the two from reporting the same call twice. Asking each call where it
/// sits gives the same set without the bookkeeping: a call under a `rescue` clause's body may say
/// `raise`, and one anywhere else may say `fail`.
fn check_semantic(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of_any(&["call", "identifier"]) {
        if !is_plain_send(node, context) {
            continue;
        }
        let rescuing = inside_rescue_body(node);
        let looked_for = match rescuing {
            true => "fail",
            false => "raise",
        };
        let Some(selector) = signal_selector(node, looked_for, context, &locals) else {
            continue;
        };
        let (message, preferred) = match rescuing {
            true => (RAISE_MSG, "raise"),
            false => (FAIL_MSG, "fail"),
        };
        offenses.push(
            context
                .offense(message, selector.byte_range())
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: preferred.to_owned(),
                    safe: true,
                }),
        );
    }
}

/// Whether the call sits in the body of a `rescue` clause -- not in the exception list or the
/// `=> e` binding, which are part of the same node but not part of the clause's body.
fn inside_rescue_body(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind_str() == "rescue"
            && parent.field("body").is_some_and(|body| {
                body.start_byte() <= node.start_byte() && node.end_byte() <= body.end_byte()
            })
        {
            return true;
        }
        current = parent;
    }
    false
}

/// `command_or_kernel_call?`: the `raise` / `fail` token of a receiverless call or of one made
/// through `Kernel`.
fn signal_selector<'tree>(
    node: Node<'tree>,
    name: &str,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Node<'tree>> {
    match node.kind_str() {
        // A bare `fail` is an identifier here and a receiverless call upstream, unless a local
        // variable of that name is in scope.
        "identifier" => (context.source.node_text(node) == name
            && !locals.is_lvar(node)
            && !is_binding_site(node))
        .then_some(node),
        "call" => {
            let method = node.field("method")?;
            if context.source.node_text(method) != name {
                return None;
            }
            match node.field("receiver") {
                None => Some(method),
                Some(receiver) => top_level_constant(receiver, "Kernel", context).then_some(method),
            }
        }
        _ => None,
    }
}

/// Whether the identifier names something being bound rather than a call: an assignment target,
/// a parameter or the name of a definition. `fail = []` is an `lvasgn` upstream, not a `send`.
fn is_binding_site(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind_str() {
        "assignment" | "operator_assignment" | "for" => parent
            .field("left")
            .or_else(|| parent.field("pattern"))
            .is_some_and(|target| target.id() == node.id()),
        "call" | "method" | "singleton_method" | "alias" | "undef" => true,
        "left_assignment_list"
        | "rest_assignment"
        | "destructured_left_assignment"
        | "method_parameters"
        | "block_parameters"
        | "lambda_parameters"
        | "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter"
        | "exception_variable" => true,
        _ => false,
    }
}

/// `{(def :fail ...) (defs _ :fail ...)}` anywhere in the file.
fn defines_fail(context: &RuleContext<'_>) -> bool {
    context
        .nodes_of_any(&["method", "singleton_method"])
        .any(|node| {
            node.field("name")
                .is_some_and(|name| context.source.node_text(name) == "fail")
        })
}
