use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use super::support::{Variables, last_named_child, spurious_assignment_list};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

// `[[:digit:][:upper:]_]` upstream, and Ruby's POSIX classes are Unicode-aware, so `Ä` counts as
// upper case. Rust's `[[:upper:]]` would only accept ASCII.
static SCREAMING_SNAKE_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\d\p{Uppercase}_]+$").unwrap());

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let variables = context.variable_roles();
    // `rescue => Foo` and `for Foo in bar` are `casgn` nodes upstream just as much as `Foo = bar`
    // is, and neither carries a value, so neither can be excused by what it was assigned.
    for node in context.nodes_of("exception_variable") {
        if let Some(target) = node.named_child(0) {
            report(context, offenses, target, None, variables);
        }
    }
    for node in context.nodes_of("for") {
        if let Some(target) = node.field("pattern") {
            report(context, offenses, target, None, variables);
        }
    }
    for node in context.nodes_of_any(&["assignment", "operator_assignment"]) {
        let Some(left) = node.field("left") else {
            continue;
        };
        let right = node.field("right");
        // `on_casgn` reads the value through a surrounding `or_asgn`, so `FOO ||= bar` is judged
        // by `bar`. Under any other operator the casgn keeps no expression of its own and the
        // value counts as unknown -- which is not the same as allowed.
        let value = if node.kind_str() == "operator_assignment" {
            right.filter(|_| operator(node) == Some("||="))
        } else {
            right
        };
        if left.kind_str() == "left_assignment_list" && spurious_assignment_list(left) {
            // The grammar swallowed the items written before the assignment; only the last of them
            // is really assigned to, and it keeps the value on the right.
            if let Some(target) = last_named_child(left) {
                report(context, offenses, target, value, variables);
            }
        } else if left.kind_str() == "left_assignment_list" {
            // Every target of a multiple assignment is a casgn without an expression, so none of
            // them can be excused by what stands on the right.
            let mut targets = Vec::new();
            collect_targets(left, &mut targets);
            for target in targets {
                report(context, offenses, target, None, variables);
            }
        } else {
            report(context, offenses, left, value, variables);
        }
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    target: Node<'_>,
    value: Option<Node<'_>>,
    variables: &Variables,
) {
    let Some(name_node) = constant_name(target) else {
        return;
    };
    if allowed_assignment(value, variables) {
        return;
    }
    if SCREAMING_SNAKE_CASE.is_match(context.source.node_text(name_node)) {
        return;
    }
    offenses.push(context.offense(
        "Use SCREAMING_SNAKE_CASE for constants.",
        name_node.byte_range(),
    ));
}

/// The part of an assignment target that `casgn.loc.name` covers: `A::B = 1` and `::B = 1` both
/// report only the `B`.
fn constant_name(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind_str() {
        "constant" => Some(node),
        "scope_resolution" => node
            .field("name")
            .filter(|name| name.kind_str() == "constant"),
        _ => None,
    }
}

fn collect_targets<'tree>(node: Node<'tree>, targets: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind_str() {
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                collect_targets(child, targets);
            }
            _ => targets.push(child),
        }
    }
}

/// The shapes `allowed_assignment?` lets through. RuboCop only judges a constant's spelling once
/// it can tell the value is not a class or module, because `SomeClass = SomeOtherClass` is a
/// perfectly good CamelCase constant.
fn allowed_assignment(value: Option<Node<'_>>, variables: &Variables) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value_kind(value, variables) {
        Value::ClassLike => true,
        Value::Call { receiver } => receiver.is_none_or(|receiver| !literal_receiver(receiver)),
        Value::Conditional => branches(value).into_iter().any(is_constant),
        Value::Other => false,
    }
}

enum Value<'tree> {
    /// `block`, `const` and `casgn`: each of those can just as well evaluate to a class.
    ClassLike,
    /// A `send`. The receiver decides, because a call on a literal cannot return a class.
    Call {
        receiver: Option<Node<'tree>>,
    },
    Conditional,
    Other,
}

