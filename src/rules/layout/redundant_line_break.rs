use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{is_plain_send, named_children, send_range};
use crate::rules::single_line::{
    descendants, is_suitable_as_single_line, last_byte, max_line_length, to_single_line,
};
use crate::rules::support::{Verification, verified_by_reparse};

const MSG: &str = "Redundant line break detected.";

/// The nodes upstream's `on_send` and the assignment handlers of `CheckAssignment` fire for, in the
/// order a walk over the file reaches them. An `and` or an `or` has no handler of its own and is
/// only ever reached by ascending out of a call written inside it, which is why the `binary` kind
/// standing for both needs a gate rather than a place of its own here.
const HANDLED: &[&str] = &[
    "call",
    "binary",
    "unary",
    "element_reference",
    "assignment",
    "operator_assignment",
];

/// The numeric literals a sign belongs to. Upstream's parser folds the sign into the literal, so a
/// signed number is no call.
const NUMBERS: &[&str] = &["integer", "float", "rational", "complex"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let inspect_blocks = context.setting("InspectBlocks").unwrap_or(false);
    let chain_cop = context.cop_enabled("Layout/SingleLineBlockChain");
    let maximum = max_line_length(context);
    // `end_with_percent_blank_string?`, which only `on_lvasgn` asks about.
    let percent_blank = context.source.text().ends_with("%\n\n");
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(HANDLED) {
        if is_assignment_handler(node) {
            if percent_blank && is_local_assignment(node) {
                continue;
            }
            // `check_assignment` never asks whether the node sits inside one already reported.
            let expression = Expression::of(node, context);
            if is_offense(&expression, maximum, inspect_blocks, chain_cop, context) {
                register(&expression, context, offenses, &mut ignored);
            }
            continue;
        }
        if is_logical(node, context) {
            // An `and` reached from a call written inside it is what upstream inspects, so one
            // holding no call at all is never looked at, and one that is not the top of an ascent is
            // reached through the call anyway.
            if !is_top(node, context) || !spine_has_send(node, context) {
                continue;
            }
        } else if !is_upstream_send(node, context) {
            continue;
        }
        let top = ascend(node, context);
        if !is_offense(&top, maximum, inspect_blocks, chain_cop, context) {
            continue;
        }
        if is_part_of_ignored(&top, &ignored, context) {
            continue;
        }
        register(&top, context, offenses, &mut ignored);
    }
}

/// One expression as upstream's tree holds it: a node, and whether the block written on it belongs
/// to the expression.
///
/// `foo(a) { b }` is one `block` node upstream wrapped around the call, and the grammar writes the
/// block inside the call instead. The two agree on the text they span -- but `foo a do b end` does
/// not, because a block written on a call with unparenthesized arguments is not one the ascent
/// reaches, so the expression there is the call alone.
#[derive(Clone, Copy)]
struct Expression<'tree> {
    node: Node<'tree>,
    with_block: bool,
}

impl<'tree> Expression<'tree> {
    fn of(node: Node<'tree>, context: &RuleContext<'_>) -> Self {
        Self {
            node,
            with_block: is_convertible_block(node, context),
        }
    }

    fn range(&self, context: &RuleContext<'_>) -> Range<usize> {
        match self.node.field("block") {
            Some(_) if !self.with_block => send_range(self.node, context),
            _ => self.node.byte_range(),
        }
    }

    /// `each_descendant`, with the block subtree left out when the block is no part of the
    /// expression.
    fn descendants(&self) -> Vec<Node<'tree>> {
        let skip = match self.with_block {
            true => None,
            false => self.node.field("block").map(|block| block.id()),
        };
        descendants(self.node, skip)
    }
}

/// `offense?`.
fn is_offense(
    expression: &Expression<'_>,
    maximum: Option<usize>,
    inspect_blocks: bool,
    chain_cop: bool,
    context: &RuleContext<'_>,
) -> bool {
    let range = expression.range(context);
    if line(range.start, context) == line(last_byte(&range), context) {
        return false;
    }
    if !is_suitable_as_single_line(&range, &expression.descendants(), maximum, context) {
        return false;
    }
    // `node.operator_keyword?`: `and` and `or` are joinable only where a backslash is what broke the
    // line, since the operator itself already lets the expression run on.
    if is_logical(expression.node, context) {
        return requires_backslash(expression.node, context);
    }
    !is_index_access_call_chained(expression.node)
        && !is_configured_to_not_be_inspected(expression, inspect_blocks, chain_cop, context)
}

