use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, named_children};

use super::regexp::{captures, interpolates, pattern};
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

/// The methods that leave `$~` set, which is what makes a numbered reference after them valid.
/// `RESTRICT_ON_SEND` is this list, so a call to anything else leaves the state alone.
const CAPTURE_METHODS: &[&str] = &[
    "=~",
    "===",
    "match",
    "grep",
    "gsub",
    "gsub!",
    "sub",
    "sub!",
    "[]",
    "slice",
    "slice!",
    "index",
    "rindex",
    "scan",
    "partition",
    "rpartition",
    "start_with?",
    "end_with?",
];

/// The methods that take the regexp as their first argument rather than as their receiver.
const ARGUMENT_METHODS: &[&str] = &[
    "=~",
    "match",
    "grep",
    "gsub",
    "gsub!",
    "sub",
    "sub!",
    "[]",
    "slice",
    "slice!",
    "index",
    "rindex",
    "scan",
    "partition",
    "rpartition",
    "start_with?",
    "end_with?",
];

/// The state the walk carries: how many captures the regexp last matched against declared, or
/// `None` once a call has made that undecidable.
struct Walk {
    valid_ref: Option<usize>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut walk = Walk { valid_ref: Some(0) };
    visit(context.root_node(), &mut walk, context, offenses);
}

/// The commissioner's traversal: `on_*` on the way in, `after_send` on the way out.
fn visit(node: Node<'_>, walk: &mut Walk, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    match node.kind_str() {
        "global_variable" => {
            if let Some(backref) = numbered_reference(context.source.node_text(node)) {
                report(backref, node, walk, context, offenses);
            }
        }
        "when" => {
            walk.valid_ref = named_children_of(node, context)
                .into_iter()
                .filter(|child| child.kind_str() == "pattern")
                .flat_map(named_children)
                .filter(|condition| condition.kind_str() == "regex")
                .filter_map(|condition| capture_count(condition, context))
                .max();
        }
        "in_clause" => {
            walk.valid_ref = node
                .field("pattern")
                .into_iter()
                .flat_map(|pattern| regexp_patterns(pattern))
                .filter_map(|pattern| capture_count(pattern, context))
                .max();
        }
        // `/(?<a>x)/ =~ str` is a `match_with_lvasgn`, whose first child is the literal.
        "binary" if is_match_with_lvasgn(node, context) => {
            if let Some(left) = node.field("left") {
                walk.valid_ref = capture_count(left, context);
            }
        }
        _ => {}
    }
    // The block written after a call is a child of the call here and its parent upstream, so the
    // call is left behind -- and its state reset -- before the block body is reached. A modifier
    // conditional puts its condition after its body here and before it upstream, for the same
    // reason: the walk has to see them in the order the parser's children are listed.
    let block = node.field("block");
    let mut children = named_children_of(node, context);
    if matches!(
        node.kind_str(),
        "if_modifier" | "unless_modifier" | "while_modifier" | "until_modifier"
    ) {
        children.reverse();
    }
    for child in &children {
        if block.is_some_and(|block| block.id() == child.id()) {
            continue;
        }
        visit(*child, walk, context, offenses);
    }
    if is_capture_call(node, context) {
        walk.valid_ref = None;
        if let Some(regexp) = regexp_operand(node, context) {
            walk.valid_ref = capture_count(regexp, context);
        }
    }
    if let Some(block) = block {
        visit(block, walk, context, offenses);
    }
}

fn report(
    backref: usize,
    node: Node<'_>,
    walk: &Walk,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let Some(valid) = walk.valid_ref else {
        return;
    };
    if backref <= valid {
        return;
    }
    let count = if valid == 0 {
        "no".to_owned()
    } else {
        valid.to_string()
    };
    let group = if valid == 1 { "group" } else { "groups" };
    offenses.push(context.offense(
        format!("${backref} is out of range ({count} regexp capture {group} detected)."),
        node.byte_range(),
    ));
}

/// `check_regexp`: named captures take priority, since numbering is off once any of them is named.
fn capture_count(node: Node<'_>, context: &RuleContext<'_>) -> Option<usize> {
    if node.kind_str() != "regex" || interpolates(node) {
        return None;
    }
    let (source, extended) = pattern(node, context)?;
    let found = captures(source, extended);
    Some(if found.named > 0 {
        found.named
    } else {
        found.numbered
    })
}

/// The `regexp` literals a `in` pattern holds, which is the pattern itself or the ones inside it.
fn regexp_patterns<'tree>(pattern: Node<'tree>) -> Vec<Node<'tree>> {
    if pattern.kind_str() == "regex" {
        return vec![pattern];
    }
    let mut found = Vec::new();
    collect_regexps(pattern, &mut found);
    found
}

fn collect_regexps<'tree>(node: Node<'tree>, found: &mut Vec<Node<'tree>>) {
    for child in named_children(node) {
        if child.kind_str() == "regex" {
            found.push(child);
        }
        collect_regexps(child, found);
    }
}

/// Whether the node is a call to one of the methods that reset `$~`.
fn is_capture_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "call" => node
            .field("method")
            .is_some_and(|method| CAPTURE_METHODS.contains(&context.source.node_text(method))),
        // Every binary operator is a `send`, and `=~` and `===` are two of the listed methods.
        "binary" => node
            .field("operator")
            .is_some_and(|operator| CAPTURE_METHODS.contains(&context.source.node_text(operator))),
        "element_reference" => true,
        _ => false,
    }
}

/// `regexp_first_argument?` and `regexp_receiver?`, which is what the state is taken from.
fn regexp_operand<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    let (name, first, receiver) = match node.kind_str() {
        "call" => (
            context
                .source
                .node_text(node.field("method")?),
            arguments(node).first().map(|argument| argument.first()),
            node.field("receiver"),
        ),
        "binary" => (
            context
                .source
                .node_text(node.field("operator")?),
            node.field("right"),
            node.field("left"),
        ),
        "element_reference" => ("[]", named_children_of(node, context).into_iter().nth(1), node.child(0)),
        _ => return None,
    };
    if let Some(first) = first
        && first.kind_str() == "regex"
        && ARGUMENT_METHODS.contains(&name)
    {
        return Some(first);
    }
    receiver.filter(|receiver| receiver.kind_str() == "regex")
}

/// `(match_with_lvasgn (regexp ...) _)`: a literal with named captures written left of `=~`.
fn is_match_with_lvasgn(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("operator")
        .is_some_and(|operator| context.source.node_text(operator) == "=~")
        && node
            .field("left")
            .is_some_and(|left| left.kind_str() == "regex")
}

/// `$1` through `$9`, the references the parser spells as `nth_ref`.
fn numbered_reference(name: &str) -> Option<usize> {
    let digits = name.strip_prefix('$')?;
    if !digits.starts_with(|first: char| first.is_ascii_digit() && first != '0')
        || !digits.chars().all(|digit| digit.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok()
}
