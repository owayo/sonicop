use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::lint::literals::{is_constant, literal_type};
use crate::rules::lint::locals::LocalVariables;

use super::conditional::{UpstreamParent, upstream_parent};

/// `ALLOWED_NODE_TYPES`: the parents whose child keeps its parentheses around a logical operator.
/// `send` covers every shape the grammar spells as a call.
const ALLOWED_PARENT_KINDS: &[&str] = &["splat_argument", "hash_splat_argument"];

/// `KEYWORDS`, by the node kind the grammar gives each of them. `and` and `or` are left out: they
/// count only when written as words, which is checked against the operator itself.
const KEYWORD_KINDS: &[&str] = &[
    "alias",
    "break",
    "case",
    "case_match",
    "class",
    "singleton_class",
    "method",
    "singleton_method",
    "begin",
    "do",
    "else",
    "ensure",
    "for",
    "if",
    "elsif",
    "if_modifier",
    "unless",
    "unless_modifier",
    "module",
    "next",
    "redo",
    "rescue",
    "rescue_modifier",
    "retry",
    "return",
    "self",
    "super",
    "then",
    "undef",
    "until",
    "until_modifier",
    "when",
    "while",
    "while_modifier",
    "yield",
    "begin_block",
    "end_block",
];

/// The node kinds that hold their arguments in an `argument_list`, which is where the jump
/// keywords park what they are given.
const JUMPS: &[&str] = &["return", "break", "next", "yield", "super"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    // `@pending_offenses`, keyed by the node the offense lands on: reporting a range twice is what
    // `add_offense` would drop, and a range reached from two parenthesized groups is spelled here.
    let mut pending: Vec<(Node<'_>, &'static str)> = Vec::new();

    for node in context.nodes_of("parenthesized_statements") {
        if !is_begin_node(context, node)
            || parens_allowed(context, node)
            || ignore_syntax(context, node)
        {
            continue;
        }
        check_group(context, &locals, node, &mut pending);
    }
    // `on_investigation_end`: each candidate's exact correction is verified by reparsing before the
    // offense is registered, so redundancy never rests on a hand-kept list of the grammar's rules.
    let verified = crate::rules::support::verified_by_reparse(
        context,
        pending,
        |(node, _)| super::parens::correct(context, *node),
        |(node, _)| node.byte_range(),
    );
    for (node, message) in verified {
        offenses.push(
            context
                .offense(
                    format!("Don't use parentheses around {message}."),
                    node.byte_range(),
                )
                .corrected_by_all(super::parens::correct(context, node)),
        );
    }
}

fn record<'tree>(
    pending: &mut Vec<(Node<'tree>, &'static str)>,
    node: Node<'tree>,
    message: &'static str,
) {
    match pending.iter_mut().find(|(seen, _)| seen.id() == node.id()) {
        Some(entry) => entry.1 = message,
        None => pending.push((node, message)),
    }
}

/// Whether the parentheses are the ones upstream's parser builds a `begin` node from.
///
/// `defined?(x)` is the one place the grammar parks a parenthesized group where the parser has
/// none: written without a space, the parentheses belong to the keyword itself.
fn is_begin_node(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    if parent.kind() != "unary" {
        return true;
    }
    let Some(operator) = parent.child_by_field_name("operator") else {
        return true;
    };
    context.source.node_text(operator) != "defined?" || operator.end_byte() != node.start_byte()
}

/// The parent of the group as upstream's parser built the tree.
///
/// The parentheses of a `defined?(x)` are the keyword's own, so a group written straight inside
/// them has the `defined?` for its parent rather than a `begin` the parser never built.
fn parent_of<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<UpstreamParent<'tree>> {
    let mut current = node;
    loop {
        match upstream_parent(current)? {
            UpstreamParent::Begin(parent) if !is_begin_node(context, parent) => current = parent,
            resolved => return Some(resolved),
        }
    }
}

fn parent_node<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    match parent_of(context, node)? {
        UpstreamParent::Begin(parent) | UpstreamParent::Node(parent) => Some(parent),
    }
}

fn parent_is_begin(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    matches!(parent_of(context, node), Some(UpstreamParent::Begin(_)))
}

