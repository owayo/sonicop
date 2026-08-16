use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use double pipes `||` instead.";
const REDUNDANT_CONDITION: &str = "This condition is not needed.";

/// `AllowedMethods`: predicates that answer more than a boolean, so `x.nonzero? ? true : y` is not
/// the same as `x.nonzero? || y`.
const DEFAULT_ALLOWED: &[&str] = &["infinite?", "nonzero?"];

/// `ARGUMENT_WITH_OPERATOR_TYPES`: an argument that spreads cannot be moved into a `||`.
const SPREAD_ARGUMENTS: &[&str] = &[
    "splat_argument",
    "block_argument",
    "hash_splat_argument",
    "forward_argument",
];

/// One branch of a conditional: the single expression it holds, and the source it spans.
#[derive(Clone)]
struct Branch<'tree> {
    node: Option<Node<'tree>>,
    range: Range<usize>,
    /// How many statements were written, which is what makes upstream build a `begin`.
    count: usize,
}

impl<'tree> Branch<'tree> {
    fn source<'a>(&self, context: &'a RuleContext<'_>) -> &'a str {
        context.source.slice(self.range.clone())
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_else(|| {
        DEFAULT_ALLOWED
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    });
    for node in context.nodes_of_any(&["if", "unless", "conditional"]) {
        // `elsif_conditional?`: an `elsif` chain is a different shape entirely.
        if node
            .field("alternative")
            .is_some_and(|alternative| alternative.kind_str() == "elsif")
        {
            continue;
        }
        let ternary = node.kind_str() == "conditional";
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let normalized_if = branch(node, true);
        let normalized_else = branch(node, false);
        // `*node`: the children as the parser stores them, which an `unless` holds the other way
        // round from the way it reads.
        let (raw_if, raw_else) = match node.kind_str() {
            "unless" => (normalized_else.clone(), normalized_if.clone()),
            _ => (normalized_if.clone(), normalized_else.clone()),
        };
        if !is_offense(
            context,
            ternary,
            condition,
            &raw_if,
            &raw_else,
            &normalized_if,
            &normalized_else,
            &allowed,
        ) {
            continue;
        }
        let redundant = normalized_else.is_none();
        let with_method = branches_have_method(context, &normalized_if, &normalized_else);
        let range = match ternary && !with_method {
            true => question_to_colon(context, node),
            false => Some(node.byte_range()),
        };
        let Some(range) = range else {
            continue;
        };
        let message = match redundant {
            true => REDUNDANT_CONDITION,
            false => MSG,
        };
        let mut offense = context.offense(message, range.clone());
        if let Some(edits) = corrections(
            context,
            node,
            ternary,
            condition,
            range,
            redundant,
            with_method,
            &normalized_if,
            &normalized_else,
            &raw_if,
            &raw_else,
        ) {
            offense = offense.corrected_by_all(edits);
        }
        offenses.push(offense);
    }
}

#[allow(clippy::too_many_arguments)]
fn is_offense(
    context: &RuleContext<'_>,
    ternary: bool,
    condition: Node<'_>,
    raw_if: &Option<Branch<'_>>,
    raw_else: &Option<Branch<'_>>,
    normalized_if: &Option<Branch<'_>>,
    normalized_else: &Option<Branch<'_>>,
    allowed: &[String],
) -> bool {
    // `use_if_branch?` and `use_hash_key_assignment?`: neither shape is a plain either-or.
    if let Some(branch) = raw_else.as_ref().and_then(|branch| branch.node) {
        if matches!(
            branch.kind_str(),
            "if" | "unless" | "elsif" | "conditional" | "if_modifier" | "unless_modifier"
        ) {
            return false;
        }
        if branch.kind_str() == "assignment"
            && branch
                .field("left")
                .is_some_and(|left| left.kind_str() == "element_reference")
        {
            return false;
        }
    }
    if !synonymous(
        context,
        condition,
        raw_if,
        normalized_if,
        normalized_else,
        allowed,
    ) {
        return false;
    }
    if ternary {
        return true;
    }
    // `else_branch` here is the **parser's** else, not the one the source reads as `else`. An
    // `unless` holds its branches the other way round, so looking at the normalized side asks
    // whether `b` fits on one line when upstream asks it of `c; d`.
    let Some(branch) = raw_else else {
        return true;
    };
    // `!else_branch.instance_of?(AST::Node)`: only the node types without a class of their own --
    // a `begin` above all -- have to fit on one line.
    if !is_plain_node(branch) {
        return true;
    }
    context.source.line_column(branch.range.start).0
        == context.source.line_column(branch.range.end).0
}

/// The node types `RuboCop::AST::Builder` has no specialized class for.
fn is_plain_node(branch: &Branch<'_>) -> bool {
    let Some(node) = branch.node else {
        return true;
    };
    if branch.count > 1 {
        return true;
    }
    matches!(
        node.kind_str(),
        "parenthesized_statements" | "nil" | "true" | "false" | "self" | "yield"
    )
}

fn synonymous(
    context: &RuleContext<'_>,
    condition: Node<'_>,
    raw_if: &Option<Branch<'_>>,
    normalized_if: &Option<Branch<'_>>,
    normalized_else: &Option<Branch<'_>>,
    allowed: &[String],
) -> bool {
    let condition_source = context.source.node_text(condition);
    if raw_if
        .as_ref()
        .is_some_and(|branch| branch.source(context) == condition_source)
    {
        return true;
    }
    if if_branch_is_true(context, condition, normalized_if, normalized_else, allowed) {
        return true;
    }
    if assignment_pair(context, normalized_if, normalized_else).is_some()
        && raw_if
            .as_ref()
            .and_then(|branch| branch.node)
            .and_then(|node| node.field("right"))
            .is_some_and(|expression| context.source.node_text(expression) == condition_source)
    {
        return true;
    }
    if !branches_have_method(context, normalized_if, normalized_else) {
        return false;
    }
    let Some(if_branch) = normalized_if.as_ref().and_then(|branch| branch.node) else {
        return false;
    };
    // `use_hash_key_access?`: `h[k]` is a lookup rather than a call that could take a default.
    if if_branch.kind_str() == "element_reference" {
        return false;
    }
    first_argument(if_branch)
        .is_some_and(|(_, range)| context.source.slice(range) == condition_source)
}

/// `if_branch_is_true_type_and_else_is_not?`: `x.zero? ? true : y` answers the same as `x.zero? || y`.
fn if_branch_is_true(
    context: &RuleContext<'_>,
    condition: Node<'_>,
    normalized_if: &Option<Branch<'_>>,
    normalized_else: &Option<Branch<'_>>,
    allowed: &[String],
) -> bool {
    let Some(if_branch) = normalized_if.as_ref().and_then(|branch| branch.node) else {
        return false;
    };
    let Some(else_branch) = normalized_else.as_ref().and_then(|branch| branch.node) else {
        return false;
    };
    if if_branch.kind_str() != "true" || else_branch.kind_str() == "true" {
        return false;
    }
    let Some(selector) = predicate_selector(context, condition) else {
        return false;
    };
    !allowed.iter().any(|name| name == selector)
}

/// The name of a predicate call, when the node is one.
fn predicate_selector<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    // A call that takes a block is a `block` node upstream rather than a `send`.
    if node.kind_str() != "call" || node.field("block").is_some() {
        return None;
    }
    let selector = node.field("method")?;
    let name = context.source.node_text(selector);
    (name.ends_with('?') || name.ends_with('!')).then_some(name)
}

