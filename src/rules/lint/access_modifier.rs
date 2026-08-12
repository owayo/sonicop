//! Access modifiers as RuboCop's `SendNode` presents them.
//!
//! `private` written on a line of its own reaches tree-sitter as a bare `identifier`, and
//! `private()` as a `call` with an empty argument list; upstream's parser builds the same
//! `(send nil :private)` for both. Whether that call *is* a modifier then depends on where it
//! stands -- `macro?` asks that the call sit directly in a class, module or block body -- so the
//! two cops that reason about visibility share the scope walk here.

use tree_sitter::Node;

use crate::rules::RuleContext;

/// The name of the receiverless call `node` spells, when it is one that takes no arguments.
///
/// `bare_access_modifier?` needs both halves of that: a receiver makes the call a message to
/// something else, and an argument makes it a *non*-bare modifier that governs only what it names.
pub(super) fn bare_send_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind() {
        "identifier" => Some(context.source.node_text(node)),
        "call" => {
            if node.child_by_field_name("receiver").is_some() {
                return None;
            }
            let empty = node
                .child_by_field_name("arguments")
                .is_none_or(|arguments| arguments.named_child_count() == 0);
            let no_block = node.child_by_field_name("block").is_none();
            let method = node.child_by_field_name("method")?;
            (empty && no_block).then(|| context.source.node_text(method))
        }
        _ => None,
    }
}

/// `macro?`'s scope half: whether the call sits in a class, module or block body rather than
/// somewhere a `private` would be an ordinary message send.
///
/// The pattern walks outward through the "wrapper" nodes that hold statements -- `kwbegin`,
/// `begin`, any block, and the branches (but not the condition) of an `if` -- until it reaches
/// either the root or a class-like node. tree-sitter has wrappers upstream's parser does not
/// (`body_statement` around a class body, `then` around a branch), and those stand exactly where
/// the `begin` they collapse into would, so they pass through the same way.
pub(super) fn in_macro_scope(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    loop {
        let Some(parent) = current.parent() else {
            return true;
        };
        match parent.kind() {
            // The top level is `root?` whether the file holds one statement or many: with many,
            // the statement's parent is a `begin` that is itself the root.
            "program" => return true,
            "class" | "module" | "singleton_class" => return true,
            // `Class.new do ... end` and its siblings create a class body without a `class`
            // keyword, and count wherever they were written -- the pattern does not ask that the
            // constructor itself stand in a macro scope.
            _ if class_constructor(parent, context) => return true,
            // A body with a `rescue`, `else` or `ensure` clause is wrapped in a `rescue`/`ensure`
            // node upstream, and neither is a wrapper the pattern walks through.
            "body_statement" | "begin" if has_rescue_clause(parent) => return false,
            "body_statement"
            | "block_body"
            | "begin"
            | "parenthesized_statements"
            | "then"
            | "else"
            | "do_block"
            | "block"
            | "lambda" => current = parent,
            // Only the branches of a conditional are in scope; its condition is not.
            "if" | "unless" | "elsif" | "if_modifier" | "unless_modifier" | "conditional" => {
                if parent
                    .child_by_field_name("condition")
                    .is_some_and(|condition| condition.id() == current.id())
                {
                    return false;
                }
                current = parent;
            }
            _ => return false,
        }
    }
}

/// Whether the body holds a clause that makes it a `rescue` or `ensure` node upstream rather than
/// a plain `begin`.
fn has_rescue_clause(body: Node<'_>) -> bool {
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .any(|child| matches!(child.kind(), "rescue" | "else" | "ensure"))
}

/// `class_constructor?`: `Class.new`, `Module.new`, `Struct.new` or `Data.define`, with or without
/// the block that gives the class its body.
pub(super) fn class_constructor(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let call = match node.kind() {
        "do_block" | "block" => match node.parent() {
            Some(parent) if parent.kind() == "call" => parent,
            _ => return false,
        },
        "call" => node,
        _ => return false,
    };
    let (Some(receiver), Some(method)) = (
        call.child_by_field_name("receiver"),
        call.child_by_field_name("method"),
    ) else {
        return false;
    };
    let Some(constant) = top_level_constant_name(receiver, context) else {
        return false;
    };
    match context.source.node_text(method) {
        "new" => matches!(constant, "Class" | "Module" | "Struct"),
        "define" => constant == "Data",
        _ => false,
    }
}

/// The name of a constant reached from the top level, which is how `global_const?` spells
/// `(const {nil? cbase} :Name)`. `Foo::Class` names another constant entirely.
fn top_level_constant_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" if node.child_by_field_name("scope").is_none() => {
            Some(context.source.node_text(node.child_by_field_name("name")?))
        }
        _ => None,
    }
}

/// The statements of the body upstream reads as a `begin`: two or more of them, with no `rescue`,
/// `else` or `ensure` clause to wrap them in a node of its own. A body of one statement is that
/// statement upstream, and an empty one is nothing at all.
pub(super) fn begin_statements<'tree>(body: Node<'tree>) -> Option<Vec<Node<'tree>>> {
    let statements = statements(body)?;
    (statements.len() >= 2).then_some(statements)
}

/// The children `each_child_node` yields for a body node: its statements, unless a `rescue`,
/// `else` or `ensure` clause has made the statements children of a node one level further down.
pub(super) fn statements<'tree>(body: Node<'tree>) -> Option<Vec<Node<'tree>>> {
    let mut cursor = body.walk();
    let mut statements = Vec::new();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            // Comments never reach upstream's syntax tree.
            "comment" => {}
            "rescue" | "else" | "ensure" => return None,
            _ => statements.push(child),
        }
    }
    Some(statements)
}