/// The number of children the parent holds, counting the ones that are not nodes the way
/// `Node#children` does. Only the shapes that can hold exactly one are spelled out; everything
/// else answers "more than one", which is all the callers ask.
fn parent_child_count(context: &RuleContext<'_>, node: Node<'_>) -> usize {
    let Some(parent) = parent_of(context, node) else {
        return 0;
    };
    match parent {
        UpstreamParent::Begin(parent) => match parent.kind() {
            "parenthesized_statements" | "interpolation" | "begin" => {
                super::nodes::children(parent).len()
            }
            // A statement list the grammar wraps -- a `then`, an `else`, a definition's body --
            // is a `begin` of the statements it holds, less the clauses that are not statements
            // of it.
            _ => super::conditional::self_statements(parent).len(),
        },
        UpstreamParent::Node(parent) => match parent.kind() {
            "array" | "string_array" | "symbol_array" | "exceptions" => {
                super::nodes::children(parent).len()
            }
            "splat_argument"
            | "hash_splat_argument"
            | "block_argument"
            | "pin"
            | "interpolation"
            | "begin_block"
            | "end_block" => 1,
            kind if JUMPS.contains(&kind) => {
                super::nodes::children(parent)
                    .first()
                    .map_or(0, |list| match list.kind() {
                        "argument_list" => super::nodes::children(*list).len(),
                        _ => 1,
                    })
            }
            // `(defined? x)` holds its expression alone; every other unary is a `send` with a
            // receiver and a selector.
            "unary" => match parent
                .child_by_field_name("operator")
                .is_some_and(|operator| operator.byte_range().len() == "defined?".len())
            {
                true => 1,
                false => 2,
            },
            _ => 2,
        },
    }
}

/// `parens_allowed?`.
fn parens_allowed(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let children = super::nodes::children(node);
    // `empty_parentheses?`: `()` says something no rewrite could keep.
    if children.is_empty() {
        return true;
    }
    is_rescue(context, node)
        || in_pattern_matching_in_method_argument(context, node, &children)
        || allowed_pin_operator(node, &children)
        || allowed_expression(context, node, &children)
}

/// `rescue?`: `{^resbody ^^resbody}`, the parentheses around an exception list or around the body
/// a `rescue` clause runs.
fn is_rescue(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = parent_node(context, node) else {
        return false;
    };
    if parent.kind() == "rescue" {
        return true;
    }
    parent.kind() == "exceptions"
        && parent
            .parent()
            .is_some_and(|grandparent| grandparent.kind() == "rescue")
}

/// `in_pattern_matching_in_method_argument?`: `foo(bar in Integer)` needs the parentheses.
fn in_pattern_matching_in_method_argument(
    context: &RuleContext<'_>,
    node: Node<'_>,
    children: &[Node<'_>],
) -> bool {
    let Some(parent) = parent_node(context, node) else {
        return false;
    };
    if !is_call(context, parent) {
        return false;
    }
    let Some(first) = children.first() else {
        return false;
    };
    // `match_pattern_p_type?` above Ruby 2.7, where `in` builds that node rather than the `=>` one.
    match context.target_ruby_version() <= crate::ruby_version::RubyVersion::new(2, 7) {
        true => first.kind() == "match_pattern",
        false => first.kind() == "test_pattern",
    }
}

/// `allowed_pin_operator?`: `^(pin (begin !{lvar ivar cvar gvar}))`.
fn allowed_pin_operator(node: Node<'_>, children: &[Node<'_>]) -> bool {
    if !node.parent().is_some_and(|parent| parent.kind() == "pin") {
        return false;
    }
    !children.first().is_some_and(|first| {
        matches!(
            first.kind(),
            "identifier" | "instance_variable" | "class_variable" | "global_variable"
        )
    })
}

