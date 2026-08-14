use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::naming::support::ruby_regex;
use crate::rules::send_node::send_range;

use super::flow;
use super::locals::LocalVariables;
use super::statements::{body_children, body_statements, statements};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "This loop will have at most one iteration.";

/// `Enumerable.instance_methods + [:each]` and `ENUMERATOR_METHODS`, which upstream reads off the
/// running Ruby. The list is baked in here because a linter must answer the same for one file
/// whatever interpreter happens to be installed next to it.
const LOOP_METHODS: &[&str] = &[
    "all?",
    "any?",
    "chain",
    "chunk",
    "chunk_while",
    "collect",
    "collect_concat",
    "compact",
    "count",
    "cycle",
    "detect",
    "downto",
    "drop",
    "drop_while",
    "each",
    "each_entry",
    "entries",
    "filter",
    "filter_map",
    "find",
    "find_all",
    "find_index",
    "first",
    "flat_map",
    "grep",
    "grep_v",
    "group_by",
    "include?",
    "inject",
    "lazy",
    "loop",
    "map",
    "map!",
    "max",
    "max_by",
    "member?",
    "min",
    "min_by",
    "minmax",
    "minmax_by",
    "none?",
    "one?",
    "partition",
    "reduce",
    "reject",
    "reject!",
    "reverse_each",
    "select",
    "select!",
    "slice_after",
    "slice_before",
    "slice_when",
    "sort",
    "sort_by",
    "sum",
    "take",
    "take_while",
    "tally",
    "times",
    "to_a",
    "to_h",
    "to_set",
    "uniq",
    "upto",
    "zip",
];

/// `while`, `until` and `for`, in both the block and the modifier spelling. `Node#loop_keyword?`
/// answers for these, which is what keeps a nested loop from counting as a `next` in front of a
/// `break`.
const LOOP_KEYWORDS: &[&str] = &["while", "until", "for", "while_modifier", "until_modifier"];

/// `CONTINUE_KEYWORDS`.
const CONTINUE: [&str; 2] = ["next", "redo"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = allowed_patterns(context);
    let locals = LocalVariables::new(context);
    for node in context.nodes_of_any(&["while", "until", "for", "while_modifier", "until_modifier"])
    {
        inspect(
            node,
            node.field("body"),
            context,
            &locals,
            &allowed,
            offenses,
        );
    }
    for node in context.nodes_of("call") {
        let Some(block) = node.field("block") else {
            continue;
        };
        if !loop_method(node, context, &allowed) {
            continue;
        }
        inspect(
            node,
            block.field("body"),
            context,
            &locals,
            &allowed,
            offenses,
        );
    }
}

fn inspect(
    node: Node<'_>,
    body: Option<Node<'_>>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    allowed: &[&'static Regex],
    offenses: &mut Vec<Offense>,
) {
    let statements = body_statements(body);
    let Some(index) = statements
        .iter()
        .position(|statement| is_break(*statement, context, locals, allowed))
    else {
        return;
    };
    if preceded_by_continue(node, &statements, index, context, allowed)
        || conditional_continue(statements[index])
    {
        return;
    }
    // `add_offense(node)`: the loop, and for a block the whole expression it hangs off -- which is
    // where the `call` node begins and where the block ends.
    offenses.push(context.offense(MSG, node.byte_range()));
}

