use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send};

use super::node_equality::identical;
use super::statements::statements;

const METHODS: [&str; 2] = ["require", "require_relative"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `@required` is keyed by the parent node, so two requires collide only when the parser would
    // have hung them off the same node -- which is not the same as sharing a line of source.
    let mut seen: Vec<(Parent, Node<'_>, &str)> = Vec::new();
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        let name = context.source.node_text(method);
        if !METHODS.contains(&name) || !is_plain_send(node, context) {
            continue;
        }
        // `{nil? (const _ :Kernel)}`: any scope may qualify `Kernel` here, unlike the cops that
        // insist on the top-level one.
        if node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| {
                !matches!(receiver.kind(), "constant" | "scope_resolution")
                    || short_name(receiver, context) != Some("Kernel")
            })
        {
            continue;
        }
        let arguments = arguments(node);
        let [argument] = arguments.as_slice() else {
            continue;
        };
        let argument = argument.first();
        let parent = parent_of(node);
        if seen.iter().any(|(group, earlier, earlier_name)| {
            *group == parent && *earlier_name == name && identical(*earlier, argument, context)
        }) {
            offenses.push(
                context
                    .offense(format!("Duplicate `{name}` detected."), node.byte_range())
                    .corrected_by(whole_lines(node, context)),
            );
        } else {
            seen.push((parent, argument, name));
        }
    }
}

fn short_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" => node
            .child_by_field_name("name")
            .map(|name| context.source.node_text(name)),
        _ => None,
    }
}

/// `range_by_whole_lines(node.source_range, include_final_newline: true)`.
fn whole_lines(node: Node<'_>, context: &RuleContext<'_>) -> Edit {
    let text = context.source.text();
    let start = text[..node.start_byte()]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = text[node.end_byte()..]
        .find('\n')
        .map_or(text.len(), |offset| node.end_byte() + offset + 1);
    Edit {
        start,
        end,
        replacement: String::new(),
        safe: false,
    }
}

/// Which node upstream's parser would have made the statement's parent.
///
/// A sequence of two or more statements is a `begin` of its own; one statement alone *is* the body,
/// so its parent is whatever the body belongs to. `begin ... end` and `(...)` are nodes however
/// little they hold, and a `rescue` or an `ensure` clause puts a node of its own between the body
/// and the definition it is written in.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Parent {
    /// The `(begin ...)` a sequence of statements builds, named by the node holding them.
    Sequence(usize),
    /// A node of the tree, reached because the branch holds exactly one statement.
    Node(usize),
    /// The `(rescue ...)` or `(ensure ...)` a body's clauses build, named by that body.
    Rescue(usize),
    Ensure(usize),
    /// The whole file, which is no node at all when it holds one statement.
    Root,
}

fn parent_of(node: Node<'_>) -> Parent {
    let Some(container) = node.parent() else {
        return Parent::Root;
    };
    let count = statements(container).len();
    match container.kind() {
        // `kwbegin` holds its statements directly, so it is their parent however many there are --
        // unless a clause splits them off into a node of its own.
        "begin" if !split_body(container) => Parent::Node(container.id()),
        "begin" | "body_statement" if count > 1 => Parent::Sequence(container.id()),
        "begin" | "body_statement" => body_parent(container),
        // `(...)` is a `begin` whatever it holds.
        "parenthesized_statements" => Parent::Node(container.id()),
        "program" if count > 1 => Parent::Sequence(container.id()),
        "program" => Parent::Root,
        _ if count > 1 => Parent::Sequence(container.id()),
        "ensure" => container
            .parent()
            .map_or(Parent::Root, |body| Parent::Ensure(body.id())),
        // A rescue's `else` belongs to the `rescue` node; an `if`'s belongs to the `if`.
        "else"
            if container
                .parent()
                .is_some_and(|body| matches!(body.kind(), "begin" | "body_statement")) =>
        {
            Parent::Rescue(container.parent().expect("checked").id())
        }
        _ => container
            .parent()
            .map_or(Parent::Root, |owner| Parent::Node(owner.id())),
    }
}

/// The node a body's own statements hang off when there is only one of them: the `ensure` or the
/// `rescue` a clause introduced, or the definition the body belongs to.
fn body_parent(container: Node<'_>) -> Parent {
    let mut cursor = container.walk();
    let mut rescue = false;
    for child in container.named_children(&mut cursor) {
        match child.kind() {
            "ensure" => return Parent::Ensure(container.id()),
            "rescue" | "else" => rescue = true,
            _ => {}
        }
    }
    if rescue {
        return Parent::Rescue(container.id());
    }
    container
        .parent()
        .map_or(Parent::Root, |owner| Parent::Node(owner.id()))
}

/// Whether a clause splits the body into parts, which is what puts a node between it and the
/// statements it holds.
fn split_body(container: Node<'_>) -> bool {
    let mut cursor = container.walk();
    container
        .named_children(&mut cursor)
        .any(|child| matches!(child.kind(), "rescue" | "else" | "ensure"))
}
