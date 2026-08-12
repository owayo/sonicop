//! `Layout/MultilineMethodCallIndentation`.

use std::ops::Range;

use super::multiline_expression::{Mixin, UpKind, UpNode, within};
use super::support::{alignment_corrections, holds_block_comment, string_interiors};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Style {
    Aligned,
    Indented,
    IndentedRelativeToReceiver,
}

/// Upstream joins `loc.dot` to `loc.selector` without guarding the selector in three places, so a
/// call written as `foo.(1)` -- which has a dot and no name -- raises there. RuboCop catches the
/// cop error per node and drops the offense with it, which is what this stands for.
struct Aborted;

/// What `offending_range` leaves behind for `message` and `autocorrect`: the range to report, how
/// far it has to move, and the base it was measured against.
struct Offending {
    rhs: Range<usize>,
    delta: i64,
    base: Option<Range<usize>>,
    /// `@hash_pair_base_column`, which only the `indented` style sets.
    hash_pair_base_column: Option<i64>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mixin = Mixin::new(context, context.setting::<i64>("IndentationWidth"));
    let style = match context.setting::<String>("EnforcedStyle").as_deref() {
        Some("indented") => Style::Indented,
        Some("indented_relative_to_receiver") => Style::IndentedRelativeToReceiver,
        _ => Style::Aligned,
    };

    for ts in context.nodes_of_any(&["call", "method_call", "assignment"]) {
        let node = UpNode::plain(ts);
        if !matches!(node.kind(context), UpKind::Send | UpKind::Csend) {
            continue;
        }
        // The call an assignment was written over is fused into the setter send upstream builds,
        // so it is no node of its own there.
        if node.is_fused_setter_target() {
            continue;
        }
        let Some(receiver) = node.receiver(context) else {
            continue;
        };
        if node.method_name(context).as_deref() == Some("[]") {
            continue;
        }
        // `relevant_node?`: only method calls with a dot operator.
        if node.dot(context).is_none() {
            continue;
        }
        let Some(rhs) = right_hand_side(&mixin, node) else {
            continue;
        };
        let lhs = mixin.left_hand_side(receiver);
        let Ok(Some(offending)) = offending_range(&mixin, style, node, lhs, &rhs) else {
            continue;
        };
        offenses.push(report(&mixin, style, node, lhs, offending));
    }
}

/// `MultilineMethodCallIndentation#right_hand_side`.
fn right_hand_side<'tree>(mixin: &Mixin<'_, 'tree>, node: UpNode<'tree>) -> Option<Range<usize>> {
    let context = mixin.context;
    let dot = node.dot(context)?;
    let selector = node.selector();
    let dotted = matches!(context.source.slice(dot.clone()), "." | "&.");
    match selector {
        Some(selector) if dotted && mixin.line(dot.start) == mixin.line(selector.start) => {
            Some(dot.start..selector.end)
        }
        Some(selector) => Some(selector),
        // `implicit_call?`: `foo.(1)` has a dot and no name, so the range runs to the paren.
        None => node
            .arguments_begin(context)
            .map(|begin| dot.start..begin.end),
    }
}

fn report<'tree>(
    mixin: &Mixin<'_, 'tree>,
    style: Style,
    node: UpNode<'tree>,
    lhs: UpNode<'tree>,
    offending: Offending,
) -> Offense {
    let context = mixin.context;
    let message = message(mixin, style, node, lhs, &offending);
    let mut offense = context.offense(message, offending.rhs.clone());
    let edits = autocorrect(mixin, node, &offending);
    if !edits.is_empty() {
        offense = offense.corrected_by_all(edits);
    }
    offense
}

/// `MultilineMethodCallIndentation#autocorrect`.
fn autocorrect<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    offending: &Offending,
) -> Vec<Edit> {
    let context = mixin.context;
    let Some(block) = node.block_node() else {
        return corrections(context, offending.rhs.clone(), offending.delta, false);
    };
    // A call carrying a block moves its own line and the block's body and `end` rather than the
    // whole span, which would take the block's opening line with it twice.
    let line = mixin.line(offending.rhs.start);
    let selector_line = context.source.line_range(line);
    let selector_line = selector_line.start..line_end(context, line);
    let mut edits = corrections(context, selector_line, offending.delta, false);
    if let Some(body) = block.body() {
        edits.extend(corrections(context, body, offending.delta, true));
    }
    if let Some(end) = block.block_end() {
        let start_line = mixin.line(end.start);
        let end_line = mixin.line(end.end);
        let whole = context.source.line_start(start_line)..line_end(context, end_line);
        edits.extend(corrections(context, whole, offending.delta, false));
    }
    edits
}