fn allowed_expression(context: &RuleContext<'_>, node: Node<'_>, children: &[Node<'_>]) -> bool {
    allowed_ancestor(context, node)
        || allowed_multiple_expression(context, node, children)
        || allowed_ternary(context, node)
        || parent_node(context, node).is_some_and(|parent| parent.kind() == "range")
}

/// `allowed_ancestor?`: `break(1)` reads as a call, so the parentheses are not the parser's.
fn allowed_ancestor(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    parent_node(context, node).is_some_and(|parent| is_keyword(context, parent))
        && parens_required(context, node)
}

/// `Parentheses#parens_required?`: a letter written straight against either parenthesis.
fn parens_required(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let bytes = context.source.text().as_bytes();
    let before = node
        .start_byte()
        .checked_sub(1)
        .and_then(|index| bytes.get(index));
    let after = bytes.get(node.end_byte());
    [before, after]
        .into_iter()
        .flatten()
        .any(|byte| byte.is_ascii_lowercase())
}

/// `Node#keyword?`.
fn is_keyword(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if is_special_keyword(context, node) {
        return true;
    }
    if node.kind() == "unary" {
        let text = node
            .child_by_field_name("operator")
            .map(|operator| context.source.node_text(operator));
        // `defined?` is a keyword outright; `not` is the one `send` that counts as one.
        return matches!(text, Some("defined?") | Some("not"));
    }
    // `and` and `or` count only in their word spelling.
    if node.kind() == "binary" {
        return node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "and" | "or"));
    }
    KEYWORD_KINDS.contains(&node.kind())
}

/// `Node#special_keyword?`: the three the parser resolves into values before a cop sees them.
fn is_special_keyword(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "identifier" | "constant" | "string" | "integer" | "simple_symbol"
    ) && matches!(
        context.source.node_text(node),
        "__FILE__" | "__LINE__" | "__ENCODING__"
    )
}

/// `allowed_multiple_expression?`: `(a; b)` written anywhere but in a statement position is the
/// only thing keeping the two statements together.
fn allowed_multiple_expression(
    context: &RuleContext<'_>,
    node: Node<'_>,
    children: &[Node<'_>],
) -> bool {
    if children.len() == 1 {
        return false;
    }
    let Some(parent) = parent_of(context, node) else {
        return false;
    };
    match parent {
        UpstreamParent::Begin(_) => false,
        UpstreamParent::Node(parent) => !matches!(
            parent.kind(),
            "method" | "singleton_method" | "block" | "do_block" | "lambda"
        ),
    }
}

/// `allowed_ternary?`: the neighbouring cop asks for these parentheses.
fn allowed_ternary(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if !node
        .parent()
        .is_some_and(|parent| parent.kind() == "conditional")
    {
        return false;
    }
    if context
        .setting_of::<bool>("Style/TernaryParentheses", "Enabled")
        .is_some_and(|enabled| !enabled)
    {
        return false;
    }
    context
        .setting_of::<String>("Style/TernaryParentheses", "EnforcedStyle")
        .is_some_and(|style| {
            matches!(
                style.as_str(),
                "require_parentheses" | "require_parentheses_when_complex"
            )
        })
}

/// `ignore_syntax?`.
fn ignore_syntax(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = parent_node(context, node) else {
        return false;
    };
    like_method_argument_parentheses(context, node, parent)
        || multiline_control_flow_statements(context, node, parent)
}

/// `like_method_argument_parentheses?`: `p (1)` reads as the argument list it looks like, so the
/// parentheses are the call's rather than a group's.
fn like_method_argument_parentheses(
    context: &RuleContext<'_>,
    node: Node<'_>,
    parent: Node<'_>,
) -> bool {
    // A setter call takes the assigned value as its only argument, is never written with
    // parentheses, and is named after no operator -- `a[0] = (x)` is `:[]=`, which is one.
    if is_setter_assignment(parent) {
        return parent
            .child_by_field_name("left")
            .is_some_and(|left| left.kind() == "call")
            && parent
                .child_by_field_name("right")
                .is_some_and(|right| right.id() == node.id());
    }
    if !matches!(parent.kind(), "call" | "super" | "yield") {
        return false;
    }
    let Some(arguments) = argument_list(parent) else {
        return false;
    };
    let list = super::nodes::children(arguments);
    if list.len() != 1 || list[0].id() != node.id() {
        return false;
    }
    // `!node.parenthesized?`: a call that already spells its own parentheses is not this shape.
    if call_is_parenthesized(context, parent) {
        return false;
    }
    !parent
        .child_by_field_name("method")
        .is_some_and(|method| super::nodes::is_operator_method(context.source.node_text(method)))
}

/// `ParameterizedNode#parenthesized?`: `loc.end` is a `)` of the call's own, which the grammar
/// tells apart from a parenthesized first argument only by the blank before it -- `p (1)` hands the
/// call one `begin` argument while `p(1)` hands it the integer.
fn call_is_parenthesized(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(arguments) = argument_list(node) else {
        return false;
    };
    let bytes = context.source.text().as_bytes();
    context.source.node_text(arguments).starts_with('(')
        && arguments
            .start_byte()
            .checked_sub(1)
            .and_then(|index| bytes.get(index))
            .is_some_and(|byte| !byte.is_ascii_whitespace())
}

fn argument_list<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    if let Some(arguments) = node.child_by_field_name("arguments") {
        return Some(arguments);
    }
    super::nodes::children(node)
        .into_iter()
        .find(|child| child.kind() == "argument_list")
}

