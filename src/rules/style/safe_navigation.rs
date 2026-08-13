use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::node_equality::identical;
use crate::rules::send_node;

const MSG: &str = "Use safe navigation (`&.`) instead of checking if an object \
                   exists before calling the method.";

/// `minimum_target_ruby_version 2.3`.
const MINIMUM_VERSION: RubyVersion = RubyVersion::new(2, 3);

/// `LOGIC_JUMP_KEYWORDS`, of which only four name a node type: `raise`, `fail` and `throw` are
/// ordinary calls, so a body written as one of them is never excluded by this test.
const LOGIC_JUMP_KEYWORDS: &[&str] = &["break", "next", "return", "yield"];

/// `nil.methods` plus the cop's `AllowedMethods`, which is what a chained call is measured against.
fn nil_methods(context: &RuleContext<'_>) -> Vec<String> {
    crate::rules::lint::nil_methods::nil_methods(context)
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM_VERSION {
        return;
    }
    // `add_offense` refuses a range it has already reported, which is what keeps the pairs an
    // `and` chain yields from being counted once per nesting level.
    let mut reported: HashSet<(usize, usize)> = HashSet::new();
    let mut ignored: HashSet<usize> = HashSet::new();

    for node in context.nodes_of_any(&[
        "if",
        "unless",
        "elsif",
        "if_modifier",
        "unless_modifier",
        "conditional",
    ]) {
        on_if(context, offenses, &mut reported, node);
    }
    for node in context.nodes_of("binary") {
        if is_and(context, node) {
            on_and(context, offenses, &mut reported, &mut ignored, node);
        }
    }
}

fn on_if(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    reported: &mut HashSet<(usize, usize)>,
    node: Node<'_>,
) {
    // `allowed_if_condition?`: an `else` -- which an `elsif` counts as -- means the check guards a
    // choice rather than one call. A ternary has no `else` keyword, so `else?` is false for one
    // however plainly it has a second branch.
    if node.kind() == "elsif"
        || (node.kind() != "conditional" && node.child_by_field_name("alternative").is_some())
    {
        return;
    }
    let Some((checked_variable, method_chain)) = candidate(context, node) else {
        return;
    };
    // A body that jumps is never the chain, however well its receiver matches.
    if LOGIC_JUMP_KEYWORDS.contains(&method_chain.kind()) {
        return;
    }
    let Some(receiver) = find_matching_receiver_invocation(context, method_chain, checked_variable)
    else {
        return;
    };
    if !offending_node(
        context,
        node,
        checked_variable,
        method_chain,
        Some(receiver),
    ) {
        return;
    }
    let method_call = upstream_parent(receiver);
    let Some(method_call) = method_call else {
        return;
    };
    if dotless_operator_call(context, method_call) || is_double_colon(context, method_call) {
        return;
    }

    let body = method_chain;
    let mut edits = vec![
        removal(node.start_byte()..body.start_byte()),
        removal(body.end_byte()..node.end_byte()),
    ];
    // The block `on_if` hands `report_offense`.
    let corrections = |edits: &mut Vec<Edit>| {
        if is_safe_navigation(context, checked_variable) {
            edits.push(Edit {
                start: receiver.start_byte(),
                end: receiver.end_byte(),
                replacement: context.source.node_text(checked_variable).to_owned(),
                safe: false,
            });
        }
        if !is_safe_navigation(context, method_call) {
            if let Some(dot) = dot_of(method_call) {
                edits.push(insertion(dot.start_byte(), "&"));
            }
        }
    };
    corrections(&mut edits);
    report_offense(
        context,
        offenses,
        reported,
        node.byte_range(),
        node,
        method_chain,
        method_call,
        edits,
    );
}