/// `AlignmentCorrector.correct`, which refuses to move a span holding a `=begin` block comment and
/// leaves the interior of string literals where it is when it was handed a node.
fn corrections(
    context: &RuleContext<'_>,
    expr: Range<usize>,
    delta: i64,
    from_node: bool,
) -> Vec<Edit> {
    if holds_block_comment(context, &expr) {
        return Vec::new();
    }
    let taboo = if from_node {
        string_interiors(context, &expr)
    } else {
        Vec::new()
    };
    alignment_corrections(context, expr, delta, &taboo)
}

/// The end of a line without its newline, which is what `buffer.line_range` and
/// `range_by_whole_lines` both stop at.
fn line_end(context: &RuleContext<'_>, line: usize) -> usize {
    let range = context.source.line_range(line);
    range.start
        + context
            .source
            .line(line)
            .trim_end_matches(['\n', '\r'])
            .len()
}

/// `MultilineMethodCallIndentation#offending_range`.
fn offending_range<'tree>(
    mixin: &Mixin<'_, 'tree>,
    style: Style,
    node: UpNode<'tree>,
    lhs: UpNode<'tree>,
    rhs: &Range<usize>,
) -> Result<Option<Offending>, Aborted> {
    let context = mixin.context;
    if !mixin.begins_its_line(rhs) {
        return Ok(None);
    }
    let pair_ancestor = find_pair_ancestor(mixin, node);
    if pair_ancestor.is_some() && style == Style::Aligned {
        return check_hash_pair_indentation(mixin, node, lhs, rhs, pair_ancestor);
    }
    if let Some(pair) = pair_ancestor
        && style == Style::Indented
        && find_base_receiver(context, node).kind(context) == UpKind::Hash
    {
        return Ok(check_hash_pair_indented_style(mixin, rhs, pair));
    }
    let skip = match pair_ancestor {
        Some(_) => inside_multiline_chain_arg(mixin, pair_ancestor),
        None => mixin.not_for_this_cop(node),
    };
    if skip {
        return Ok(None);
    }
    check_regular_indentation(mixin, style, node, lhs, rhs)
}

/// `MultilineMethodCallIndentation#find_pair_ancestor`.
fn find_pair_ancestor<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    let context = mixin.context;
    let range = node.range(context);
    for ancestor in node.ancestors() {
        if ancestor.kind(context) == UpKind::Pair {
            return Some(ancestor);
        }
        if mixin.grouped_expression(ancestor) || mixin.inside_arg_list_parentheses(&range, ancestor)
        {
            return None;
        }
    }
    None
}

/// `MultilineMethodCallIndentation#find_base_receiver`.
fn find_base_receiver<'tree>(context: &RuleContext<'_>, node: UpNode<'tree>) -> UpNode<'tree> {
    let mut base = node;
    while let Some(receiver) = base.receiver(context) {
        base = receiver;
    }
    base
}

/// `MultilineMethodCallIndentation#first_call_has_a_dot`.
fn first_call_has_a_dot<'tree>(
    context: &RuleContext<'_>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    let mut current = find_base_receiver(context, node).parent()?;
    loop {
        if current.dot(context).is_some() {
            return Some(current);
        }
        current = current.parent()?;
    }
}

/// `node.loc.dot.join(node.loc.selector)`.
fn dot_through_selector(context: &RuleContext<'_>, node: UpNode<'_>) -> Option<Range<usize>> {
    let dot = node.dot(context)?;
    let selector = node.selector()?;
    Some(dot.start..selector.end)
}

/// The same join at the three call sites upstream leaves unguarded.
fn dot_through_selector_or_abort(
    context: &RuleContext<'_>,
    node: UpNode<'_>,
) -> Result<Range<usize>, Aborted> {
    dot_through_selector(context, node).ok_or(Aborted)
}

