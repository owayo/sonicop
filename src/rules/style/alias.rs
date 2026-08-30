use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG_ALIAS: &str = "Use `alias_method` instead of `alias`.";

/// Where `self` points inside the expression, which decides whether `alias` can stand in.
#[derive(PartialEq, Eq)]
enum Scope {
    /// The innermost class or module block, so the keyword means what the call would.
    Lexical,
    /// A method or a plain block, where `alias` would bind somewhere else.
    Dynamic,
    /// An `instance_eval` block, where neither form can be swapped for the other.
    InstanceEval,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let prefer_alias = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "prefer_alias");

    for node in context.nodes_of("call") {
        check_alias_method(context, node, prefer_alias, offenses);
    }
    for node in context.nodes_of("alias") {
        check_alias(context, node, prefer_alias, offenses);
    }
}

/// `on_send`: an `alias_method` call that the keyword could replace.
fn check_alias_method(
    context: &RuleContext<'_>,
    node: Node<'_>,
    prefer_alias: bool,
    offenses: &mut Vec<Offense>,
) {
    let Some(selector) = node.field("method") else {
        return;
    };
    if node.field("receiver").is_some()
        || context.source.node_text(selector) != "alias_method"
        || !prefer_alias
    {
        return;
    }
    let arguments = node
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    // `alias_keyword_possible?`, the argument count, and the positions where a keyword cannot go.
    if scope(context, node) == Scope::Dynamic
        || arguments.len() != 2
        || !arguments.iter().all(|argument| is_symbol(*argument))
        || value_used(node)
    {
        return;
    }

    let replacement = format!(
        "alias {} {}",
        identifier(context, arguments[0]),
        identifier(context, arguments[1])
    );
    offenses.push(
        context
            .offense(
                format!(
                    "Use `alias` instead of `alias_method` {}.",
                    lexical_scope(node)
                ),
                selector.byte_range(),
            )
            .corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }),
    );
}

/// `on_alias`: the keyword used where the call belongs, or written with needless colons.
fn check_alias(
    context: &RuleContext<'_>,
    node: Node<'_>,
    prefer_alias: bool,
    offenses: &mut Vec<Offense>,
) {
    let (Some(new), Some(old)) = (
        node.field("name"),
        node.field("alias"),
    ) else {
        return;
    };
    // `alias_method_possible?`: a global variable has no method form, and inside a method or an
    // `instance_eval` block the call would not mean the same thing.
    let scope = scope(context, node);
    if scope == Scope::InstanceEval
        || [new, old]
            .iter()
            .any(|argument| argument.kind_str() == "global_variable")
        || enclosed_by_method(node)
    {
        return;
    }

    if scope == Scope::Dynamic || !prefer_alias {
        let Some(keyword) = node.child(0) else {
            return;
        };
        offenses.push(
            context
                .offense(MSG_ALIAS, keyword.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!(
                        "alias_method {}, {}",
                        identifier(context, new),
                        identifier(context, old)
                    ),
                    safe: true,
                }),
        );
        return;
    }

    // `bareword?`: a name already written without a colon needs nothing done to it.
    if [new, old]
        .iter()
        .any(|argument| bareword(context, *argument))
    {
        return;
    }
    let existing = format!(
        "{} {}",
        context.source.node_text(new),
        context.source.node_text(old)
    );
    let preferred = format!("{} {}", trimmed(context, new), trimmed(context, old));
    offenses.push(
        context
            .offense(
                format!("Use `alias {preferred}` instead of `alias {existing}`."),
                new.start_byte()..old.end_byte(),
            )
            .corrected_by_all([
                Edit {
                    start: new.start_byte(),
                    end: new.end_byte(),
                    replacement: trimmed(context, new).to_owned(),
                    safe: true,
                },
                Edit {
                    start: old.start_byte(),
                    end: old.end_byte(),
                    replacement: trimmed(context, old).to_owned(),
                    safe: true,
                },
            ]),
    );
}

/// `identifier`: a symbol is written back as one, anything else keeps its source.
fn identifier(context: &RuleContext<'_>, node: Node<'_>) -> String {
    match node.kind_str() {
        "simple_symbol" => context.source.node_text(node).to_owned(),
        "identifier" | "constant" | "operator" => {
            format!(":{}", context.source.node_text(node))
        }
        _ => context.source.node_text(node).to_owned(),
    }
}

fn trimmed<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> &'a str {
    let text = context.source.node_text(node);
    text.strip_prefix(':').unwrap_or(text)
}

fn bareword(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    !context.source.node_text(node).starts_with(':') || node.kind_str() == "delimited_symbol"
}

fn is_symbol(node: Node<'_>) -> bool {
    match node.kind_str() {
        "simple_symbol" => true,
        // `:"a"` is a `sym` too, unless an interpolation makes it a `dsym`.
        "delimited_symbol" => {
            let mut cursor = node.walk();
            !node.named_children(&mut cursor)
                .any(|child| child.kind_str() == "interpolation")
        }
        _ => false,
    }
}

/// `alias_method_value_used?`: the call's result is read, where a keyword statement cannot go.
fn value_used(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| match parent.kind_str() {
        "argument_list" => true,
        "assignment" | "operator_assignment" => parent
            .field("right")
            .is_some_and(|right| right.id() == node.id()),
        _ => false,
    })
}

/// `scope_type`.
fn scope(context: &RuleContext<'_>, node: Node<'_>) -> Scope {
    let mut current = node.parent_of(context);
    while let Some(parent) = current {
        match parent.kind_str() {
            // `sclass` is none of the types `scope_type` names, so a `class << self` block does
            // not stop the walk: what counts is what encloses it.
            "class" | "module" => return Scope::Lexical,
            "method" | "singleton_method" => return Scope::Dynamic,
            "block" | "do_block" => {
                return match instance_eval_block(context, parent) {
                    true => Scope::InstanceEval,
                    false => Scope::Dynamic,
                };
            }
            _ => {}
        }
        current = parent.parent_of(context);
    }
    Scope::Lexical
}

fn instance_eval_block(context: &RuleContext<'_>, block: Node<'_>) -> bool {
    block
        .parent_of(context)
        .filter(|call| call.kind_str() == "call")
        .and_then(|call| call.field("method"))
        .is_some_and(|method| context.source.node_text(method) == "instance_eval")
}

/// `node.each_ancestor(:def).none?`.
fn enclosed_by_method(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        // `node.each_ancestor(:def).none?` lists `:def` alone -- a `defs` (`def obj.name`) does
        // not stop `alias_method` from being possible, it only makes the scope dynamic.
        if parent.kind_str() == "method" {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// `lexical_scope_type`.
fn lexical_scope(node: Node<'_>) -> &'static str {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind_str() {
            "class" => return "in a class body",
            "module" => return "in a module body",
            _ => {}
        }
        current = parent.parent();
    }
    "at the top level"
}