/// `multiline_control_flow_statements?`: a jump written over several lines keeps its parentheses.
fn multiline_control_flow_statements(
    context: &RuleContext<'_>,
    node: Node<'_>,
    parent: Node<'_>,
) -> bool {
    let _ = node;
    if !matches!(parent.kind(), "return" | "next" | "break") {
        return false;
    }
    context.source.line_column(parent.start_byte()).0
        != context.source.line_column(parent.end_byte()).0
}

/// `check`.
fn check_group<'tree>(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    node: Node<'tree>,
    pending: &mut Vec<(Node<'tree>, &'static str)>,
) {
    let children = super::nodes::children(node);
    let Some(inner) = children.first().copied() else {
        return;
    };
    if let Some(message) = find_offense_message(context, locals, node, inner, &children) {
        if message == "block body" {
            record(pending, node, message);
            return;
        }
        // A range keeps the parentheses that hold it apart from what surrounds it, so the group
        // reported is the one around them.
        let target =
            match is_range(inner) && !argument_of_parenthesized_method_call(context, node, inner) {
                true => parent_node(context, node).unwrap_or(node),
                false => node,
            };
        record(pending, target, message);
        return;
    }
    if is_call_node(context, locals, inner) {
        check_send(context, locals, node, inner, pending);
    }
}

fn is_range(node: Node<'_>) -> bool {
    node.kind() == "range"
}

/// `find_offense_message`.
fn find_offense_message(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    node: Node<'_>,
    inner: Node<'_>,
    children: &[Node<'_>],
) -> Option<&'static str> {
    if keyword_with_redundant_parentheses(context, inner) {
        return Some("a keyword");
    }
    if literal_type(inner, context).is_some() && disallowed_literal(context, node, inner) {
        return Some("a literal");
    }
    if is_variable(locals, inner) {
        return Some("a variable");
    }
    if is_constant(inner, context) {
        return Some("a constant");
    }
    if parent_node(context, node).is_some_and(is_block) || body_range(context, node, inner) {
        return Some("block body");
    }
    if is_assignment(inner)
        && (parent_of(context, node).is_none() || parent_is_begin(context, node))
    {
        return Some("an assignment");
    }
    if is_lambda_or_proc(context, inner) {
        return Some("an expression");
    }
    if disallowed_one_line_pattern_matching(context, node, inner) {
        return Some("a one-line pattern matching");
    }
    if is_interpolation(context, node) {
        return Some("an interpolated expression");
    }
    if argument_of_parenthesized_method_call(context, node, inner) {
        return Some("a method argument");
    }
    if oneline_rescue_parentheses_required(context, node, inner) {
        return Some("a one-line rescue");
    }
    if is_chained(context, node) {
        return None;
    }
    if is_operator_keyword(context, inner) {
        if is_semantic_operator(context, inner) && parent_of(context, node).is_some() {
            return None;
        }
        if context.source.line_column(inner.start_byte()).0
            != context.source.line_column(inner.end_byte()).0
            && allow_in_multiline_conditions(context)
        {
            return None;
        }
        if parent_node(context, node).is_some_and(|parent| {
            is_call(context, parent)
                || is_or(context, parent)
                || ALLOWED_PARENT_KINDS.contains(&parent.kind())
        }) {
            return None;
        }
        if !is_and(context, inner)
            && parent_node(context, node).is_some_and(|parent| is_and(context, parent))
        {
            return None;
        }
        if node
            .parent()
            .is_some_and(|parent| parent.kind() == "conditional")
        {
            return None;
        }
        let _ = children;
        return Some("a logical expression");
    }
    if is_comparison(context, inner) && parent_of(context, node).is_none() {
        return Some("a comparison expression");
    }
    None
}

