use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::access_modifier::send_name;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, named_children, symbol_name};
use crate::rules::visibility::{node_visibility, siblings, statements};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let expected: Vec<String> = context.setting("ExpectedOrder").unwrap_or_default();
    if expected.is_empty() {
        return;
    }
    let categories = categories(context);
    // `on_class` and `on_sclass`: a module body is not checked.
    for class in context.nodes_of_any(&["class", "singleton_class"]) {
        let mut previous: Option<usize> = None;
        for node in class_elements(class) {
            let classification = classify(node, &categories, &expected, context);
            if is_ignored(node, &classification, &expected, context) {
                continue;
            }
            let Some(index) = expected.iter().position(|entry| *entry == classification) else {
                continue;
            };
            if let Some(previous_index) = previous
                && index < previous_index
            {
                offenses.push(offense(
                    node,
                    &classification,
                    &expected[previous_index],
                    &categories,
                    &expected,
                    context,
                ));
            }
            previous = Some(index);
        }
    }
}

/// `Categories`: which method names each category covers, in the order the configuration lists them.
fn categories(context: &RuleContext<'_>) -> Vec<(String, Vec<String>)> {
    let Some(serde_yaml_ng::Value::Mapping(configured)) = context.setting("Categories") else {
        return Vec::new();
    };
    configured
        .iter()
        .filter_map(|(key, value)| {
            let name = key.as_str()?.to_owned();
            let names = match value {
                serde_yaml_ng::Value::Sequence(items) => items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect(),
                serde_yaml_ng::Value::String(only) => vec![only.clone()],
                _ => Vec::new(),
            };
            Some((name, names))
        })
        .collect()
}

/// `class_elements`: the statements of the class body, with a `begin ... end` written among them
/// flattened into the list.
fn class_elements<'tree>(class: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(body) = class.field("body") else {
        return Vec::new();
    };
    let children = statements(body);
    match children.as_slice() {
        [] => Vec::new(),
        // A body of one statement is that statement, which is only flattened when it is a `kwbegin`.
        [only] if only.kind_str() != "begin" => vec![*only],
        several => flatten(several),
    }
}

fn flatten<'tree>(nodes: &[Node<'tree>]) -> Vec<Node<'tree>> {
    nodes
        .iter()
        .flat_map(|node| match node.kind_str() == "begin" {
            true => {
                let children = statements(*node);
                // `begin ... rescue ... end` is a `kwbegin` holding a single `rescue` upstream, so
                // nothing written inside it is an element of the class body.
                match children
                    .iter()
                    .find(|child| matches!(child.kind_str(), "rescue" | "ensure"))
                {
                    Some(clause) => vec![*clause],
                    None => flatten(&children),
                }
            }
            false => vec![*node],
        })
        .collect()
}

/// `classify`.
fn classify(
    node: Node<'_>,
    categories: &[(String, Vec<String>)],
    expected: &[String],
    context: &RuleContext<'_>,
) -> String {
    // A `block` is classified by the call it hangs off, which is the same node here. A bare
    // receiverless call reaches the grammar as an identifier, and upstream as a `send` all the same.
    if node.kind_str() == "call" || send_name(node, context).is_some() {
        return send_node_category(node, categories, expected, context);
    }
    let name = humanize(node, context);
    find_category(&name, categories).unwrap_or(name)
}

/// `find_send_node_category`.
fn send_node_category(
    node: Node<'_>,
    categories: &[(String, Vec<String>)],
    expected: &[String],
    context: &RuleContext<'_>,
) -> String {
    let Some(name) = node
        .field("method")
        .map(|method| context.source.node_text(method))
        .or_else(|| send_name(node, context))
        .map(str::to_owned)
    else {
        return "call".to_owned();
    };
    let key = find_category(&name, categories).unwrap_or_else(|| name.clone());
    let visibility_key = match is_def_modifier(node) {
        true => match name.ends_with("_class_method") {
            true => format!("{name}s"),
            false => format!("{name}_methods"),
        },
        false => format!("{}_{key}", node_visibility(node, context)),
    };
    match expected.contains(&visibility_key) {
        true => visibility_key,
        false => key,
    }
}

/// `humanize_node` together with `HUMANIZED_NODE_TYPE`.
fn humanize(node: Node<'_>, context: &RuleContext<'_>) -> String {
    match node.kind_str() {
        "method" => {
            let name = node
                .field("name")
                .map(|name| context.source.node_text(name))
                .unwrap_or_default();
            match name == "initialize" {
                true => "initializer".to_owned(),
                false => format!("{}_methods", node_visibility(node, context)),
            }
        }
        "singleton_method" => "public_class_methods".to_owned(),
        "singleton_class" => "class_singleton".to_owned(),
        kind if is_constant_assignment(node) || is_namespaced_constant_assignment(node) => {
            let _ = kind;
            "constants".to_owned()
        }
        kind => kind.to_owned(),
    }
}