/// `require_backslash?`: the line the operator sits on ends with a backslash.
fn requires_backslash(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(operator) = node.field("operator") else {
        return false;
    };
    context
        .source
        .line_without_terminator(line(operator.start_byte(), context))
        .ends_with('\\')
}

/// `index_access_call_chained?`: `a[1][2]`, whose line break the index cop owns.
fn is_index_access_call_chained(node: Node<'_>) -> bool {
    node.kind_str() == "element_reference"
        && node
            .field("object")
            .is_some_and(|object| object.kind_str() == "element_reference")
}

/// `configured_to_not_be_inspected?`.
fn is_configured_to_not_be_inspected(
    expression: &Expression<'_>,
    inspect_blocks: bool,
    chain_cop: bool,
    context: &RuleContext<'_>,
) -> bool {
    if other_cop_takes_precedence(expression, chain_cop, context) {
        return true;
    }
    if inspect_blocks {
        return false;
    }
    expression.with_block
        || expression
            .descendants()
            .iter()
            .any(|node| is_block(*node) && is_block_multiline(*node, context))
}

/// `other_cop_takes_precedence?`: a single-line block with a call chained onto it belongs to
/// `Layout/SingleLineBlockChain` while that cop is on.
fn other_cop_takes_precedence(
    expression: &Expression<'_>,
    chain_cop: bool,
    context: &RuleContext<'_>,
) -> bool {
    chain_cop
        && expression.descendants().iter().any(|node| {
            is_block(*node)
                && !is_block_multiline(*node, context)
                && chained_onto(*node, context).is_some()
        })
}

/// `block_node.parent` where that parent is a call reached through a dot. The block belongs to the
/// call it was written on here, so the call upstream calls its parent is that call's own.
fn chained_onto<'tree>(block: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    // The block belongs to the call it was written on here, so the node upstream calls its parent is
    // that call's own -- whether the block stands there as a receiver or as an argument.
    let call = block.parent_of(context)?;
    let outer = upstream_parent(call, context)?;
    (outer.kind_str() == "call" && outer.field("operator").is_some()).then_some(outer)
}

fn is_block(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "block" | "do_block")
}

/// `BlockNode#multiline?`, which reads the block's own delimiters rather than the span of everything
/// written inside it.
fn is_block_multiline(block: Node<'_>, context: &RuleContext<'_>) -> bool {
    line(block.start_byte(), context) != line(last_byte(&block.byte_range()), context)
}

/// The offense, whose correction is the expression written on one line -- reported only once that
/// exact rewrite is verified to parse to the tree it started from.
fn register(
    expression: &Expression<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    ignored: &mut Vec<Range<usize>>,
) {
    let range = expression.range(context);
    let edit = Edit {
        start: range.start,
        end: range.end,
        replacement: to_single_line(context.source.slice(range.clone()))
            .trim()
            .to_owned(),
        safe: true,
    };
    // `verified_by_reparse([node], oversized: :verify)`: joining lines merges split string literals,
    // which changes the tree without changing the string, so the comparison folds every
    // concatenation to one shape first.
    let verification = Verification {
        verify_oversized: true,
        fold_string_concatenation: true,
        ..Default::default()
    };
    let verified = verified_by_reparse(
        context,
        vec![()],
        |_| vec![edit.clone()],
        |_| range.clone(),
        verification,
    );
    if verified.is_empty() {
        return;
    }
    offenses.push(context.offense(MSG, range.clone()).corrected_by(edit));
    ignored.push(range);
}

/// `part_of_ignored_node?`.
fn is_part_of_ignored(
    expression: &Expression<'_>,
    ignored: &[Range<usize>],
    context: &RuleContext<'_>,
) -> bool {
    let range = expression.range(context);
    ignored
        .iter()
        .any(|seen| seen.start <= range.start && seen.end >= range.end)
}