fn is_block(node: Node<'_>) -> bool {
    matches!(node.kind(), "block" | "do_block" | "lambda")
}

/// `node.variable?`: an instance, class or global variable, or a bare name the parser resolved
/// into a local variable read.
fn is_variable(locals: &LocalVariables<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "instance_variable" | "class_variable" | "global_variable" => true,
        "identifier" => locals.is_lvar(node),
        _ => false,
    }
}

fn is_assignment(node: Node<'_>) -> bool {
    matches!(node.kind(), "assignment" | "operator_assignment")
}

/// `node.lambda_or_proc? && (node.braces? || node.send_node.lambda_literal?)`.
fn is_lambda_or_proc(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind() == "lambda" {
        // `-> {}` and `->() do end` are both `lambda_literal?`.
        return true;
    }
    let Some(block) = node.child_by_field_name("block") else {
        return false;
    };
    let name = node
        .child_by_field_name("method")
        .map(|method| context.source.node_text(method));
    if !matches!(name, Some("lambda") | Some("proc"))
        || node.child_by_field_name("receiver").is_some()
    {
        return false;
    }
    // `node.braces?`: only the brace spelling is rewritten, since `do ... end` would bind
    // differently once the parentheses go.
    block.kind() == "block"
}

/// `disallowed_one_line_pattern_matching?`.
fn disallowed_one_line_pattern_matching(
    context: &RuleContext<'_>,
    node: Node<'_>,
    inner: Node<'_>,
) -> bool {
    if let Some(parent) = parent_node(context, node) {
        if matches!(parent.kind(), "method" | "singleton_method")
            // `parent.endless?`: a definition written with `=` and no `end`.
            && parent.child_by_field_name("body").is_some_and(|body| {
                body.id() == node.id() && super::conditional::token(parent, &["end"]).is_none()
            })
        {
            return false;
        }
        if is_assignment(parent) {
            return false;
        }
    }
    if !matches!(inner.kind(), "match_pattern" | "test_pattern") {
        return false;
    }
    let mut current = node.parent();
    while let Some(ancestor) = current {
        // `each_ancestor.none?(&:operator_keyword?)`: an `and` or an `or` above it makes the
        // pattern match one operand of a logical expression, which needs the parentheses.
        if is_operator_keyword(context, ancestor) {
            return false;
        }
        current = ancestor.parent();
    }
    true
}

/// `interpolation?`: `[^begin ^^dstr]`, a group written straight inside a `#{}` of a string.
fn is_interpolation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "interpolation" {
        return false;
    }
    parent
        .parent()
        .is_some_and(|grandparent| literal_type(grandparent, context) == Some("dstr"))
}

/// `argument_of_parenthesized_method_call?`.
fn argument_of_parenthesized_method_call(
    context: &RuleContext<'_>,
    node: Node<'_>,
    inner: Node<'_>,
) -> bool {
    if is_basic_conditional(inner)
        || inner.kind() == "rescue_modifier"
        || method_call_parentheses_required(context, inner)
    {
        return false;
    }
    let Some(parent) = parent_node(context, node) else {
        return false;
    };
    if !is_call(context, parent) {
        return false;
    }
    // `parent.receiver != begin_node`: `Node#!=` compares structurally, so a receiver written the
    // same way as the group counts as being it.
    call_is_parenthesized(context, parent)
        && !parent
            .child_by_field_name("receiver")
            .is_some_and(|receiver| {
                crate::rules::lint::node_equality::identical(receiver, node, context)
            })
}