/// `find_category`: the first category whose list of names holds this one.
fn find_category(name: &str, categories: &[(String, Vec<String>)]) -> Option<String> {
    categories
        .iter()
        .find(|(_, names)| names.iter().any(|entry| entry == name))
        .map(|(category, _)| category.clone())
}

/// `ignore?`.
fn is_ignored(
    node: Node<'_>,
    classification: &str,
    expected: &[String],
    context: &RuleContext<'_>,
) -> bool {
    classification.ends_with('=')
        || !expected.iter().any(|entry| entry == classification)
        || is_private_constant(node, context)
}

/// `private_constant?`: a constant the body marks with `private_constant`.
fn is_private_constant(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !is_constant_assignment(node) {
        return false;
    }
    let Some(name) = node
        .field("left")
        .filter(|left| left.kind_str() == "constant")
        .map(|left| context.source.node_text(left))
    else {
        return false;
    };
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    named_children(parent).into_iter().any(|sibling| {
        sibling.kind_str() == "call"
            && sibling
                .field("method")
                .is_some_and(|method| context.source.node_text(method) == "private_constant")
            && arguments(sibling).iter().any(|argument| {
                let node = argument.first();
                symbol_name(node, context) == Some(name)
                    || (crate::rules::send_node::is_string(node, context)
                        && crate::rules::send_node::string_text(node, context) == name)
            })
    })
}

/// A `casgn` whose constant was written under a namespace: `Foo::BAR = 1`. `HUMANIZED_NODE_TYPE`
/// classifies it as a constant all the same; only the private and dynamic checks insist on a bare
/// name.
fn is_namespaced_constant_assignment(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| left.kind_str() == "scope_resolution")
}

/// `casgn` with no namespace: `FOO = 1`, not `Foo::BAR = 1`.
fn is_constant_assignment(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| left.kind_str() == "constant")
}

/// `def_modifier?`: `private def foo`, and the chains of such calls.
fn is_def_modifier(node: Node<'_>) -> bool {
    if node.field("receiver").is_some() {
        return false;
    }
    let arguments = arguments(node);
    let [only] = arguments.as_slice() else {
        return false;
    };
    let argument = only.first();
    match argument.kind_str() {
        "method" | "singleton_method" => true,
        "call" => is_def_modifier(argument),
        _ => false,
    }
}

/// The offense, whose correction moves the element above the last sibling of a different category.
fn offense(
    node: Node<'_>,
    classification: &str,
    previous_category: &str,
    categories: &[(String, Vec<String>)],
    expected: &[String],
    context: &RuleContext<'_>,
) -> Offense {
    let message = format!("`{classification}` is supposed to appear before `{previous_category}`.");
    let offense = context.offense(message, node.byte_range());
    let Some(previous) = preceding_target(node, classification, categories, expected, context)
    else {
        return offense;
    };
    let current_range = with_comment(node, context);
    let previous_range = with_comment(previous, context);
    offense
        .corrections_anchored_at(current_range.clone())
        .corrected_by_all([
            Edit {
                start: previous_range.start,
                end: previous_range.start,
                replacement: context.source.slice(current_range.clone()).to_owned(),
                safe: true,
            },
            Edit {
                start: current_range.start,
                end: current_range.end,
                replacement: String::new(),
                safe: true,
            },
        ])
}

/// `node.left_siblings.reverse.find { |sibling| !ignore_for_autocorrect?(node, sibling) }`.
fn preceding_target<'tree>(
    node: Node<'tree>,
    classification: &str,
    categories: &[(String, Vec<String>)],
    expected: &[String],
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    if is_dynamic_constant(node, context) {
        return None;
    }
    siblings(node, context)?
        .into_iter()
        .take_while(|sibling| sibling.id() != node.id())
        .filter(|sibling| {
            let sibling_class = classify(*sibling, categories, expected, context);
            !is_ignored(*sibling, &sibling_class, expected, context)
                && sibling_class != classification
        })
        .last()
}

/// `dynamic_constant?`: a constant assigned the result of a call, other than a frozen literal.
fn is_dynamic_constant(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !is_constant_assignment(node) {
        return false;
    }
    let Some(right) = node.field("right") else {
        return false;
    };
    if !is_upstream_send(right, context) {
        return false;
    }
    let frozen = right
        .field("method")
        .is_some_and(|method| context.source.node_text(method) == "freeze")
        && right
            .field("receiver")
            .is_some_and(|receiver| is_basic_literal(receiver));
    !frozen
}

