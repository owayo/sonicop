use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Convert `if-elsif` to `case-when`.";

/// `Node::LITERALS`, as the grammar spells them.
const LITERALS: &[&str] = &[
    "string",
    "chained_string",
    "heredoc_beginning",
    "subshell",
    "integer",
    "float",
    "simple_symbol",
    "delimited_symbol",
    "hash_key_symbol",
    "array",
    "hash",
    "regex",
    "true",
    "false",
    "nil",
    "range",
    "complex",
    "rational",
    "character",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let minimum: usize = context
        .setting::<i64>("MinBranchesCount")
        .filter(|count| *count > 0)
        .unwrap_or(3) as usize;

    for node in context.nodes_of("if") {
        let branches = branch_conditions(node);
        // `elsif_conditional?` and `min_branches_count?`: the chain has to be an `if`/`elsif` one
        // with enough arms to be worth a `case`.
        if branches.len() < 2 || branches.len() < minimum {
            continue;
        }
        let Some(target) = find_target(context, branches[0]) else {
            continue;
        };
        let target = context.source.node_text(target).to_owned();
        let mut per_branch = Vec::new();
        for condition in &branches {
            if regexp_with_working_captures(context, *condition) {
                per_branch.clear();
                break;
            }
            let mut conditions = Vec::new();
            if !collect_conditions(context, *condition, &target, &mut conditions) {
                per_branch.clear();
                break;
            }
            per_branch.push(conditions);
        }
        if per_branch.len() != branches.len() {
            continue;
        }
        let indent = " ".repeat(node.start_position().column);
        let mut edits = vec![Edit {
            start: node.start_byte(),
            end: node.start_byte(),
            replacement: format!("case {target}\n{indent}"),
            safe: true,
        }];
        for (condition, conditions) in branches.iter().zip(&per_branch) {
            let Some(keyword) = condition.parent().and_then(|parent| parent.child(0)) else {
                continue;
            };
            edits.push(Edit {
                start: keyword.start_byte(),
                end: condition.end_byte(),
                replacement: format!("when {}", conditions.join(", ")),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `branch_conditions`: the condition of every `if`/`elsif` in the chain.
fn branch_conditions<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let mut conditions = Vec::new();
    let mut current = Some(node);
    while let Some(branch) = current {
        if !matches!(branch.kind_str(), "if" | "elsif") {
            break;
        }
        let Some(condition) = branch.field("condition") else {
            break;
        };
        conditions.push(condition);
        current = branch.field("alternative");
    }
    conditions
}

/// `find_target`: what the `case` would be written about.
fn find_target<'t>(context: &RuleContext<'_>, node: Node<'t>) -> Option<Node<'t>> {
    match node.kind_str() {
        // `find_target` reaches for the *first* statement here, unlike `deparenthesize`.
        "parenthesized_statements" => {
            find_target(context, *super::nodes::children(node).first()?)
        }
        _ => {
            let call = Call::new(context, node)?;
            if call.is_or() {
                return find_target(context, call.receiver?);
            }
            match call.method {
                "is_a?" => call.receiver,
                "==" | "eql?" | "equal?" => find_target_in_equality(context, &call),
                "===" => call.argument,
                "include?" | "cover?" => {
                    let receiver = deparenthesize(call.receiver?);
                    (receiver.kind_str() == "range").then_some(call.argument)?
                }
                "match" | "match?" | "=~" => find_target_in_match(&call),
                _ => None,
            }
        }
    }
}

fn find_target_in_equality<'t>(context: &RuleContext<'_>, call: &Call<'_, 't>) -> Option<Node<'t>> {
    let (receiver, argument) = (call.receiver?, call.argument?);
    if is_literal(argument) || is_const_reference(context, argument) {
        return Some(receiver);
    }
    (is_literal(receiver) || is_const_reference(context, receiver)).then_some(argument)
}

fn find_target_in_match<'t>(call: &Call<'_, 't>) -> Option<Node<'t>> {
    let receiver = call.receiver?;
    if receiver.kind_str() == "regex" {
        return call.argument;
    }
    call.argument
        .filter(|argument| argument.kind_str() == "regex")
        .map(|_| receiver)
}

/// `collect_conditions`: the `when` list one branch turns into, or nothing when the branch says
/// something the `case` could not.
fn collect_conditions(
    context: &RuleContext<'_>,
    node: Node<'_>,
    target: &str,
    conditions: &mut Vec<String>,
) -> bool {
    if node.kind_str() == "parenthesized_statements" {
        let Some(inner) = super::nodes::children(node).into_iter().next() else {
            return false;
        };
        return collect_conditions(context, inner, target, conditions);
    }
    let Some(call) = Call::new(context, node) else {
        return false;
    };
    if call.is_or() {
        let (Some(left), Some(right)) = (call.receiver, call.argument) else {
            return false;
        };
        return collect_conditions(context, left, target, conditions)
            && collect_conditions(context, right, target, conditions);
    }
    let condition = match call.method {
        "is_a?" => call
            .receiver
            .filter(|receiver| context.source.node_text(*receiver) == target)
            .and(call.argument),
        "==" | "eql?" | "equal?" => condition_from_binary(context, &call, target)
            .filter(|condition| !is_class_reference(context, *condition)),
        "=~" | "match" | "match?" => condition_from_binary(context, &call, target),
        "===" => call
            .argument
            .filter(|argument| context.source.node_text(*argument) == target)
            .and(call.receiver),
        "include?" | "cover?" => call
            .argument
            .filter(|argument| context.source.node_text(*argument) == target)
            .and(call.receiver)
            .map(deparenthesize)
            .filter(|receiver| receiver.kind_str() == "range"),
        _ => None,
    };
    match condition {
        Some(condition) => {
            conditions.push(context.source.node_text(condition).to_owned());
            true
        }
        None => false,
    }
}

/// `condition_from_binary_op`: whichever side is not the target.
fn condition_from_binary<'t>(
    context: &RuleContext<'_>,
    call: &Call<'_, 't>,
    target: &str,
) -> Option<Node<'t>> {
    let (left, right) = (
        deparenthesize(call.receiver?),
        deparenthesize(call.argument?),
    );
    if context.source.node_text(left) == target {
        return Some(right);
    }
    (context.source.node_text(right) == target).then_some(left)
}