/// `MultilineMethodCallIndentation#check_hash_pair_indentation`.
fn check_hash_pair_indentation<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    lhs: UpNode<'tree>,
    rhs: &Range<usize>,
    pair_ancestor: Option<UpNode<'tree>>,
) -> Result<Option<Offending>, Aborted> {
    let context = mixin.context;
    let mut base = find_hash_pair_alignment_base(context, node)?;
    if base.is_none() && inside_multiline_chain_arg(mixin, pair_ancestor) {
        return Ok(None);
    }
    if base.is_none() {
        base = first_dot_alignment_base(mixin, node, rhs)?.or_else(|| Some(lhs.range(context)));
    }
    if aligned_with_first_line_dot(mixin, node, rhs) {
        return Ok(None);
    }
    let Some(base) = base else {
        return Ok(None);
    };
    Ok(delta_offense(
        mixin,
        rhs,
        mixin.column(base.start),
        Some(base),
        None,
    ))
}

/// `MultilineMethodCallIndentation#find_hash_pair_alignment_base`.
fn find_hash_pair_alignment_base(
    context: &RuleContext<'_>,
    node: UpNode<'_>,
) -> Result<Option<Range<usize>>, Aborted> {
    let Some(receiver) = node.receiver(context) else {
        return Ok(None);
    };
    if find_base_receiver(context, receiver).kind(context) != UpKind::Hash {
        return Ok(None);
    }
    let Some(first_call) = first_call_has_a_dot(context, node) else {
        return Ok(None);
    };
    dot_through_selector_or_abort(context, first_call).map(Some)
}

/// `MultilineMethodCallIndentation#first_dot_alignment_base`.
fn first_dot_alignment_base<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    rhs: &Range<usize>,
) -> Result<Option<Range<usize>>, Aborted> {
    let context = mixin.context;
    if !starts_with_dot(context, rhs) {
        return Ok(None);
    }
    let (Some(first_call), ..) = (first_call_has_a_dot(context, node), ()) else {
        return Ok(None);
    };
    let Some(dot) = first_call.dot(context) else {
        return Ok(None);
    };
    if first_call.same(node) {
        return Ok(None);
    }
    if let Some(after_block) = after_multiline_block_base(mixin, first_call, node)? {
        return Ok(Some(after_block));
    }
    let Some(receiver) = first_call.receiver(context) else {
        return Ok(None);
    };
    if mixin.line(dot.start) != mixin.line(receiver.range(context).start) {
        return Ok(None);
    }
    dot_through_selector_or_abort(context, first_call).map(Some)
}

/// `MultilineMethodCallIndentation#after_multiline_block_base`.
fn after_multiline_block_base<'tree>(
    mixin: &Mixin<'_, 'tree>,
    first_call: UpNode<'tree>,
    node: UpNode<'tree>,
) -> Result<Option<Range<usize>>, Aborted> {
    let context = mixin.context;
    let (Some(block), ..) = (first_call.block_node(), ()) else {
        return Ok(None);
    };
    if !block.multiline(context) {
        return Ok(None);
    }
    let Some(after_block) = block.parent() else {
        return Ok(None);
    };
    if !after_block.kind(context).call_type()
        || after_block.dot(context).is_none()
        || after_block.same(node)
    {
        return Ok(None);
    }
    dot_through_selector_or_abort(context, after_block).map(Some)
}

/// `MultilineMethodCallIndentation#inside_multiline_chain_arg?`.
fn inside_multiline_chain_arg<'tree>(
    mixin: &Mixin<'_, 'tree>,
    pair_ancestor: Option<UpNode<'tree>>,
) -> bool {
    let context = mixin.context;
    let Some(enclosing) = find_enclosing_chain_call(context, pair_ancestor) else {
        return false;
    };
    let (Some(selector), Some(receiver)) = (
        enclosing.selector(),
        enclosing.receiver(context).map(|r| r.range(context)),
    ) else {
        return false;
    };
    mixin.line(selector.start) != mixin.line(receiver.start)
}

/// `MultilineMethodCallIndentation#find_enclosing_chain_call`.
fn find_enclosing_chain_call<'tree>(
    context: &RuleContext<'_>,
    pair_ancestor: Option<UpNode<'tree>>,
) -> Option<UpNode<'tree>> {
    let hash_ancestor = pair_ancestor?.parent()?;
    let enclosing = hash_ancestor.parent()?;
    // `hash_arg_in_chain?`: the hash is an argument of a chained call rather than its receiver.
    if !enclosing.kind(context).call_type() || enclosing.dot(context).is_none() {
        return None;
    }
    if enclosing
        .receiver(context)
        .is_some_and(|receiver| receiver.same(hash_ancestor))
    {
        return None;
    }
    Some(enclosing)
}