fn on_and(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    reported: &mut HashSet<(usize, usize)>,
    ignored: &mut HashSet<usize>,
    node: Node<'_>,
) {
    for (left, operator, right) in collect_and_clauses(context, node) {
        let not_nil_check = not_nil_check(context, left);
        let lhs_receiver = not_nil_check.unwrap_or(left);
        let Some(stripped) = strip_begin(right) else {
            continue;
        };
        let rhs_receiver = find_matching_receiver_invocation(context, stripped, lhs_receiver);

        if !context
            .setting::<bool>("ConvertCodeThatCanStartToReturnNil")
            .unwrap_or(false)
            && not_nil_check.is_some()
        {
            continue;
        }
        if !offending_node(context, node, lhs_receiver, right, rhs_receiver) {
            continue;
        }
        let Some(rhs_receiver) = rhs_receiver else {
            continue;
        };

        // Every clause of a chain of `and` nodes is walked, so a check of an object further along
        // the chain has to be told from a check of the object itself.
        let lhs_method_chain = find_method_chain(context, lhs_receiver);
        if lhs_method_chain.id() != lhs_receiver.id() && not_nil_check.is_none() {
            continue;
        }

        let mut edits = vec![
            removal(with_trailing_space(context, left.byte_range())),
            removal(with_trailing_space(context, operator.clone())),
            Edit {
                start: rhs_receiver.start_byte(),
                end: rhs_receiver.end_byte(),
                replacement: context.source.node_text(lhs_receiver).to_owned(),
                safe: false,
            },
        ];
        let offense_range = left.start_byte()..right.end_byte();
        if ignored.contains(&node.id()) {
            edits.clear();
        }
        report_offense(
            context,
            offenses,
            reported,
            offense_range,
            node,
            right,
            rhs_receiver,
            edits,
        );
        ignored.insert(node.id());
    }
}

/// `report_offense`: the removals and the cop's own edits, then the comments the collapse would
/// have swallowed and the `&` every remaining call in the chain needs.
#[allow(clippy::too_many_arguments)]
fn report_offense(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    reported: &mut HashSet<(usize, usize)>,
    offense_range: Range<usize>,
    node: Node<'_>,
    rhs: Node<'_>,
    rhs_receiver: Node<'_>,
    mut edits: Vec<Edit>,
) {
    if !reported.insert((offense_range.start, offense_range.end)) {
        return;
    }
    let mut offense = context.offense(MSG, offense_range);
    // An `or` on the right cannot be corrected: the check guards every clause of it, and removing
    // it would need a `&.` on each.
    if edits.is_empty() || and_with_rhs_or(context, node) {
        offenses.push(offense);
        return;
    }
    let comments = comments_to_move(context, node);
    if !comments.is_empty() {
        edits.push(insertion(rhs.start_byte(), format!("{comments}\n")));
        // `insert_before(method_call, ...)` hands the corrector the chain's own range, which is
        // what keeps this insertion the parent of the edits inside it rather than their sibling.
        offense = offense.corrections_anchored_at(rhs.byte_range());
    }
    add_safe_nav_to_all_methods_in_chain(context, &mut edits, rhs_receiver, rhs);
    offenses.push(offense.corrected_by_all(edits));
}

/// `add_safe_nav_to_all_methods_in_chain`.
fn add_safe_nav_to_all_methods_in_chain(
    context: &RuleContext<'_>,
    edits: &mut Vec<Edit>,
    start: Node<'_>,
    chain: Node<'_>,
) {
    for ancestor in upstream_ancestors(start) {
        // `break unless ancestor.type?(:call, :any_block)`.
        if !(ancestor.is_block || is_call(context, ancestor.node)) {
            break;
        }
        if !ancestor.is_block
            && is_send(context, ancestor.node)
            && !is_operator_method(context, ancestor.node)
        {
            if let Some(dot) = dot_of(ancestor.node) {
                edits.push(insertion(dot.start_byte(), "&"));
            }
        }
        if same_as_chain(&ancestor, chain) {
            break;
        }
    }
}

/// `offending_node?`.
fn offending_node(
    context: &RuleContext<'_>,
    node: Node<'_>,
    lhs_receiver: Node<'_>,
    rhs: Node<'_>,
    rhs_receiver: Option<Node<'_>>,
) -> bool {
    let Some(rhs_receiver) = rhs_receiver else {
        return false;
    };
    if !matching_nodes(context, Some(lhs_receiver), Some(rhs_receiver)) {
        return false;
    }
    // `use_var_only_in_unless_modifier?`: `foo.bar unless foo` checks the object rather than a
    // call on it, so folding it would change what the condition means.
    if matches!(node.kind(), "unless" | "unless_modifier")
        && upstream_parent_is_send(context, lhs_receiver).is_none()
    {
        return false;
    }
    let Some(method) = upstream_parent(rhs_receiver) else {
        return false;
    };
    if chain_length(context, rhs, rhs_receiver) > max_chain_length(context) {
        return false;
    }
    if unsafe_method_used(context, node, rhs, method) {
        return false;
    }
    !(is_send(context, rhs) && method_name(context, rhs) == Some("empty?".to_owned()))
}