fn value_kind<'tree>(node: Node<'tree>, variables: &Variables) -> Value<'tree> {
    match node.kind_str() {
        "constant" => Value::ClassLike,
        "scope_resolution" => match node.field("name") {
            Some(name) if name.kind_str() == "constant" => Value::ClassLike,
            _ => Value::Call {
                receiver: node.field("scope"),
            },
        },
        // `A = B = Class.new` chains casgn nodes, and the inner one excuses the outer.
        "assignment" => match node.field("left") {
            Some(left) if constant_name(left).is_some() => Value::ClassLike,
            _ => Value::Other,
        },
        // A method call carrying a block is a `block` node upstream, not a `send`.
        "call" if node.field("block").is_some() => Value::ClassLike,
        "lambda" => Value::ClassLike,
        "call" => Value::Call {
            receiver: node.field("receiver"),
        },
        // A bare identifier is a receiverless call unless the parser resolved it to a local.
        "identifier" => {
            if variables.is_reference(node) {
                Value::Other
            } else {
                Value::Call { receiver: None }
            }
        }
        "binary" => match operator(node) {
            // `&&` and `||` build their own node types upstream; they are not method calls.
            Some("&&" | "||" | "and" | "or") => Value::Other,
            _ => Value::Call {
                receiver: node.field("left"),
            },
        },
        "unary" => match operator(node) {
            Some("defined?") => Value::Other,
            // A signed number reaches the parser as one numeric literal, not a call to `-@`.
            Some("-" | "+") if node.field("operand").is_some_and(numeric) => {
                Value::Other
            }
            _ => Value::Call {
                receiver: node.field("operand"),
            },
        },
        "element_reference" => Value::Call {
            receiver: node.field("object"),
        },
        "if" | "unless" | "conditional" => Value::Conditional,
        _ => Value::Other,
    }
}

/// `literal_receiver?`. The `(send (begin literal?) ...)` half of the pattern is why a
/// parenthesised literal counts too.
fn literal_receiver(node: Node<'_>) -> bool {
    if node.kind_str() == "parenthesized_statements" {
        return node.named_child_count() == 1 && node.named_child(0).is_some_and(is_literal);
    }
    is_literal(node)
}

fn is_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "integer"
            | "float"
            | "rational"
            | "complex"
            | "string"
            | "bare_string"
            | "chained_string"
            | "subshell"
            | "heredoc_beginning"
            | "character"
            | "regex"
            | "simple_symbol"
            | "delimited_symbol"
            | "bare_symbol"
            | "hash_key_symbol"
            | "array"
            | "string_array"
            | "symbol_array"
            | "hash"
            | "range"
            | "true"
            | "false"
            | "nil"
    ) || (node.kind_str() == "unary"
        && matches!(operator(node), Some("-" | "+"))
        && node.field("operand").is_some_and(numeric))
}

fn numeric(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "integer" | "float" | "rational" | "complex")
}

fn is_constant(node: Node<'_>) -> bool {
    constant_name(node).is_some()
}

/// The branch values of a conditional, following `elsif` chains the way `IfNode#branches` does.
/// A branch holding more than one statement is a `begin` upstream and so can never be a constant,
/// which is why only a lone child counts.
fn branches(node: Node<'_>) -> Vec<Node<'_>> {
    if node.kind_str() == "conditional" {
        return node
            .field("consequence")
            .into_iter()
            .chain(node.field("alternative"))
            .collect();
    }
    let mut branches = Vec::new();
    let mut current = Some(node);
    while let Some(node) = current.take() {
        branches.extend(node.field("consequence").and_then(only_child));
        match node.field("alternative") {
            Some(alternative) if alternative.kind_str() == "elsif" => current = Some(alternative),
            Some(alternative) => branches.extend(only_child(alternative)),
            None => {}
        }
    }
    branches
}

fn only_child(node: Node<'_>) -> Option<Node<'_>> {
    if node.named_child_count() == 1 {
        node.named_child(0)
    } else {
        None
    }
}

/// The operator token of a node whose operands tree-sitter names but whose operator it does not.
fn operator(node: Node<'_>) -> Option<&'static str> {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        let child = cursor.node();
        if !child.is_named() {
            return Some(child.kind_str());
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}