/// `branches_have_assignment?`: both branches assign the same name.
fn assignment_pair(
    context: &RuleContext<'_>,
    normalized_if: &Option<Branch<'_>>,
    normalized_else: &Option<Branch<'_>>,
) -> Option<String> {
    let name = |branch: &Option<Branch<'_>>| -> Option<String> {
        let node = branch.as_ref().and_then(|branch| branch.node)?;
        if node.kind_str() != "assignment" {
            return None;
        }
        let left = node.field("left")?;
        matches!(
            left.kind_str(),
            "identifier" | "instance_variable" | "class_variable" | "global_variable" | "constant"
        )
        .then(|| context.source.node_text(left).to_owned())
    };
    let (first, second) = (name(normalized_if)?, name(normalized_else)?);
    (first == second).then_some(first)
}

/// `branches_have_method?`: both branches send the same message to the same receiver with one
/// argument each.
fn branches_have_method(
    context: &RuleContext<'_>,
    normalized_if: &Option<Branch<'_>>,
    normalized_else: &Option<Branch<'_>>,
) -> bool {
    let (Some(first), Some(second)) = (
        normalized_if.as_ref().and_then(|branch| branch.node),
        normalized_else.as_ref().and_then(|branch| branch.node),
    ) else {
        return false;
    };
    single_argument_method(first)
        && single_argument_method(second)
        && selector(context, first) == selector(context, second)
        && receiver(context, first) == receiver(context, second)
}