/// `MultilineMethodCallIndentation#aligned_with_first_line_dot?`.
fn aligned_with_first_line_dot<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    rhs: &Range<usize>,
) -> bool {
    let context = mixin.context;
    if !starts_with_dot(context, rhs) {
        return false;
    }
    let Some(first_call) = first_call_has_a_dot(context, node) else {
        return false;
    };
    if node
        .receiver(context)
        .is_some_and(|receiver| first_call.same(receiver))
    {
        return false;
    }
    let Some(dot) = first_call.dot(context) else {
        return false;
    };
    mixin.line(dot.start) == node.line(context)
        && mixin.column(dot.start) == mixin.column(rhs.start)
}

/// `MultilineMethodCallIndentation#check_hash_pair_indented_style`.
fn check_hash_pair_indented_style<'tree>(
    mixin: &Mixin<'_, 'tree>,
    rhs: &Range<usize>,
    pair_ancestor: UpNode<'tree>,
) -> Option<Offending> {
    let key = pair_ancestor.node_field("key")?;
    let column = mixin.column(key.start);
    delta_offense(
        mixin,
        rhs,
        column + mixin.width * 2,
        None,
        Some(column + mixin.width),
    )
}

/// `MultilineMethodCallIndentation#check_regular_indentation`.
fn check_regular_indentation<'tree>(
    mixin: &Mixin<'_, 'tree>,
    style: Style,
    node: UpNode<'tree>,
    lhs: UpNode<'tree>,
    rhs: &Range<usize>,
) -> Result<Option<Offending>, Aborted> {
    let context = mixin.context;
    let base = alignment_base(mixin, style, node, rhs)?;
    let correct_column = match &base {
        Some(base) => {
            let parent =
                node.parent()
                    .and_then(|parent| match parent.kind(context) == UpKind::Block {
                        true => parent.parent(),
                        false => Some(parent),
                    });
            mixin.column(base.start) + extra_indentation(mixin, style, parent)
        }
        None => mixin.indentation(lhs) + mixin.correct_indentation(node),
    };
    Ok(delta_offense(mixin, rhs, correct_column, base, None))
}

/// `MultilineMethodCallIndentation#calculate_column_delta_offense`.
fn delta_offense(
    mixin: &Mixin<'_, '_>,
    rhs: &Range<usize>,
    correct_column: i64,
    base: Option<Range<usize>>,
    hash_pair_base_column: Option<i64>,
) -> Option<Offending> {
    let delta = correct_column - mixin.column(rhs.start);
    (delta != 0).then(|| Offending {
        rhs: rhs.clone(),
        delta,
        base,
        hash_pair_base_column,
    })
}

/// `MultilineMethodCallIndentation#extra_indentation`.
fn extra_indentation(mixin: &Mixin<'_, '_>, style: Style, parent: Option<UpNode<'_>>) -> i64 {
    if style != Style::IndentedRelativeToReceiver {
        return 0;
    }
    let context = mixin.context;
    match parent.map(|parent| (parent.kind(context), parent.range(context))) {
        Some((UpKind::Splat, range)) => mixin.width - splat_operator_length(context, &range),
        Some((UpKind::Kwsplat, range)) => mixin.width - splat_operator_length(context, &range),
        _ => mixin.width,
    }
}

fn splat_operator_length(context: &RuleContext<'_>, range: &Range<usize>) -> i64 {
    context.source.text()[range.clone()]
        .bytes()
        .take_while(|byte| *byte == b'*')
        .count() as i64
}

/// `MultilineMethodCallIndentation#alignment_base`.
fn alignment_base<'tree>(
    mixin: &Mixin<'_, 'tree>,
    style: Style,
    node: UpNode<'tree>,
    rhs: &Range<usize>,
) -> Result<Option<Range<usize>>, Aborted> {
    match style {
        Style::Aligned => Ok(semantic_alignment_base(mixin, node, rhs)
            .or_else(|| syntactic_alignment_base(mixin, node, rhs))),
        Style::IndentedRelativeToReceiver => receiver_alignment_base(mixin, node),
        Style::Indented => Ok(None),
    }
}

/// `MultilineMethodCallIndentation#syntactic_alignment_base`.
fn syntactic_alignment_base<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    rhs: &Range<usize>,
) -> Option<Range<usize>> {
    let context = mixin.context;
    // a if b
    //      .c
    if let Some(base) = mixin.keyword_ancestor(node) {
        return mixin.indented_keyword_expression(base);
    }
    // a = b
    //     .c
    if let Some(base) = mixin.part_of_assignment_rhs(node, Some(rhs)) {
        return mixin.assignment_rhs(base).map(|rhs| rhs.range(context));
    }
    // a + b
    //     .c
    operation_rhs(mixin, node).map(|rhs| rhs.range(context))
}