/// `BASIC_CONDITIONALS`: `if`, `while` and `until`. A ternary is an `if` upstream, so it is one.
fn is_basic_conditional(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "conditional"
            | "if"
            | "unless"
            | "elsif"
            | "if_modifier"
            | "unless_modifier"
            | "while"
            | "until"
            | "while_modifier"
            | "until_modifier"
    )
}

/// `method_call_parentheses_required?`: a call with arguments and no receiver, or one reached
/// through a dot, keeps the parentheses that hold its arguments.
fn method_call_parentheses_required(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if !is_call(context, node) {
        return false;
    }
    let has_receiver = receiver_of(context, node).is_some();
    let selector = match node.kind() {
        "assignment" => node.child_by_field_name("left"),
        _ => Some(node),
    };
    let dot = selector.is_some_and(|selector| {
        selector.kind() == "call"
            && selector
                .child_by_field_name("operator")
                .is_some_and(|operator| matches!(context.source.node_text(operator), "." | "&."))
    });
    (!has_receiver || dot) && call_has_arguments(node)
}

/// `oneline_rescue_parentheses_required?`: `(a rescue b)` keeps its parentheses unless what holds
/// it already separates it from what follows.
fn oneline_rescue_parentheses_required(
    context: &RuleContext<'_>,
    node: Node<'_>,
    inner: Node<'_>,
) -> bool {
    if inner.kind() != "rescue_modifier" {
        return false;
    }
    let Some(parent) = parent_node(context, node) else {
        return false;
    };
    if parent.kind() == "conditional" {
        return false;
    }
    if is_conditional(parent)
        && parent
            .child_by_field_name("condition")
            .is_some_and(|condition| condition.id() == node.id())
    {
        return false;
    }
    !(is_call(context, parent)
        || matches!(
            parent.kind(),
            "array" | "string_array" | "symbol_array" | "pair"
        ))
}

fn is_conditional(node: Node<'_>) -> bool {
    is_basic_conditional(node) || matches!(node.kind(), "case" | "case_match" | "conditional")
}

/// `node.chained?`: the group is the receiver a call hangs off.
fn is_chained(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    is_call(context, parent)
        && receiver_of(context, parent).is_some_and(|receiver| receiver.id() == node.id())
}

/// `node.call_type?` for a node that may be a bare name: the parser builds an `lvar` for one it
/// has seen assigned, and only a name it has not is a receiverless call.
fn is_send(context: &RuleContext<'_>, locals: &LocalVariables<'_>, node: Node<'_>) -> bool {
    if node.kind() == "identifier" && locals.is_lvar(node) {
        return false;
    }
    is_call(context, node)
}

/// Whether an assignment is the setter call upstream builds a `send` for.
fn is_setter_assignment(node: Node<'_>) -> bool {
    node.kind() == "assignment"
        && node
            .child_by_field_name("left")
            .is_some_and(|left| matches!(left.kind(), "call" | "element_reference"))
}

/// `SendNode#receiver`: the operand the grammar names differently for each shape a call takes.
fn receiver_of<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind() {
        "call" => node.child_by_field_name("receiver"),
        "element_reference" => node.child_by_field_name("object"),
        "assignment" => node
            .child_by_field_name("left")
            .and_then(|left| receiver_of(context, left)),
        // A binary operator is a call on its left operand, and a unary one a call on what follows.
        "binary" if !is_operator_keyword(context, node) => node.child_by_field_name("left"),
        "unary" => node.child_by_field_name("operand"),
        _ => None,
    }
}

fn is_operator_keyword(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    is_and(context, node) || is_or(context, node)
}

fn binary_operator<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    if node.kind() != "binary" {
        return None;
    }
    node.child_by_field_name("operator")
        .map(|operator| context.source.node_text(operator))
}

fn is_and(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    matches!(binary_operator(context, node), Some("&&") | Some("and"))
}

fn is_or(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    matches!(binary_operator(context, node), Some("||") | Some("or"))
}

fn is_semantic_operator(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    matches!(binary_operator(context, node), Some("and") | Some("or"))
}