/// `node = node.parent while node.parent&.send_type? || convertible_block?(node) ||
/// node.parent.is_a?(BinaryOperatorNode)`: the whole expression a call was written in.
fn ascend<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Expression<'tree> {
    let mut current = Expression::of(node, context);
    loop {
        // A block the ascent cannot reach past ends it: upstream's parent there is that block, which
        // is no call.
        if current.node.field("block").is_some() && !current.with_block {
            return current;
        }
        let Some(parent) = upstream_parent(current.node, context) else {
            return current;
        };
        let continues = (is_upstream_send(parent, context) && is_plain_send(parent, context))
            || is_logical(parent, context);
        if !continues {
            return current;
        }
        current = Expression::of(parent, context);
    }
}

/// Whether the ascent out of the node reaches the node itself, which is what makes it the one
/// upstream inspects.
fn is_top(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    ascend(node, context).node.id() == node.id()
}

/// Whether a call is written somewhere along the operator chain, which is the only way an `and` or
/// an `or` is ever inspected.
fn spine_has_send(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if is_logical(node, context) {
        return named_children(node)
            .into_iter()
            .any(|child| spine_has_send(child, context));
    }
    if is_upstream_send(node, context) {
        return true;
    }
    // A bare name that is no local variable is a call on `self`, which the grammar writes as a plain
    // identifier.
    node.kind_str() == "identifier" && !context.variable_roles().names_a_local(node)
}

/// `convertible_block?`: whether the block written on the call is one the ascent reaches, which asks
/// for the call to be parenthesized or to take no arguments at all.
fn is_convertible_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.field("block").is_none() {
        return false;
    }
    match node.field("arguments") {
        None => true,
        Some(list) => context.source.node_text(list).starts_with('('),
    }
}

/// Whether the node is one of the `send` nodes upstream's parser builds, which is every call however
/// it was written -- an operator, an index read, an attribute write.
fn is_upstream_send(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        // `super(...)` is a node of its own upstream and no call, which the grammar writes as a call
        // whose method name is the keyword.
        "call" => node
            .field("method")
            .is_none_or(|method| method.kind_str() != "super"),
        "element_reference" => true,
        "binary" => !is_logical(node, context),
        // `defined?` is a node of its own upstream, and a sign belongs to the number it was written
        // on rather than to a call.
        "unary" => !is_keyword_unary(node, context) && !is_signed_number(node, context),
        // `foo.bar = 1` and `foo[1] = 2` are `send` nodes upstream; every other assignment is not.
        "assignment" => node
            .field("left")
            .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference")),
        _ => false,
    }
}

fn is_keyword_unary(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("operator")
        .is_some_and(|operator| context.source.node_text(operator) == "defined?")
}

fn is_signed_number(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("operator")
        .is_some_and(|operator| matches!(context.source.node_text(operator), "-" | "+"))
        && node
            .field("operand")
            .is_some_and(|operand| NUMBERS.contains(&operand.kind_str()))
}

/// `operator_keyword?`: the `and` and `or` nodes, whichever of the two spellings they were written
/// with.
fn is_logical(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node.field("operator").is_some_and(|operator| {
            matches!(
                context.source.node_text(operator),
                "&&" | "||" | "and" | "or"
            )
        })
}

/// The assignments `CheckAssignment` hands to `check_assignment`, which is every one whose left-hand
/// side is not a call.
fn is_assignment_handler(node: Node<'_>) -> bool {
    match node.kind_str() {
        "operator_assignment" => true,
        "assignment" => node
            .field("left")
            .is_some_and(|left| !matches!(left.kind_str(), "call" | "element_reference")),
        _ => false,
    }
}

/// `on_lvasgn`, the one handler the cop guards with `end_with_percent_blank_string?`.
fn is_local_assignment(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| left.kind_str() == "identifier")
}

/// The parent upstream's tree gives the node, with the argument list the grammar adds skipped: an
/// argument's parent is the call itself there.
fn upstream_parent<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context)?;
    while current.kind_str() == "argument_list" {
        current = current.parent_of(context)?;
    }
    Some(current)
}

fn line(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}