/// `MultilineMethodCallIndentation#operation_rhs`.
fn operation_rhs<'tree>(mixin: &Mixin<'_, 'tree>, node: UpNode<'tree>) -> Option<UpNode<'tree>> {
    let context = mixin.context;
    let receiver = node.receiver(context)?;
    let receiver_range = receiver.range(context);
    receiver
        .ancestors()
        .filter(|ancestor| ancestor.kind(context) == UpKind::Send)
        .find_map(|ancestor| {
            if !ancestor.operator_method(context) {
                return None;
            }
            let first = ancestor.first_argument(context)?;
            within(&receiver_range, &first.range(context)).then_some(first)
        })
}

/// `MultilineMethodCallIndentation#semantic_alignment_base`.
fn semantic_alignment_base<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    rhs: &Range<usize>,
) -> Option<Range<usize>> {
    let context = mixin.context;
    if !starts_with_dot(context, rhs) {
        return None;
    }
    let node = semantic_alignment_node(mixin, node)?;
    dot_through_selector(context, node)
}

/// `MultilineMethodCallIndentation#semantic_alignment_node`.
fn semantic_alignment_node<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    if mixin.argument_in_method_call(node, true).is_some() {
        return None;
    }
    get_dot_right_above(mixin, node)
        .or_else(|| find_multiline_block_chain_node(mixin, node))
        .or_else(|| first_call_alignment_node(mixin, node))
}

/// `MultilineMethodCallIndentation#get_dot_right_above`.
fn get_dot_right_above<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    let context = mixin.context;
    let dot = node.dot(context)?;
    node.ancestors().find(|ancestor| {
        ancestor.dot(context).is_some_and(|above| {
            mixin.line(above.start) + 1 == mixin.line(dot.start)
                && mixin.column(above.start) == mixin.column(dot.start)
        })
    })
}

/// `MultilineMethodCallIndentation#find_multiline_block_chain_node`.
fn find_multiline_block_chain_node<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    match node.block_node() {
        Some(_) => find_continuation_node(mixin, node),
        None => handle_descendant_block(mixin, node),
    }
}

/// `MultilineMethodCallIndentation#find_continuation_node`.
fn find_continuation_node<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    let context = mixin.context;
    let receiver = node.receiver(context)?;
    if single_line_block_receiver(context, receiver) {
        return leftmost_call_on_same_line(mixin, receiver);
    }
    if !receiver.kind(context).call_type() || receiver.dot(context).is_none() {
        return None;
    }
    let inner = receiver.receiver(context)?;
    if inner.kind(context) == UpKind::Begin
        && node
            .block_node()
            .is_some_and(|block| block.single_line(context))
    {
        return Some(receiver);
    }
    let dot = receiver.dot(context)?;
    (mixin.line(dot.start) > inner.last_line(context)).then_some(receiver)
}

/// `MultilineMethodCallIndentation#single_line_block_receiver?`.
fn single_line_block_receiver(context: &RuleContext<'_>, receiver: UpNode<'_>) -> bool {
    receiver.single_line(context) && receiver.kind(context) == UpKind::Block
}

/// `MultilineMethodCallIndentation#leftmost_call_on_same_line`.
fn leftmost_call_on_same_line<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    let context = mixin.context;
    let mut current = node.send_node();
    loop {
        let Some(dot) = current.dot(context) else {
            return Some(current);
        };
        let Some(receiver) = current.receiver(context) else {
            return Some(current);
        };
        if !receiver.kind(context).call_type() {
            return Some(current);
        }
        let Some(above) = receiver.dot(context) else {
            return Some(current);
        };
        if mixin.line(above.start) != mixin.line(dot.start) {
            return Some(current);
        }
        current = receiver;
    }
}

/// `MultilineMethodCallIndentation#handle_descendant_block`.
fn handle_descendant_block<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    let context = mixin.context;
    let receiver = node.receiver(context)?;
    if single_line_block_receiver(context, receiver) {
        return leftmost_call_on_same_line(mixin, receiver);
    }
    let block = node.first_descendant_block()?;
    if !block.multiline(context) {
        return None;
    }
    match mixin.call_type(receiver) {
        true => Some(receiver),
        false => block.parent(),
    }
}

