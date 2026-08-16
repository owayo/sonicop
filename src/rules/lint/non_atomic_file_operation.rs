use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments, pair_key_symbol};

use super::node_equality::identical;
use super::statements::statements;

/// The four method groups, and what each is replaced by.
const MAKE_FORCE_METHODS: [&str; 3] = ["makedirs", "mkdir_p", "mkpath"];
const MAKE_METHODS: [&str; 1] = ["mkdir"];
const REMOVE_FORCE_METHODS: [&str; 2] = ["rm_f", "rm_rf"];
const REMOVE_METHODS: [&str; 7] = [
    "remove",
    "delete",
    "unlink",
    "remove_file",
    "rm",
    "rmdir",
    "safe_unlink",
];
const RECURSIVE_REMOVE_METHODS: [&str; 3] = ["remove_dir", "remove_entry", "remove_entry_secure"];

/// `(const {cbase nil?} {:FileTest :File :Dir :Shell})`, the receivers whose `exist?` the check is
/// written on.
const EXIST_RECEIVERS: [&str; 4] = ["FileTest", "File", "Dir", "Shell"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
            continue;
        };
        // `on_send` は `csend` に呼ばれない。`alias on_csend on_send` を書いていない cop は
        // `x&.foo` を構造的に一切見ないので、ここで落とさないと過剰検出になる。
        if !crate::rules::send_node::is_plain_send(node, context) {
            continue;
        }
        let name = context.source.node_text(method);
        if !is_restricted(name) || !is_constant(receiver) {
            continue;
        }
        let Some(conditional) = enclosing_conditional(node, context) else {
            continue;
        };
        if explicitly_not_force(node, context) {
            continue;
        }
        let Some(exist) = find_exist_call(conditional, context) else {
            continue;
        };
        let call_arguments = arguments(node);
        let exist_arguments = arguments(exist);
        let (Some(first), Some(exist_first)) = (call_arguments.first(), exist_arguments.first())
        else {
            continue;
        };
        if !identical(first.first(), exist_first.first(), context) {
            continue;
        }
        report(context, offenses, node, conditional, exist, &call_arguments);
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    conditional: Node<'_>,
    exist: Node<'_>,
    call_arguments: &[Argument<'_>],
) {
    let replacement = replacement_method(node, context);
    if !is_force_method(node, context) {
        offenses.push(context.offense(
            format!("Use atomic file operation method `FileUtils.{replacement}`."),
            node.byte_range(),
        ));
    }
    let (Some(keyword), Some(condition)) = (conditional.child(0), conditional.field("condition"))
    else {
        return;
    };
    // `parent.loc.keyword.begin.join(parent.condition.source_range.end)`: for a modifier the
    // keyword stands after the body, so the span is not the head of the line.
    let keyword_start = if conditional.kind_str().ends_with("_modifier") {
        modifier_keyword_start(conditional, context)
    } else {
        keyword.start_byte()
    };
    let range = keyword_start..condition.end_byte();
    // `receiver_and_method_name` captures the constant's *name*, so a `::File` is named `File`.
    let message = format!(
        "Remove unnecessary existence check `{}.{}`.",
        exist
            .field("receiver")
            .and_then(|receiver| short_constant_name(receiver, context))
            .unwrap_or_default(),
        exist
            .field("method")
            .map_or("", |method| context.source.node_text(method)),
    );
    let offense = context.offense(message, range.clone());
    // `parent.elsif?`: the check cannot simply be dropped from the middle of a chain.
    offenses.push(if conditional.kind_str() == "elsif" {
        offense
    } else {
        offense.corrected_by_all(corrections(
            context,
            node,
            conditional,
            range,
            call_arguments,
            &replacement,
        ))
    });
}

fn corrections(
    context: &RuleContext<'_>,
    node: Node<'_>,
    conditional: Node<'_>,
    range: Range<usize>,
    call_arguments: &[Argument<'_>],
    replacement: &str,
) -> Vec<Edit> {
    let mut edits = vec![Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }];
    if !is_force_method(node, context) {
        if let (Some(receiver), Some(method)) = (node.field("receiver"), node.field("method")) {
            edits.push(Edit {
                start: receiver.start_byte(),
                end: receiver.end_byte(),
                replacement: "FileUtils".to_owned(),
                safe: true,
            });
            edits.push(Edit {
                start: method.start_byte(),
                end: method.end_byte(),
                replacement: replacement.to_owned(),
                safe: true,
            });
        }
        // `require_mode_keyword?`: `Dir.mkdir(path, 0o700)` passes the mode positionally, which
        // `FileUtils.mkdir_p` takes by keyword.
        if requires_mode_keyword(node, call_arguments, replacement, context)
            && let Some(last) = call_arguments.last()
        {
            let start = last.range().start;
            edits.push(Edit {
                start,
                end: start,
                replacement: "mode: ".to_owned(),
                safe: true,
            });
        }
    }
    if conditional.kind_str().ends_with("_modifier") {
        edits.push(Edit {
            start: node.end_byte(),
            end: modifier_keyword_start(conditional, context),
            replacement: String::new(),
            safe: true,
        });
    } else if let Some(end) = conditional
        .child(conditional.child_count().saturating_sub(1) as u32)
        .filter(|end| context.source.node_text(*end) == "end")
    {
        edits.push(Edit {
            start: end.start_byte(),
            end: end.end_byte(),
            replacement: String::new(),
            safe: true,
        });
    }
    edits
}