/// `chain_length`: how many calls stand between the receiver and the end of the chain.
fn chain_length(context: &RuleContext<'_>, chain: Node<'_>, method: Node<'_>) -> usize {
    let mut total = 0;
    for ancestor in upstream_ancestors(method) {
        if ancestor.is_block || !is_call(context, ancestor.node) {
            continue;
        }
        total += 1;
        if same_as_chain(&ancestor, chain) {
            break;
        }
    }
    total
}

fn max_chain_length(context: &RuleContext<'_>) -> usize {
    context.setting("MaxChainLength").unwrap_or(2)
}

/// `unsafe_method_used?`.
fn unsafe_method_used(
    context: &RuleContext<'_>,
    node: Node<'_>,
    chain: Node<'_>,
    method: Node<'_>,
) -> bool {
    if unsafe_method(context, node, method) {
        return true;
    }
    let allowed = nil_methods(context);
    for ancestor in upstream_ancestors(method) {
        if ancestor.is_block || !is_send(context, ancestor.node) {
            continue;
        }
        if !context.cop_enabled("Lint/SafeNavigationChain") {
            return true;
        }
        if unsafe_method(context, node, ancestor.node) {
            return true;
        }
        if method_name(context, ancestor.node).is_some_and(|name| allowed.contains(&name)) {
            return true;
        }
        if same_as_chain(&ancestor, chain) {
            return false;
        }
    }
    false
}

/// `unsafe_method?`.
fn unsafe_method(context: &RuleContext<'_>, node: Node<'_>, send: Node<'_>) -> bool {
    if negated(context, send) {
        return true;
    }
    if node.kind() == "conditional" {
        return false;
    }
    is_setter(context, send) || (!has_dot(context, send) && !is_safe_navigation(context, send))
}

/// `negated?`: whether the chain ends in a `!`, which the fold would put on the wrong side.
fn negated(context: &RuleContext<'_>, send: Node<'_>) -> bool {
    match upstream_parent_is_send(context, send) {
        Some(parent) => negated(context, parent),
        None => is_send(context, send) && method_name(context, send).as_deref() == Some("!"),
    }
}

/// `method_called?`: whether the node is the receiver of a further `send`.
fn upstream_parent_is_send<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<Node<'tree>> {
    upstream_parent(node).filter(|parent| is_send(context, *parent))
}

/// `dotless_operator_call?`: an operator or an index written without a dot has nowhere to put the
/// `&`.
fn dotless_operator_call(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    if dotless_operator_method(context, call) {
        return true;
    }
    let mut call = call;
    while let Some(parent) = upstream_parent(call).filter(|parent| is_send(context, *parent)) {
        call = parent;
    }
    dotless_operator_method(context, call)
}

fn dotless_operator_method(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    if has_dot(context, call) {
        return false;
    }
    let Some(name) = method_name(context, call) else {
        return false;
    };
    name == "[]" || name == "[]=" || super::nodes::is_operator_method(&name)
}

/// The comments the collapse would delete, joined the way upstream joins them.
fn comments_to_move(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let first = node.start_position().row + 1;
    let last = node.end_position().row + 1;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut begin = first;
    for child in child_nodes(node) {
        ranges.push((begin, child.start_position().row + 1));
        begin = child.end_position().row + 1;
    }
    ranges.push((begin, last));

    let mut found = Vec::new();
    for comment in context.comment_ranges() {
        let line = context.source.line_column(comment.start).0;
        if ranges
            .iter()
            .any(|(start, end)| line >= *start && line < *end)
        {
            found.push(context.source.slice(comment.clone()));
        }
    }
    found.join("\n")
}