fn single_argument_method(node: Node<'_>) -> bool {
    if !is_send(node) {
        return false;
    }
    let Some((argument, _)) = first_argument(node) else {
        return false;
    };
    if arguments(node).len() != 1 {
        return false;
    }
    // `argument_with_operator?`: a splat, a block pass and a `**` inside a brace-less hash all
    // spread rather than pass one value.
    !SPREAD_ARGUMENTS.contains(&argument.kind_str())
}

/// Whether upstream's parser would have built a `send` here. An operator written between two
/// operands is one there, however much the grammar spells it as a `binary`.
fn is_send(node: Node<'_>) -> bool {
    match node.kind_str() {
        "call" => node.field("block").is_none(),
        "binary" => true,
        // `a.foo = 1` is `(send a :foo= 1)` upstream, so `test.bar = foo` is one of these -- the
        // grammar spells it as an assignment whose left is a call. Leaving it out makes the cop
        // silent on `if foo / test.bar = foo / else / test.bar = 'baz' / end`, which upstream
        // reports through `branches_have_method?` (its `asgn_type?` deliberately excludes it).
        "assignment" => attribute_assignment(node).is_some(),
        _ => false,
    }
}

/// The call an attribute assignment writes through, when that is what the node is.
fn attribute_assignment<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    if node.kind_str() != "assignment" {
        return None;
    }
    let left = node.field("left")?;
    (left.kind_str() == "call" && left.field("receiver").is_some()).then_some(left)
}

/// A call's arguments grouped the way upstream's parser does: a trailing run of `key: value` pairs
/// is one `hash` argument there, however many pairs were written.
fn arguments<'tree>(node: Node<'tree>) -> Vec<(Node<'tree>, Range<usize>)> {
    // The value an attribute assignment writes is upstream's only argument to `foo=`.
    if attribute_assignment(node).is_some() {
        return node
            .field("right")
            .map(|right| vec![(right, right.byte_range())])
            .unwrap_or_default();
    }
    if node.kind_str() == "binary" {
        return node
            .field("right")
            .map(|right| vec![(right, right.byte_range())])
            .unwrap_or_default();
    }
    let written = node
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    let mut out: Vec<(Node<'tree>, Range<usize>)> = Vec::new();
    let mut hash: Vec<Node<'tree>> = Vec::new();
    for child in written {
        if matches!(child.kind_str(), "pair" | "hash_splat_argument") {
            hash.push(child);
            continue;
        }
        if let Some(first) = hash.first() {
            out.push((*first, first.start_byte()..hash[hash.len() - 1].end_byte()));
            hash.clear();
        }
        out.push((child, child.byte_range()));
    }
    if let Some(first) = hash.first() {
        out.push((*first, first.start_byte()..hash[hash.len() - 1].end_byte()));
    }
    out
}

fn first_argument<'tree>(node: Node<'tree>) -> Option<(Node<'tree>, Range<usize>)> {
    arguments(node).first().cloned()
}

fn selector<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    if let Some(call) = attribute_assignment(node) {
        // The selector upstream sees is `foo=`, but only the name is written; comparing the two
        // branches by the bare name answers the same question.
        return call
            .field("method")
            .map(|method| context.source.node_text(method));
    }
    let field = match node.kind_str() {
        "binary" => "operator",
        _ => "method",
    };
    node.field(field)
        .map(|selector| context.source.node_text(selector))
}

fn receiver<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    if let Some(call) = attribute_assignment(node) {
        return call
            .field("receiver")
            .map(|receiver| context.source.node_text(receiver));
    }
    let field = match node.kind_str() {
        "binary" => "left",
        _ => "receiver",
    };
    node.field(field)
        .map(|receiver| context.source.node_text(receiver))
}

/// One branch of the conditional, read from the clause the grammar hangs off the node.
fn branch<'tree>(node: Node<'tree>, want_consequence: bool) -> Option<Branch<'tree>> {
    let field = match (node.kind_str(), want_consequence) {
        ("conditional", true) => "consequence",
        ("conditional", false) => "alternative",
        (_, true) => "consequence",
        (_, false) => "alternative",
    };
    let clause = node.field(field)?;
    if node.kind_str() == "conditional" {
        return Some(Branch {
            node: Some(clause),
            range: clause.byte_range(),
            count: 1,
        });
    }
    let written = super::nodes::children(clause);
    let (first, last) = (written.first()?, written.last()?);
    Some(Branch {
        node: (written.len() == 1).then_some(*first),
        range: first.start_byte()..last.end_byte(),
        count: written.len(),
    })
}