fn is_comparison(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let comparison = |name: &str| matches!(name, "==" | "===" | "!=" | "<=" | ">=" | ">" | "<");
    match node.kind() {
        "binary" => binary_operator(context, node).is_some_and(comparison),
        "call" => node
            .child_by_field_name("method")
            .is_some_and(|method| comparison(context.source.node_text(method))),
        _ => false,
    }
}

fn allow_in_multiline_conditions(context: &RuleContext<'_>) -> bool {
    context
        .setting_of::<bool>(
            "Style/ParenthesesAroundCondition",
            "AllowInMultilineConditions",
        )
        .unwrap_or(false)
}

/// `disallowed_literal?`.
fn disallowed_literal(context: &RuleContext<'_>, node: Node<'_>, inner: Node<'_>) -> bool {
    if !is_range(inner) {
        return true;
    }
    parent_is_begin(context, node) && parent_child_count(context, node) == 1
}

/// `body_range?`: a beginless or endless range at either end of a statement list needs the
/// parentheses to stay apart from the statement next to it.
fn body_range(context: &RuleContext<'_>, node: Node<'_>, inner: Node<'_>) -> bool {
    if is_chained(context, node) || !is_range(inner) {
        return false;
    }
    let Some(UpstreamParent::Begin(parent)) = parent_of(context, node) else {
        return false;
    };
    let statements = match parent.kind() {
        "parenthesized_statements" | "interpolation" | "begin" => super::nodes::children(parent),
        _ => super::conditional::self_statements(parent),
    };
    let beginless = inner.child_by_field_name("begin").is_none()
        && !context
            .source
            .node_text(inner)
            .starts_with(|c: char| c != '.');
    let endless = inner.child_by_field_name("end").is_none();
    let first = statements.first().is_some_and(|statement| {
        statement.start_byte() <= node.start_byte() && statement.end_byte() >= node.end_byte()
    });
    let last = statements.last().is_some_and(|statement| {
        statement.start_byte() <= node.start_byte() && statement.end_byte() >= node.end_byte()
    });
    (beginless && first) || (endless && last)
}

/// `keyword_with_redundant_parentheses?`.
fn keyword_with_redundant_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if !is_keyword(context, node) {
        return false;
    }
    if is_special_keyword(context, node) {
        return true;
    }
    // `args = *node`: the node's own children, which for a keyword taking arguments is the list it
    // was given. `not x` is a `send`, whose selector counts as a second child there and so can
    // never be the single parenthesized argument below.
    let arguments = match node.kind() {
        kind if JUMPS.contains(&kind) => argument_list(node)
            .map(super::nodes::children)
            .unwrap_or_default(),
        // `(defined? x)` holds its expression alone, so a parenthesized one is the single argument
        // `only_begin_arg?` asks for. `not x` is a `send`, whose selector counts as a second child
        // there and rules that branch out.
        "unary" => {
            let defined = node
                .child_by_field_name("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "defined?");
            let group = node
                .child_by_field_name("operand")
                .filter(|operand| defined && operand.kind() == "parenthesized_statements");
            return match group {
                Some(group) => has_own_parentheses(context, group),
                None => has_own_parentheses(context, node),
            };
        }
        _ => super::nodes::children(node),
    };
    // `only_begin_arg?`: the keyword's single argument is itself a parenthesized group.
    if let [only] = arguments.as_slice()
        && only.kind() == "parenthesized_statements"
    {
        return has_own_parentheses(context, *only);
    }
    arguments.is_empty() || has_own_parentheses(context, node)
}

/// `Util.parentheses?(node)`: `loc.end` is a `)` of the node's own, which is the closing
/// parenthesis of an argument list rather than whatever character the node happens to end on.
fn has_own_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "call" | "super" | "yield" | "return" | "break" | "next" => {
            call_is_parenthesized(context, node)
        }
        // `defined?(x)` and `not(x)` carry the parentheses themselves; written with a blank they
        // are a group of their own.
        "unary" => node
            .child_by_field_name("operator")
            .is_some_and(|operator| {
                matches!(context.source.node_text(operator), "defined?" | "not")
                    && context.source.text().as_bytes().get(operator.end_byte()) == Some(&b'(')
            }),
        "parenthesized_statements" => true,
        _ => false,
    }
}

