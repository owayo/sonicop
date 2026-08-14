use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{is_plain_send, named_children, top_level_constant};

use super::literals::literal_type;
use super::statements::statements;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Replace splat expansion with comma separated values.";
const ARRAY_PARAM_MSG: &str = "Pass array contents as separate arguments.";

/// `ASSIGNMENT_TYPES`: the writes that may keep an `Array.new` expansion. `masgn` and `op_asgn`
/// are deliberately missing upstream.
const ASSIGNMENT_TARGETS: &[&str] = &[
    "identifier",
    "instance_variable",
    "class_variable",
    "global_variable",
    "constant",
    "scope_resolution",
];

/// The literal types the splat pattern lists.
const EXPANDABLE: &[&str] = &["str", "dstr", "int", "float", "array"];

/// Where the splat sits, in the terms upstream's parser puts it in. tree-sitter writes several of
/// these without the `array` the parser wraps around them.
enum Position {
    /// `node.parent.call_type?`.
    Argument,
    /// An `array` written with brackets.
    BracketedArray,
    /// The `array` the parser builds around a right-hand side or a rescue list.
    ImplicitArray,
    /// `(when (splat ...) ...)`.
    When,
    /// `return`, `break`, `next` and `yield`, which hold the splat directly.
    Keyword,
    Other,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_percent = context
        .setting::<bool>("AllowPercentLiteralArrayArgument")
        .unwrap_or(true);
    for node in context.nodes_of("splat_argument") {
        let Some(expanded) = named_children(node).into_iter().next() else {
            continue;
        };
        let (position, grandparent) = position_of(node);
        let is_array_new = array_new(expanded, context);
        if is_array_new {
            if array_new_inside_array_literal(node, &position) {
                continue;
            }
            if grandparent.is_some_and(|grandparent| !is_assignment(grandparent)) {
                continue;
            }
        } else {
            let Some(kind) = literal_type(expanded, context) else {
                continue;
            };
            if !EXPANDABLE.contains(&kind) {
                continue;
            }
            if kind == "array" && named_children(expanded).is_empty() {
                continue;
            }
        }
        let array_splat = literal_type(expanded, context) == Some("array");
        let in_collection = matches!(position, Position::Argument | Position::BracketedArray);
        let message = if array_splat && in_collection {
            if allow_percent
                && matches!(position, Position::Argument)
                && matches!(expanded.kind_str(), "string_array" | "symbol_array")
            {
                continue;
            }
            ARRAY_PARAM_MSG
        } else {
            MSG
        };
        let edit = replacement(
            node,
            expanded,
            &position,
            is_array_new,
            grandparent,
            context,
        );
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(edit),
        );
    }
}

/// Where the splat was written, and the node upstream would call its grandparent.
fn position_of<'tree>(node: Node<'tree>) -> (Position, Option<Node<'tree>>) {
    let Some(parent) = node.parent() else {
        return (Position::Other, None);
    };
    match parent.kind_str() {
        "argument_list" => match parent.parent() {
            Some(call) if call.kind_str() == "call" => (Position::Argument, upstream_parent(call)),
            Some(keyword) => (Position::Keyword, upstream_parent(keyword)),
            None => (Position::Other, None),
        },
        // `Hash[*list]` is a `send :[]` upstream, so its splat is a method argument.
        "element_reference" => (Position::Argument, upstream_parent(parent)),
        "array" => (Position::BracketedArray, upstream_parent(parent)),
        // `x = *foo` and `x = 1, *foo` are one `array` upstream, whose parent is the write.
        "assignment" | "operator_assignment" => (Position::ImplicitArray, Some(parent)),
        "right_assignment_list" => (Position::ImplicitArray, upstream_parent(parent)),
        // `rescue *ERRORS` lists its exceptions in an `array` whose parent is the `resbody`.
        "exceptions" => (Position::ImplicitArray, upstream_parent(parent)),
        "pattern" if parent.parent().is_some_and(|when| when.kind_str() == "when") => (
            Position::When,
            parent.parent().and_then(upstream_parent),
        ),
        _ => (Position::Other, upstream_parent(parent)),
    }
}

/// `node.parent`, which is `nil` for the root of the file.
///
/// A file holding one statement *is* that statement upstream, so nothing above it exists; a file
/// holding several is a `begin` around them, which `program` stands for.
fn upstream_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    if parent.kind_str() == "program" && statements(parent).len() <= 1 {
        return None;
    }
    Some(parent)
}

fn is_assignment(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| ASSIGNMENT_TARGETS.contains(&left.kind_str()))
}

/// `array_new?`: `Array.new(...)`, with or without a block.
fn array_new(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        && is_plain_send(node, context)
        && node
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "new")
        && node
            .field("receiver")
            .is_some_and(|receiver| top_level_constant(receiver, "Array", context))
}

/// `array_new_inside_array_literal?`: an `Array.new` beside other elements stays as it is.
fn array_new_inside_array_literal(node: Node<'_>, position: &Position) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match position {
        Position::BracketedArray => elements(parent).len() > 1,
        Position::ImplicitArray => {
            parent.kind_str() == "right_assignment_list" && elements(parent).len() > 1
        }
        _ => false,
    }
}

fn elements<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect()
}

/// `replacement_range_and_content`.
fn replacement(
    node: Node<'_>,
    expanded: Node<'_>,
    position: &Position,
    is_array_new: bool,
    grandparent: Option<Node<'_>>,
    context: &RuleContext<'_>,
) -> Edit {
    let source = |range: Range<usize>| context.source.slice(range).to_owned();
    if is_array_new {
        // The `array` upstream wraps a right-hand side in spans exactly what the splat does, so
        // only a bracketed literal widens the range.
        let range = match position {
            Position::BracketedArray => node
                .parent_of(context)
                .map_or(node.byte_range(), |parent| parent.byte_range()),
            _ => node.byte_range(),
        };
        return Edit {
            start: range.start,
            end: range.end,
            replacement: source(expanded.byte_range()),
            safe: true,
        };
    }
    if literal_type(expanded, context) != Some("array") {
        let inner = source(expanded.byte_range());
        let replacement = if matches!(position, Position::ImplicitArray) {
            format!("[{inner}]")
        } else {
            inner
        };
        return Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement,
            safe: true,
        };
    }
    if redundant_brackets(position, grandparent) {
        return Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: remove_brackets(expanded, context),
            safe: true,
        };
    }
    // `node.loc.operator`: just the `*`.
    Edit {
        start: node.start_byte(),
        end: node.start_byte() + 1,
        replacement: String::new(),
        safe: true,
    }
}

fn redundant_brackets(position: &Position, grandparent: Option<Node<'_>>) -> bool {
    matches!(
        position,
        Position::When | Position::Argument | Position::BracketedArray
    ) || grandparent.is_some_and(|grandparent| grandparent.kind_str() == "rescue")
}

/// `remove_brackets`: the elements as they were written, in the syntax the literal used.
fn remove_brackets(array: Node<'_>, context: &RuleContext<'_>) -> String {
    let items: Vec<&str> = elements(array)
        .into_iter()
        .map(|element| context.source.node_text(element))
        .collect();
    let opening = array
        .child(0)
        .map_or("", |child| context.source.node_text(child));
    if opening.starts_with("%w") {
        format!("'{}'", items.join("', '"))
    } else if opening.starts_with("%W") {
        format!("\"{}\"", items.join("\", \""))
    } else if opening.starts_with("%i") {
        format!(":{}", items.join(", :"))
    } else if opening.starts_with("%I") {
        format!(":\"{}\"", items.join("\", :\""))
    } else {
        items.join(", ")
    }
}