/// `range_between(node.loc.question.begin_pos, node.loc.colon.end_pos)`.
fn question_to_colon(context: &RuleContext<'_>, node: Node<'_>) -> Option<Range<usize>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
    let question = children
        .iter()
        .find(|child| context.source.node_text(**child) == "?")?;
    let colon = children
        .iter()
        .find(|child| context.source.node_text(**child) == ":")?;
    Some(question.start_byte()..colon.end_byte())
}

#[allow(clippy::too_many_arguments)]
fn corrections(
    context: &RuleContext<'_>,
    node: Node<'_>,
    ternary: bool,
    condition: Node<'_>,
    range: Range<usize>,
    redundant: bool,
    with_method: bool,
    normalized_if: &Option<Branch<'_>>,
    normalized_else: &Option<Branch<'_>>,
    raw_if: &Option<Branch<'_>>,
    raw_else: &Option<Branch<'_>>,
) -> Option<Vec<Edit>> {
    // A comment anywhere inside would be lost by the rewrite.
    if context
        .comment_ranges()
        .iter()
        .any(|comment| node.start_byte() <= comment.start && comment.end <= node.end_byte())
    {
        return None;
    }
    if ternary && !with_method {
        let mut edits = vec![Edit {
            start: range.start,
            end: range.end,
            replacement: "||".to_owned(),
            safe: true,
        }];
        // A range written as the else branch has to keep its own parentheses.
        if let Some(else_branch) = normalized_else.as_ref().and_then(|branch| branch.node)
            && else_branch.kind_str() == "range"
        {
            edits.push(Edit {
                start: else_branch.start_byte(),
                end: else_branch.start_byte(),
                replacement: "(".to_owned(),
                safe: true,
            });
            edits.push(Edit {
                start: else_branch.end_byte(),
                end: else_branch.end_byte(),
                replacement: ")".to_owned(),
                safe: true,
            });
        }
        return Some(edits);
    }
    let replacement = match redundant {
        true => normalized_if.as_ref()?.source(context).to_owned(),
        false => ternary_form(
            context,
            node,
            condition,
            with_method,
            raw_if,
            raw_else,
            normalized_if,
        )?,
    };
    Some(vec![Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement,
        safe: true,
    }])
}

/// `make_ternary_form`: the two branches joined with `||`.
fn ternary_form(
    context: &RuleContext<'_>,
    node: Node<'_>,
    condition: Node<'_>,
    with_method: bool,
    raw_if: &Option<Branch<'_>>,
    raw_else: &Option<Branch<'_>>,
    normalized_if: &Option<Branch<'_>>,
) -> Option<String> {
    let if_branch = raw_if.as_ref()?;
    let else_branch = raw_else.as_ref()?;
    let arithmetic = if_branch
        .node
        .is_some_and(|node| arithmetic_operation(context, node));
    let mut form = format!(
        "{} || {}",
        if_source(context, condition, if_branch, with_method, arithmetic),
        else_source(context, else_branch, with_method, arithmetic, normalized_if)
    );
    if with_method
        && if_branch
            .node
            .is_some_and(|node| is_parenthesized(context, node))
    {
        form.push(')');
    }
    // `node.parent&.send_type?`: a conditional standing where a send takes an operand keeps the
    // `||` from spilling out of it.
    let wrapped = node
        .parent_of(context)
        .is_some_and(|parent| stands_in_a_send(context, parent));
    Some(match wrapped {
        true => format!("({form})"),
        false => form,
    })
}

/// Whether a node holding the conditional is one the parser would have built a `send` for.
///
/// The grammar spreads a send over several kinds, and two of those are not sends at all. A logical
/// operator is an `and`/`or` node upstream rather than a call, and an assignment is a send only
/// when it writes through `[]=` or an attribute writer -- `x = `, `@x = ` and `X = ` are their own
/// kinds of assignment, and an operator assignment is an `op-asgn` whatever it writes to. Safe
/// navigation answers to `csend_type?`, which `send_type?` is false for.
fn stands_in_a_send(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "argument_list" | "element_reference" => true,
        "call" => node.field("receiver").is_some(),
        "binary" => !matches!(
            binary_operator(context, node),
            Some("&&" | "||" | "and" | "or")
        ),
        "assignment" => node
            .field("left")
            .is_some_and(|left| match left.kind_str() {
                "element_reference" => true,
                "call" => !writes_through_safe_navigation(context, left),
                _ => false,
            }),
        _ => false,
    }
}