/// `SendNode#arguments.any?`: an operator call carries its other operand as its only argument, and
/// an index its subscripts.
fn call_has_arguments(node: Node<'_>) -> bool {
    match node.kind() {
        // A setter call takes the assigned value, and an indexing setter its subscripts too.
        "binary" | "assignment" => true,
        "element_reference" => !super::nodes::children(node).is_empty(),
        _ => argument_list(node).is_some_and(|list| !super::nodes::children(list).is_empty()),
    }
}

/// `call_node?`: a call, or a brace block that is not a lambda or a proc.
fn is_call_node(context: &RuleContext<'_>, locals: &LocalVariables<'_>, node: Node<'_>) -> bool {
    if is_send(context, locals, node) {
        return true;
    }
    node.child_by_field_name("block")
        .is_some_and(|block| block.kind() == "block")
        && !is_lambda_or_proc(context, node)
}

/// `node.call_type?`: every shape the grammar spells a `send` as, including the setter calls it
/// writes as assignments -- `a.b = 1` is `(send a :b= 1)` upstream, not an assignment node.
fn is_call(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if is_setter_assignment(node) {
        return true;
    }
    matches!(node.kind(), "call" | "element_reference" | "identifier")
        || (node.kind() == "binary" && !is_operator_keyword(context, node))
        || (node.kind() == "unary"
            && node
                .child_by_field_name("operator")
                .is_some_and(|operator| context.source.node_text(operator) != "defined?"))
}

/// `check_send`.
fn check_send<'tree>(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    node: Node<'tree>,
    inner: Node<'tree>,
    pending: &mut Vec<(Node<'tree>, &'static str)>,
) {
    let mut call = inner;
    if is_unary_operation(context, call) {
        if is_chained(context, node) {
            return;
        }
        while is_suspect_unary(context, call) {
            let Some(operand) = call.child_by_field_name("operand") else {
                break;
            };
            call = operand;
        }
        if method_call_with_redundant_parentheses(context, locals, node, call) {
            record(pending, node, "a unary operation");
        }
        return;
    }
    if method_call_with_redundant_parentheses(context, locals, node, call) {
        record(pending, node, "a method call");
    }
}

/// `unary_operation?`: an operator call whose selector opens the expression.
fn is_unary_operation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "unary"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| {
                context.source.node_text(operator) != "defined?"
                    && operator.start_byte() == node.start_byte()
            })
}

fn is_prefix_not(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "unary"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "not")
}

fn is_suspect_unary(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    is_unary_operation(context, node) && !is_prefix_not(context, node)
}

/// `method_call_with_redundant_parentheses?`.
fn method_call_with_redundant_parentheses(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    node: Node<'_>,
    call: Node<'_>,
) -> bool {
    let candidate = is_send(context, locals, call)
        || matches!(call.kind(), "super" | "yield")
        || (call.kind() == "unary"
            && call
                .child_by_field_name("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "defined?"));
    if !candidate || is_prefix_not(context, call) {
        return false;
    }
    if singular_parenthesized_parent(context, node) {
        return true;
    }
    !call_has_arguments(call) || has_own_parentheses(context, call) || square_brackets(call)
}

/// `singular_parenthesized_parent?`.
fn singular_parenthesized_parent(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = parent_of(context, node) else {
        return true;
    };
    if let UpstreamParent::Node(parent) = parent
        && matches!(parent.kind(), "splat_argument" | "hash_splat_argument")
    {
        return false;
    }
    parent_child_count(context, node) == 1
}

/// `square_brackets?`: an index written on something the parentheses cannot be part of.
fn square_brackets(node: Node<'_>) -> bool {
    if node.kind() != "element_reference" {
        return false;
    }
    let Some(object) = node.child_by_field_name("object") else {
        return false;
    };
    super::conditional::descendants(object)
        .into_iter()
        .any(|descendant| match descendant.kind() {
            "string" | "array" | "string_array" | "symbol_array" | "hash" => true,
            "constant" | "scope_resolution" => true,
            "instance_variable" | "class_variable" | "global_variable" => true,
            // `(send _recv _msg)`: a call written with no arguments at all.
            "call" => argument_list(descendant).is_none(),
            "identifier" => true,
            _ => false,
        })
}