/// `node.send_type?`: what the grammar writes as a call, an operator or an index read. A call
/// carrying a block is a `block` upstream and not a `send`, and `&&` / `||` build an `and` or an `or`.
fn is_upstream_send(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "call" => node.field("block").is_none(),
        // A bare name reaches upstream as a receiverless `send` unless it read a local variable.
        // **The three keyword literals are nodes of their own there**: `__LINE__` is an `int`,
        // `__FILE__` a `str` and `__ENCODING__` its own type, so none of them is a `send`.
        "identifier" => {
            !matches!(
                context.source.node_text(node),
                "__LINE__" | "__FILE__" | "__ENCODING__"
            ) && send_name(node, context).is_some()
        }
        "element_reference" => true,
        "binary" => !matches!(
            operator_text(node, context),
            Some("&&") | Some("||") | Some("and") | Some("or")
        ),
        // A signed number folds into one numeric literal, and `defined?` is a type of its own.
        "unary" => {
            !matches!(operator_text(node, context), Some("defined?")) && !is_signed_number(node)
        }
        _ => false,
    }
}

fn operator_text<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named())
        .map(|child| context.source.node_text(child))
}

fn is_signed_number(node: Node<'_>) -> bool {
    node.field("operand").is_some_and(|operand| {
        matches!(
            operand.kind_str(),
            "integer" | "float" | "rational" | "complex"
        )
    })
}

/// `recursive_basic_literal?` for the receivers a frozen constant is written with.
fn is_basic_literal(node: Node<'_>) -> bool {
    match node.kind_str() {
        // The leaves a literal is written out of. A `hash_key_symbol` is the `sym` of a `key:` pair,
        // and the bare forms are the elements of a `%w[]` or `%i[]`.
        "integer" | "float" | "rational" | "complex" | "true" | "false" | "nil"
        | "simple_symbol" | "delimited_symbol" | "hash_key_symbol" | "character"
        | "string_content" | "escape_sequence" | "regex_options" | "heredoc_content"
        | "heredoc_end" | "heredoc_beginning" => true,
        // A heredoc's body is a sibling of the statement here, so it reaches this walk on its own;
        // an interpolation inside it makes the literal a `dstr` with a `begin` part, which is not
        // basic.
        "string" | "bare_string" | "bare_symbol" | "chained_string" | "regex" | "range" | "array"
        | "string_array" | "symbol_array" | "hash" | "pair" | "heredoc_body" => named_children(node)
            .iter()
            // A comment written inside a literal is no child of it upstream.
            .filter(|child| child.kind_str() != "comment")
            .all(|child| is_basic_literal(*child)),
        // A signed number is one numeric literal to upstream's parser rather than a call.
        "unary" => is_signed_number(node),
        _ => false,
    }
}

/// `source_range_with_comment`, as this cop overrides it: from the line break above the topmost
/// whole-line comment attached to the node through the end of the node's last line.
fn with_comment(node: Node<'_>, context: &RuleContext<'_>) -> Range<usize> {
    let first_line = context.source.line_column(node.start_byte()).0;
    let mut top = first_line;
    let mut line = first_line;
    while line > 1 {
        line -= 1;
        if comment_at_line(line, context).is_none() {
            break;
        }
        if context.source.line(line).trim_start().starts_with('#') {
            top = line;
        }
    }
    let start = context.source.line_start(top).saturating_sub(1);
    // `end_position_for`: a constant assigned a heredoc runs to just past the terminator.
    let end = match is_constant_assignment(node) {
        true => heredoc_end(node, context),
        false => None,
    };
    let end = end.unwrap_or_else(|| {
        let last_line = context.source.line_column(node.end_byte()).0;
        let range = context.source.line_range(last_line);
        range.end - usize::from(context.source.slice(range.clone()).ends_with('\n'))
    });
    start..end
}

fn comment_at_line(line: usize, context: &RuleContext<'_>) -> Option<Range<usize>> {
    context
        .comment_ranges()
        .iter()
        .find(|range| context.source.line_column(range.start).0 == line)
        .cloned()
}

/// The end of the heredoc a constant was assigned, one past its terminator.
fn heredoc_end(node: Node<'_>, context: &RuleContext<'_>) -> Option<usize> {
    let start = node.start_byte();
    let end = node.end_byte();
    let opener = context
        .nodes_of("heredoc_beginning")
        .position(|opener| opener.start_byte() >= start && opener.end_byte() <= end)?;
    let body = context.nodes_of("heredoc_body").nth(opener)?;
    Some(body.end_byte() + 1)
}