/// Where the `if` or `unless` of a modifier begins, which the grammar leaves as an anonymous token
/// between the body and the condition.
fn modifier_keyword_start(conditional: Node<'_>, context: &RuleContext<'_>) -> usize {
    let keyword = if conditional.kind_str().starts_with("unless") {
        "unless"
    } else {
        "if"
    };
    let Some(condition) = conditional.field("condition") else {
        return conditional.start_byte();
    };
    context.source.text()[..condition.start_byte()]
        .rfind(keyword)
        .unwrap_or_else(|| conditional.start_byte())
}

fn is_restricted(name: &str) -> bool {
    MAKE_METHODS.contains(&name)
        || MAKE_FORCE_METHODS.contains(&name)
        || REMOVE_METHODS.contains(&name)
        || RECURSIVE_REMOVE_METHODS.contains(&name)
        || REMOVE_FORCE_METHODS.contains(&name)
}

fn is_constant(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "constant" | "scope_resolution")
}

/// `if_node_child?`: the call is the whole body of an `if` or `unless` that has no `else` and
/// whose condition is a single test.
fn enclosing_conditional<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let parent = node.parent_of(context)?;
    // A body holding one statement *is* that statement upstream; anything else is a `begin`, whose
    // parent is not an `if`.
    let conditional = match parent.kind_str() {
        "then" | "else" if statements(parent).len() == 1 => parent.parent_of(context)?,
        "if_modifier" | "unless_modifier" => parent,
        _ => return None,
    };
    if !matches!(
        conditional.kind_str(),
        "if" | "unless" | "elsif" | "if_modifier" | "unless_modifier"
    ) {
        return None;
    }
    // `allowable_use_with_if?`: a compound condition or an `else` means the check is doing more.
    let condition = conditional.field("condition")?;
    if is_operator_keyword(condition, context) || conditional.field("alternative").is_some() {
        return None;
    }
    Some(conditional)
}

fn is_operator_keyword(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node.child(1).is_some_and(|operator| {
            matches!(
                context.source.node_text(operator),
                "&&" | "||" | "and" | "or"
            )
        })
}

/// `send_exist_node`: the first `File.exist?` written anywhere inside the conditional.
fn find_exist_call<'tree>(
    conditional: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut stack = vec![conditional];
    while let Some(node) = stack.pop() {
        if node.kind_str() == "call"
            && let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver"))
            && matches!(context.source.node_text(method), "exist?" | "exists?")
            && short_constant_name(receiver, context)
                .is_some_and(|name| EXIST_RECEIVERS.contains(&name))
        {
            return Some(node);
        }
        let mut children = Vec::new();
        crate::rules::push_named_children(node, &mut children);
        children.reverse();
        stack.extend(children);
    }
    None
}

/// `(const {cbase nil?} $_)`: a constant reached from the top level, and its name.
fn short_constant_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" if node.field("scope").is_none() => {
            Some(context.source.node_text(node.field("name")?))
        }
        _ => None,
    }
}

/// `explicit_not_force?`: `force: false` says the author wants the check.
fn explicitly_not_force(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    force_pair_value(node, context) == Some(false)
}

fn is_force_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let name = node
        .field("method")
        .map_or("", |method| context.source.node_text(method));
    MAKE_FORCE_METHODS.contains(&name)
        || REMOVE_FORCE_METHODS.contains(&name)
        || force_pair_value(node, context) == Some(true)
}

fn force_pair_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<bool> {
    arguments(node).iter().find_map(|argument| {
        argument.parts().iter().find_map(|part| {
            if pair_key_symbol(*part, context) != Some("force") {
                return None;
            }
            match part.field("value")?.kind_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }
        })
    })
}

fn replacement_method(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let name = node
        .field("method")
        .map_or("", |method| context.source.node_text(method));
    if MAKE_METHODS.contains(&name) {
        "mkdir_p".to_owned()
    } else if REMOVE_METHODS.contains(&name) {
        "rm_f".to_owned()
    } else if RECURSIVE_REMOVE_METHODS.contains(&name) {
        "rm_rf".to_owned()
    } else {
        name.to_owned()
    }
}

fn requires_mode_keyword(
    node: Node<'_>,
    call_arguments: &[Argument<'_>],
    replacement: &str,
    context: &RuleContext<'_>,
) -> bool {
    node.field("receiver")
        .and_then(|receiver| short_constant_name(receiver, context))
        == Some("Dir")
        && replacement == "mkdir_p"
        && call_arguments.len() == 2
}