/// `node.child_nodes`, in the order the parser lists them. Only the shapes this cop reports on
/// need an answer: a conditional and an `and`.
fn child_nodes<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    if node.kind() == "binary" {
        for field in ["left", "right"] {
            if let Some(operand) = node.child_by_field_name(field) {
                found.push(operand);
            }
        }
        return found;
    }
    if let Some(condition) = node.child_by_field_name("condition") {
        found.push(condition);
    }
    if node.kind() == "conditional" {
        for field in ["consequence", "alternative"] {
            if let Some(branch) = node.child_by_field_name(field) {
                found.push(branch);
            }
        }
        return found;
    }
    if let Some(body) = if_body(node) {
        found.push(body);
    }
    found
}

/// The one statement a conditional's branch holds, which the grammar wraps in a `then` unless the
/// conditional was written after it.
fn if_body<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind() {
        "if_modifier" | "unless_modifier" => node.child_by_field_name("body"),
        _ => super::conditional::body_of(node.child_by_field_name("consequence")?).single(),
    }
}

/// `modifier_if_safe_navigation_candidate` and `ternary_safe_navigation_candidate`, as the pair of
/// the checked variable and the chain the branch holds.
fn candidate<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    let condition = node.child_by_field_name("condition")?;
    if node.kind() == "conditional" {
        let (consequence, alternative) = (
            node.child_by_field_name("consequence")?,
            node.child_by_field_name("alternative")?,
        );
        // `(if (send $_ {:nil? :!}) nil $_)`.
        if consequence.kind() == "nil" {
            let variable = nil_or_bang_check(context, condition).unwrap_or(condition);
            return Some((variable, alternative));
        }
        if alternative.kind() != "nil" {
            return None;
        }
        // `(if (send (send $_ :nil?) :!) $_ nil)` and `(if $_ $_ nil)`.
        let variable = not_nil_check(context, condition).unwrap_or(condition);
        return Some((variable, consequence));
    }
    let body = if_body(node)?;
    match node.kind() {
        // The body sits where the parser puts the `else` branch, so the condition is read the
        // other way round.
        "unless" | "unless_modifier" => {
            let variable = nil_or_bang_check(context, condition).unwrap_or(condition);
            Some((variable, body))
        }
        _ => {
            let variable = not_nil_check(context, condition).unwrap_or(condition);
            Some((variable, body))
        }
    }
}

/// `(send $_ {:nil? :!})`.
fn nil_or_bang_check<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind() {
        "call" if method_name(context, node).as_deref() == Some("nil?") => {
            node.child_by_field_name("receiver")
        }
        "unary" if is_bang(context, node) => node.child_by_field_name("operand"),
        _ => None,
    }
}

/// `not_nil_check?`: `(send (send $_ :nil?) :!)`.
fn not_nil_check<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    if node.kind() != "unary" || !is_bang(context, node) {
        return None;
    }
    let inner = node.child_by_field_name("operand")?;
    if inner.kind() != "call" || method_name(context, inner).as_deref() != Some("nil?") {
        return None;
    }
    inner.child_by_field_name("receiver")
}

/// `strip_begin`: `{ (begin $!begin) $!(begin) }`.
fn strip_begin<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    if node.kind() != "parenthesized_statements" {
        return Some(node);
    }
    match super::nodes::children(node).as_slice() {
        [only] if only.kind() != "parenthesized_statements" => Some(*only),
        _ => None,
    }
}

/// `and_with_rhs_or?`: `(and _ {or (begin or)})`.
fn and_with_rhs_or(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if !is_and(context, node) {
        return false;
    }
    let Some(right) = node.child_by_field_name("right") else {
        return false;
    };
    if is_or(context, right) {
        return true;
    }
    right.kind() == "parenthesized_statements"
        && matches!(super::nodes::children(right).as_slice(), [only] if is_or(context, *only))
}

/// `find_matching_receiver_invocation`.
fn find_matching_receiver_invocation<'tree>(
    context: &RuleContext<'_>,
    chain: Node<'tree>,
    variable: Node<'tree>,
) -> Option<Node<'tree>> {
    let receiver = receiver_of(context, chain);
    if matching_nodes(context, receiver, Some(variable)) {
        return receiver;
    }
    find_matching_receiver_invocation(context, receiver?, variable)
}