/// The operator token a binary expression is written with, which the grammar keeps unnamed.
fn binary_operator<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    let mut cursor = node.walk();
    let operator = node
        .children(&mut cursor)
        .find(|child| !child.is_named() && !child.is_extra())?;
    Some(context.source.node_text(operator))
}

fn writes_through_safe_navigation(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    let mut cursor = call.walk();
    call.children(&mut cursor)
        .any(|child| !child.is_named() && context.source.node_text(child) == "&.")
}

fn if_source(
    context: &RuleContext<'_>,
    condition: Node<'_>,
    if_branch: &Branch<'_>,
    with_method: bool,
    arithmetic: bool,
) -> String {
    let source = if_branch.source(context);
    if let Some(node) = if_branch.node {
        if with_method && is_parenthesized(context, node) {
            return source.strip_suffix(')').unwrap_or(source).to_owned();
        }
        if arithmetic
            && let (Some(receiver), Some(selector), Some((_, argument))) = (
                receiver(context, node),
                selector(context, node),
                first_argument(node),
            )
        {
            return format!("{receiver} {selector} ({}", context.source.slice(argument));
        }
        if node.kind_str() == "true" {
            let condition_source = context.source.node_text(condition);
            if arguments(condition).is_empty() || is_parenthesized(context, condition) {
                return condition_source.to_owned();
            }
            let (Some(selector), Some(argument)) =
                (condition.field("method"), first_argument(condition))
            else {
                return condition_source.to_owned();
            };
            return format!(
                "{}({})",
                context
                    .source
                    .slice(condition.start_byte()..selector.end_byte()),
                context
                    .source
                    .slice(argument.0.start_byte()..condition.end_byte())
            );
        }
    }
    source.to_owned()
}

fn else_source(
    context: &RuleContext<'_>,
    else_branch: &Branch<'_>,
    with_method: bool,
    arithmetic: bool,
    normalized_if: &Option<Branch<'_>>,
) -> String {
    let source = else_branch.source(context);
    let Some(node) = else_branch.node else {
        return source.to_owned();
    };
    if arithmetic && let Some((_, argument)) = first_argument(node) {
        return format!("{})", context.source.slice(argument));
    }
    if with_method && let Some((argument, range)) = first_argument(node) {
        return wrapped_argument(context, argument, context.source.slice(range));
    }
    if requires_parentheses(context, node) {
        return format!("({source})");
    }
    if node.kind_str() == "call"
        && !arguments(node).is_empty()
        && !is_parenthesized(context, node)
        && node
            .field("method")
            .is_some_and(|selector| selector.kind_str() != "operator")
    {
        let Some(selector) = node.field("method") else {
            return source.to_owned();
        };
        return format!(
            "{}({})",
            context.source.node_text(selector),
            arguments(node)
                .into_iter()
                .map(|(_, range)| context.source.slice(range))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // `branches_have_assignment?`: only the value assigned is carried over.
    if assignment_pair(context, normalized_if, &Some(else_branch.clone())).is_some()
        && let Some(expression) = node.field("right")
    {
        return wrapped_argument(context, expression, context.source.node_text(expression));
    }
    source.to_owned()
}

/// The argument of the else branch, parenthesized or braced where it would otherwise change what it
/// binds to.
fn wrapped_argument(context: &RuleContext<'_>, node: Node<'_>, source: &str) -> String {
    if requires_parentheses(context, node) {
        return format!("({source})");
    }
    if node.kind_str() == "pair" || (node.kind_str() == "hash" && !source.starts_with('{')) {
        return format!("{{ {source} }}");
    }
    source.to_owned()
}

fn requires_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if matches!(
        node.kind_str(),
        "if_modifier" | "unless_modifier" | "while_modifier" | "until_modifier" | "rescue_modifier"
    ) {
        return true;
    }
    if node.kind_str() == "range" {
        return true;
    }
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "and" | "or"))
}

/// `arithmetic_operation?`: one of the operators whose result is a new value.
fn arithmetic_operation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    is_send(node)
        && selector(context, node)
            .is_some_and(|selector| matches!(selector, "+" | "-" | "*" | "/" | "%" | "**"))
        && arguments(node).len() == 1
}

fn is_parenthesized(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.field("arguments")
        .is_some_and(|arguments| context.source.node_text(arguments).starts_with('('))
}