/// `break_statement?`.
fn is_break(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    allowed: &[&'static Regex],
) -> bool {
    if flow::is_break_command(node, context, locals) {
        return true;
    }
    match node.kind_str() {
        "begin" | "parenthesized_statements" => {
            let inner = if node.kind_str() == "begin" {
                body_children(node)
            } else {
                statements(node)
            };
            inner
                .iter()
                .position(|statement| is_break(*statement, context, locals, allowed))
                .is_some_and(|index| {
                    !preceded_by_sibling_continue(&inner, index, context, allowed)
                })
        }
        "if" | "unless" | "elsif" | "conditional" => {
            flow::check_if(node, &mut |child| is_break(child, context, locals, allowed))
        }
        "case" | "case_match" => {
            flow::check_case(node, &mut |child| is_break(child, context, locals, allowed))
        }
        _ => false,
    }
}

/// `preceded_by_continue_statement?`: whether any statement written before the break holds a `next`
/// or a `redo`. A loop of its own does not count -- its keywords belong to it, not to this one.
fn preceded_by_sibling_continue(
    statements: &[Node<'_>],
    index: usize,
    context: &RuleContext<'_>,
    allowed: &[&'static Regex],
) -> bool {
    statements[..index]
        .iter()
        .any(|sibling| is_continue_sibling(*sibling, None, context, allowed))
}

fn preceded_by_continue(
    loop_node: Node<'_>,
    statements: &[Node<'_>],
    index: usize,
    context: &RuleContext<'_>,
    allowed: &[&'static Regex],
) -> bool {
    // A body of one statement is that statement upstream, so its left siblings are the loop's own
    // other children: the condition of a `while`, or the call and the parameters of a block.
    if statements.len() == 1 {
        return outer_siblings(loop_node)
            .into_iter()
            .any(|(sibling, skip)| is_continue_sibling(sibling, skip, context, allowed));
    }
    preceded_by_sibling_continue(statements, index, context, allowed)
}

fn is_continue_sibling(
    sibling: Node<'_>,
    skip: Option<Node<'_>>,
    context: &RuleContext<'_>,
    allowed: &[&'static Regex],
) -> bool {
    !LOOP_KEYWORDS.contains(&sibling.kind_str())
        && !is_loop_shape(sibling, context, allowed)
        && has_continue(sibling, skip)
}

/// The children a loop node has before its body, each with the subtree that is the body rather
/// than a sibling.
fn outer_siblings<'tree>(node: Node<'tree>) -> Vec<(Node<'tree>, Option<Node<'tree>>)> {
    if let Some(block) = node.field("block") {
        // The block is `(block send args body)` upstream: the send is one sibling, the parameters
        // another, and the send is everything of the call but the block itself.
        let mut siblings = vec![(node, Some(block))];
        if let Some(parameters) = block.field("parameters") {
            siblings.push((parameters, None));
        }
        return siblings;
    }
    ["condition", "pattern", "value"]
        .iter()
        .filter_map(|field| node.field(field))
        .map(|child| (child, None))
        .collect()
}

/// Whether the node is a block on a method that iterates, which upstream skips because its own
/// `next` belongs to it.
fn is_loop_shape(node: Node<'_>, context: &RuleContext<'_>, allowed: &[&'static Regex]) -> bool {
    node.kind_str() == "call"
        && node.field("block").is_some()
        && loop_method(node, context, allowed)
}

/// `each_descendant(:next, :redo).any?`, without descending into `skip`.
fn has_continue(node: Node<'_>, skip: Option<Node<'_>>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| skip.is_none_or(|skip| skip.id() != child.id()))
        .any(|child| CONTINUE.contains(&child.kind_str()) || has_continue(child, skip))
}

/// `conditional_continue_keyword?`: the last `or` written anywhere in the break statement, when its
/// right-hand side is a `next` or a `redo`.
fn conditional_continue(node: Node<'_>) -> bool {
    let Some(last) = last_or(node) else {
        return false;
    };
    last.field("right")
        .is_some_and(|right| CONTINUE.contains(&right.kind_str()))
}

fn last_or<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut found = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_or(child) {
            found = Some(child);
        }
        if let Some(inner) = last_or(child) {
            found = Some(inner);
        }
    }
    found
}

fn is_or(node: Node<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(operator.kind_str(), "||" | "or"))
}

/// `loop_method?`: a block on a method that iterates, unless its source matches `AllowedPatterns`.
fn loop_method(call: Node<'_>, context: &RuleContext<'_>, allowed: &[&'static Regex]) -> bool {
    let Some(method) = call.field("method") else {
        return false;
    };
    let name = context.source.node_text(method);
    if !LOOP_METHODS.contains(&name) && !name.starts_with("each_") {
        return false;
    }
    let source = context.source.slice(send_range(call, context));
    !allowed.iter().any(|pattern| pattern.is_match(source))
}

fn allowed_patterns(context: &RuleContext<'_>) -> Vec<&'static Regex> {
    context
        .setting::<Vec<serde_yaml_ng::Value>>("AllowedPatterns")
        .unwrap_or_default()
        .iter()
        .filter_map(ruby_regex)
        .collect()
}