/// A `send` as the pattern reads it, however the grammar spells the message.
struct Call<'a, 't> {
    method: &'a str,
    receiver: Option<Node<'t>>,
    argument: Option<Node<'t>>,
}

impl<'a, 't> Call<'a, 't> {
    fn new(context: &'a RuleContext<'_>, node: Node<'t>) -> Option<Self> {
        match node.kind_str() {
            "binary" => Some(Self {
                method: context
                    .source
                    .node_text(node.field("operator")?),
                receiver: node.field("left"),
                argument: node.field("right"),
            }),
            "call" => {
                let arguments = node
                    .field("arguments")
                    .map(super::nodes::children)
                    .unwrap_or_default();
                Some(Self {
                    method: context
                        .source
                        .node_text(node.field("method")?),
                    receiver: node.field("receiver"),
                    argument: arguments.first().copied(),
                })
            }
            _ => None,
        }
    }

    fn is_or(&self) -> bool {
        matches!(self.method, "||" | "or")
    }
}

/// `regexp_with_working_captures?`: a named capture the `when` form would stop binding.
fn regexp_with_working_captures(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(call) = Call::new(context, node) else {
        return false;
    };
    let sides: Vec<Node<'_>> = [call.receiver, call.argument]
        .into_iter()
        .flatten()
        .collect();
    match call.method {
        "=~" => sides
            .first()
            .is_some_and(|left| has_named_captures(context, *left)),
        "match" => sides.iter().any(|side| has_named_captures(context, *side)),
        _ => false,
    }
}

fn has_named_captures(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind_str() != "regex" {
        return false;
    }
    let source = context.source.node_text(node);
    let bytes = source.as_bytes();
    source.match_indices("(?").any(|(index, _)| {
        // `(?<=` and `(?<!` are look-behind, not a capture.
        (bytes.get(index + 2) == Some(&b'<')
            && !matches!(bytes.get(index + 3), Some(b'=') | Some(b'!')))
            || bytes.get(index + 2) == Some(&b'\'')
    })
}

fn deparenthesize<'t>(mut node: Node<'t>) -> Node<'t> {
    while node.kind_str() == "parenthesized_statements" {
        match super::nodes::children(node).last() {
            Some(last) => node = *last,
            None => break,
        }
    }
    node
}

fn is_literal(node: Node<'_>) -> bool {
    LITERALS.contains(&node.kind_str())
}

/// `const_reference?`: a constant whose name is all upper case, which reads as a value rather than
/// a class.
fn is_const_reference(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match constant_name(context, node) {
        Some(name) => name.len() > 1 && name == name.to_uppercase(),
        None => false,
    }
}

/// `class_reference?`: a constant with a lower-case letter in its name.
fn is_class_reference(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    constant_name(context, node).is_some_and(|name| name.chars().any(char::is_lowercase))
}

/// The last segment of a constant path, which is what `node.children[1]` names.
fn constant_name<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" => Some(context.source.node_text(node.field("name")?)),
        _ => None,
    }
}