/// `matching_nodes?`.
fn matching_nodes(
    context: &RuleContext<'_>,
    left: Option<Node<'_>>,
    right: Option<Node<'_>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            identical(left, right, context) || matching_call_nodes(context, left, right)
        }
        _ => false,
    }
}

/// `matching_call_nodes?`: the same receiver and method, whether or not either was written with
/// safe navigation.
fn matching_call_nodes(context: &RuleContext<'_>, left: Node<'_>, right: Node<'_>) -> bool {
    if left.kind() != "call" || right.kind() != "call" {
        return false;
    }
    if method_name(context, left) != method_name(context, right) {
        return false;
    }
    if !matching_nodes(
        context,
        left.child_by_field_name("receiver"),
        right.child_by_field_name("receiver"),
    ) {
        return false;
    }
    let (left, right) = (send_node::arguments(left), send_node::arguments(right));
    left.len() == right.len()
        && left.iter().zip(&right).all(|(left, right)| {
            left.parts().len() == right.parts().len()
                && left
                    .parts()
                    .iter()
                    .zip(right.parts())
                    .all(|(left, right)| identical(*left, *right, context))
        })
}

/// `find_method_chain`: the outermost call the node is the receiver of.
fn find_method_chain<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Node<'tree> {
    let mut current = node;
    while let Some(parent) = upstream_parent(current).filter(|parent| is_call(context, *parent)) {
        current = parent;
    }
    current
}

/// `not x` is `(send x :!)` just as `!x` is.
fn is_bang(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.child_by_field_name("operator")
        .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
}

/// One element of the flattened `and` chain: a clause, or the operator that joined two of them.
enum Part<'tree> {
    Node(Node<'tree>),
    Operator(Range<usize>),
}

impl Part<'_> {
    fn start(&self) -> usize {
        match self {
            Self::Node(node) => node.start_byte(),
            Self::Operator(range) => range.start,
        }
    }
}

/// `collect_and_clauses`: the clauses of a chain of `and` nodes, paired with the operator that
/// joined them, and then taken two at a time.
fn collect_and_clauses<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Vec<(Node<'tree>, Range<usize>, Node<'tree>)> {
    let mut parts = and_parts(context, node);
    for descendant in descendants(node) {
        if !is_and(context, descendant) || has_block_ancestor(context, descendant) {
            continue;
        }
        parts.extend(and_parts(context, descendant));
    }
    parts.sort_by_key(Part::start);

    // `each_slice(2)`: every clause with whatever followed it.
    let mut clauses: Vec<(Node<'tree>, Option<Range<usize>>)> = Vec::new();
    let mut index = 0;
    while index < parts.len() {
        if let Part::Node(node) = parts[index] {
            let operator = match parts.get(index + 1) {
                Some(Part::Operator(range)) => Some(range.clone()),
                _ => None,
            };
            clauses.push((node, operator));
        }
        index += 2;
    }

    // `each_cons(2)`: every neighbouring pair of those.
    let mut found = Vec::new();
    for pair in clauses.windows(2) {
        let (left, operator) = &pair[0];
        let (right, _) = &pair[1];
        if let Some(operator) = operator {
            found.push((*left, operator.clone(), *right));
        }
    }
    found
}

/// `and_parts`.
fn and_parts<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Vec<Part<'tree>> {
    let mut parts = Vec::new();
    if let Some(operator) = node.child_by_field_name("operator") {
        parts.push(Part::Operator(operator.byte_range()));
    }
    if let Some(right) = node.child_by_field_name("right") {
        if !and_inside_begin(context, right) {
            parts.push(Part::Node(right));
        }
    }
    if let Some(left) = node.child_by_field_name("left") {
        if !is_and(context, left) && !and_inside_begin(context, left) {
            parts.push(Part::Node(left));
        }
    }
    parts
}

/// `and_inside_begin?`: `` `(begin and ...) ``.
fn and_inside_begin(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    send_node::any_descendant(node, &mut |candidate| {
        candidate.kind() == "parenthesized_statements"
            && super::nodes::children(candidate)
                .first()
                .is_some_and(|first| is_and(context, *first))
    })
}

fn has_block_ancestor(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if node_is_block(parent) && !is_implicit_block(context, parent) {
            return true;
        }
        current = parent;
    }
    false
}