/// `MultilineMethodCallIndentation#first_call_alignment_node`.
fn first_call_alignment_node<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Option<UpNode<'tree>> {
    let context = mixin.context;
    let node = first_call_has_a_dot(context, node)?;
    let base_receiver = find_base_receiver(context, node);
    if method_on_receiver_last_line(mixin, node, base_receiver, UpKind::Array) {
        return Some(node);
    }
    let dot = node.dot(context)?;
    if mixin.line(dot.start) != node.line(context) {
        return None;
    }
    if method_on_receiver_last_line(mixin, node, base_receiver, UpKind::Begin) {
        return None;
    }
    Some(node)
}

/// `MultilineMethodCallIndentation#method_on_receiver_last_line?`.
fn method_on_receiver_last_line<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    base_receiver: UpNode<'tree>,
    kind: UpKind,
) -> bool {
    let context = mixin.context;
    if base_receiver.same(node) {
        return false;
    }
    let Some(dot) = node.dot(context) else {
        return false;
    };
    mixin.line(dot.start) == base_receiver.last_line(context) && base_receiver.kind(context) == kind
}

/// `MultilineMethodCallIndentation#receiver_alignment_base`.
fn receiver_alignment_base<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Result<Option<Range<usize>>, Aborted> {
    let context = mixin.context;
    if let Some(base) = find_hash_method_base_in_receiver_chain(mixin, node)? {
        return Ok(Some(base));
    }
    let Some(first_call) = first_call_has_a_dot(context, node) else {
        return Ok(None);
    };
    Ok(first_call
        .receiver(context)
        .map(|receiver| receiver.range(context)))
}

/// `MultilineMethodCallIndentation#find_hash_method_base_in_receiver_chain`.
fn find_hash_method_base_in_receiver_chain<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
) -> Result<Option<Range<usize>>, Aborted> {
    let context = mixin.context;
    let mut chain = node.receiver(context).map(UpNode::send_node);
    while let Some(current) = chain.filter(|current| current.kind(context).call_type()) {
        let base_receiver = current.receiver(context).map(UpNode::send_node);
        let hash_base = base_receiver.is_some_and(|base| base.kind(context) == UpKind::Hash)
            || base_receiver.is_some_and(|base| {
                method_on_receiver_last_line(mixin, current, base, UpKind::Begin)
            });
        if hash_base {
            return dot_through_selector_or_abort(context, current).map(Some);
        }
        chain = base_receiver;
    }
    Ok(None)
}

fn starts_with_dot(context: &RuleContext<'_>, range: &Range<usize>) -> bool {
    let text = context.source.slice(range.clone());
    text.starts_with('.') || text.starts_with("&.")
}

/// `MultilineMethodCallIndentation#message`.
fn message<'tree>(
    mixin: &Mixin<'_, 'tree>,
    style: Style,
    node: UpNode<'tree>,
    lhs: UpNode<'tree>,
    offending: &Offending,
) -> String {
    let context = mixin.context;
    let Some(base) = &offending.base else {
        return no_base_message(mixin, node, lhs, offending);
    };
    let source = context.source.slice(base.clone());
    let first_line = source.split('\n').next().unwrap_or(source);
    let line = mixin.line(base.start);
    match style {
        Style::IndentedRelativeToReceiver => format!(
            "Indent `{}` {} spaces more than `{first_line}` on line {line}.",
            context.source.slice(offending.rhs.clone()),
            mixin.width
        ),
        Style::Aligned => format!(
            "Align `{}` with `{first_line}` on line {line}.",
            context.source.slice(offending.rhs.clone())
        ),
        Style::Indented => no_base_message(mixin, node, lhs, offending),
    }
}

/// `MultilineMethodCallIndentation#no_base_message`.
fn no_base_message<'tree>(
    mixin: &Mixin<'_, 'tree>,
    node: UpNode<'tree>,
    lhs: UpNode<'tree>,
    offending: &Offending,
) -> String {
    let column = mixin.column(offending.rhs.start);
    let (used, expected) = match offending.hash_pair_base_column {
        Some(base) => (column - base, mixin.width),
        None => (
            column - mixin.indentation(lhs),
            mixin.correct_indentation(node),
        ),
    };
    let what = mixin.operation_description(node, &offending.rhs);
    format!("Use {expected} (not {used}) spaces for indenting {what} spanning multiple lines.")
}