fn descendants<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        for child in super::nodes::children(current).into_iter().rev() {
            stack.push(child);
        }
        if current.id() != node.id() {
            found.push(current);
        }
    }
    found
}

fn removal(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: false,
    }
}

fn insertion(at: usize, text: impl Into<String>) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text.into(),
        safe: false,
    }
}

/// `range_with_surrounding_space(range: ..., side: :right)`: spaces and tabs, then newlines.
fn with_trailing_space(context: &RuleContext<'_>, range: Range<usize>) -> Range<usize> {
    let text = context.source.text().as_bytes();
    let mut end = range.end;
    while end < text.len() && matches!(text[end], b' ' | b'\t') {
        end += 1;
    }
    while end < text.len() && text[end] == b'\n' {
        end += 1;
    }
    range.start..end
}

fn is_and(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "binary"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "&&" | "and"))
}

fn is_or(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "binary"
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "||" | "or"))
}

/// Whether that block is a `numblock` or an `itblock`, which `each_ancestor(:block)` does not
/// match.
fn is_implicit_block(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.child_by_field_name("block") {
        Some(block) => super::block_args::implicit(context, block),
        None => false,
    }
}

/// Whether upstream's parser builds a `send` or a `csend` for the node.
fn is_call(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "call" | "element_reference" => true,
        "binary" => !is_and(context, node) && !is_or(context, node),
        "unary" => node
            .child_by_field_name("operator")
            .is_some_and(|operator| context.source.node_text(operator) != "defined?"),
        "assignment" => is_setter(context, node),
        _ => false,
    }
}

/// Whether upstream's parser builds a `send` -- not a `csend` -- for the node.
fn is_send(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    is_call(context, node) && !is_safe_navigation(context, node)
}

/// `SendNode#assignment?`: a call to a setter, which the grammar writes as an assignment whose
/// left-hand side is itself a call.
fn is_setter(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "assignment" => node
            .child_by_field_name("left")
            .is_some_and(|left| matches!(left.kind(), "call" | "element_reference")),
        "call" => method_name(context, node)
            .is_some_and(|name| name.ends_with('=') && !super::nodes::is_operator_method(&name)),
        _ => false,
    }
}

/// `SendNode#method_name`, for whichever shape the grammar gave the call.
fn method_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    match node.kind() {
        "call" => Some(
            context
                .source
                .node_text(node.child_by_field_name("method")?)
                .to_owned(),
        ),
        "element_reference" => Some("[]".to_owned()),
        "binary" => Some(
            context
                .source
                .node_text(node.child_by_field_name("operator")?)
                .to_owned(),
        ),
        "unary" => {
            let operator = context
                .source
                .node_text(node.child_by_field_name("operator")?);
            // The parser names a unary minus `:-@` to tell it from the binary one.
            Some(match operator {
                "-" | "+" | "~" => format!("{operator}@"),
                other => other.to_owned(),
            })
        }
        "assignment" => {
            let left = node.child_by_field_name("left")?;
            match left.kind() {
                "element_reference" => Some("[]=".to_owned()),
                "call" => Some(format!("{}=", method_name(context, left)?)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_operator_method(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    method_name(context, node).is_some_and(|name| super::nodes::is_operator_method(&name))
}

/// `loc.dot`, which is the `.`, the `&.` or the `::` a call was written with.
fn dot_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind() {
        "call" => node.child_by_field_name("operator"),
        "assignment" => dot_of(node.child_by_field_name("left")?),
        _ => None,
    }
}

fn has_dot(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let _ = context;
    dot_of(node).is_some()
}

fn is_safe_navigation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    dot_of(node).is_some_and(|dot| context.source.node_text(dot) == "&.")
}

fn is_double_colon(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    dot_of(node).is_some_and(|dot| context.source.node_text(dot) == "::")
}

/// `Node#receiver`, for whichever shape the grammar gave the call.
fn receiver_of<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind() {
        "call" => node.child_by_field_name("receiver"),
        "element_reference" => node.child_by_field_name("object"),
        "binary" if !is_and(context, node) && !is_or(context, node) => {
            node.child_by_field_name("left")
        }
        "unary"
            if node
                .child_by_field_name("operator")
                .is_some_and(|operator| context.source.node_text(operator) != "defined?") =>
        {
            node.child_by_field_name("operand")
        }
        "assignment" if is_setter(context, node) => {
            receiver_of(context, node.child_by_field_name("left")?)
        }
        _ => None,
    }
}

/// One ancestor as upstream's parser has it.
///
/// A call written with a block is two nodes there -- a `block` wrapped around a `send` -- where the
/// grammar has one, so the same node stands for both and which of the two an ancestor is depends
/// on where the walk came from: the block's body has only the `block` above it, while the call's
/// receiver and arguments have the `send` and then the `block`.
#[derive(Clone, Copy)]
struct Ancestor<'tree> {
    node: Node<'tree>,
    is_block: bool,
}

/// Node kinds whose statements upstream folds into one `begin` once there is more than one of
/// them, and which stand for nothing at all while they hold a single statement.
const STATEMENT_CONTAINERS: &[&str] = &[
    "program",
    "then",
    "else",
    "body_statement",
    "block_body",
    "do",
];

/// Whether the node is one upstream's parser builds a `block`, `numblock` or `itblock` for.
fn node_is_block(node: Node<'_>) -> bool {
    node.kind() == "lambda" || node.child_by_field_name("block").is_some()
}

/// `ancestor == method_chain`, with the two aspects of a call written with a block told apart.
fn same_as_chain(ancestor: &Ancestor<'_>, chain: Node<'_>) -> bool {
    ancestor.node.id() == chain.id() && ancestor.is_block == node_is_block(chain)
}

/// One step up the tree, as upstream's parser would take it, and whether the step passed through
/// the block of a call.
fn parent_step<'tree>(node: Node<'tree>) -> Option<(Node<'tree>, bool)> {
    let mut current = node;
    let mut through_block = false;
    loop {
        let parent = current.parent()?;
        match parent.kind() {
            // `(...)` is a `begin` however little it holds.
            "parenthesized_statements" => return Some((parent, through_block)),
            kind if STATEMENT_CONTAINERS.contains(&kind) => {
                if super::conditional::self_statements(parent).len() > 1 {
                    return Some((parent, through_block));
                }
                current = parent;
            }
            // The grammar hangs a block off the call it belongs to, and wraps arguments in a list
            // of their own; upstream has neither node.
            "block" | "do_block" => {
                through_block = true;
                current = parent;
            }
            "argument_list" => current = parent,
            _ => return Some((parent, through_block)),
        }
    }
}

/// `node.parent`, with the extra node the grammar interposes for a setter call folded away: `a.b =
/// v` is one `send` upstream where the grammar has an `assignment` wrapped around the `a.b` call.
fn upstream_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    first_ancestor(node).filter(|ancestor| !ancestor.is_block).map(|ancestor| ancestor.node)
}

fn first_ancestor<'tree>(node: Node<'tree>) -> Option<Ancestor<'tree>> {
    upstream_ancestors(node).into_iter().next()
}

fn fold_setter<'tree>(node: Node<'tree>) -> Node<'tree> {
    if !matches!(node.kind(), "call" | "element_reference") {
        return node;
    }
    match node.parent() {
        Some(parent)
            if parent.kind() == "assignment"
                && parent
                    .child_by_field_name("left")
                    .is_some_and(|left| left.id() == node.id()) =>
        {
            parent
        }
        _ => node,
    }
}

fn upstream_ancestors<'tree>(node: Node<'tree>) -> Vec<Ancestor<'tree>> {
    let mut found = Vec::new();
    let mut current = fold_setter(node);
    while let Some((parent, through_block)) = parent_step(current) {
        let parent = fold_setter(parent);
        if parent.id() == current.id() {
            current = parent;
            continue;
        }
        if node_is_block(parent) {
            // Reached from the block's body the call is only the `block`; from anywhere else the
            // `send` comes first and the `block` stands above it.
            if !through_block {
                found.push(Ancestor {
                    node: parent,
                    is_block: false,
                });
            }
            found.push(Ancestor {
                node: parent,
                is_block: true,
            });
        } else {
            found.push(Ancestor {
                node: parent,
                is_block: false,
            });
        }
        current = parent;
    }
    found
}
