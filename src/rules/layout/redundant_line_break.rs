use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{is_plain_send, named_children, send_range};
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

/// `each_descendant(:if, :case, :kwbegin, :any_def, :rescue, :ensure)`: what a line the correction
/// joins can never hold, since none of these constructs survives losing its line breaks.
const UNSAFE_KINDS: &[&str] = &[
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "case",
    "begin",
    "method",
    "singleton_method",
    "rescue",
    "ensure",
];

/// `:sym`, whose spellings are the ones a multiline symbol can be written with.
const SYMBOL_KINDS: &[&str] = &[
    "simple_symbol",
    "delimited_symbol",
    "hash_key_symbol",
    "bare_symbol",
];

/// The kinds that hold a list of statements, which upstream's parser writes as its `begin` node once
/// more than one was written.
const SEQUENCES: &[&str] = &["block_body", "body_statement"];

/// The numeric literals a sign belongs to. Upstream's parser folds the sign into the literal, so a
/// signed number is no call.
const NUMBERS: &[&str] = &["integer", "float", "rational", "complex"];

/// `\s` as Ruby reads it, which is the ASCII run and not the Unicode one this engine defaults to.
const SPACE: &str = r"[ \t\r\n\f\x0B]";

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
        let mut descendants = Vec::new();
        let mut stack = named_children(self.node);
        while let Some(node) = stack.pop() {
            if Some(node.id()) == skip {
                continue;
            }
            descendants.push(node);
            stack.extend(named_children(node));
        }
        descendants
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
    if !is_suitable_as_single_line(expression, &range, maximum, context) {
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

/// `suitable_as_single_line?`.
fn is_suitable_as_single_line(
    expression: &Expression<'_>,
    range: &Range<usize>,
    maximum: Option<usize>,
    context: &RuleContext<'_>,
) -> bool {
    !is_too_long(range, maximum, context)
        && !has_comment_within(range, context)
        && is_safe_to_split(expression, context)
}

/// `too_long?`, which measures the whole lines the expression sits on rather than the expression.
fn is_too_long(range: &Range<usize>, maximum: Option<usize>, context: &RuleContext<'_>) -> bool {
    let Some(maximum) = maximum else {
        return false;
    };
    let lines: Vec<&str> = (line(range.start, context)..=line(last_byte(range), context))
        .map(|number| context.source.line_without_terminator(number))
        .collect();
    to_single_line(&lines.join("\n")).chars().count() > maximum
}

/// `comment_within?`: a comment written on any of the lines the expression spans.
fn has_comment_within(range: &Range<usize>, context: &RuleContext<'_>) -> bool {
    let (first, last) = (line(range.start, context), line(last_byte(range), context));
    context.comment_ranges().iter().any(|comment| {
        let number = line(comment.start, context);
        number >= first && number <= last
    })
}

/// `safe_to_split?`.
fn is_safe_to_split(expression: &Expression<'_>, context: &RuleContext<'_>) -> bool {
    expression.descendants().iter().all(|node| {
        !is_unsafe_kind(*node)
            && !is_unjoinable_string(*node, context)
            && !is_multiline_sequence_or_symbol(*node, context)
    })
}

/// The constructs `each_descendant(:if, :case, :kwbegin, :any_def, :rescue, :ensure)` looks for.
/// `case ... in` is a `case_match` upstream and no `case`, which the grammar spells with a kind of
/// its own, so the list stands on the kinds alone.
fn is_unsafe_kind(node: Node<'_>) -> bool {
    UNSAFE_KINDS.contains(&node.kind_str())
}

/// `each_descendant(:dstr, :str).none? { |n| n.heredoc? || n.value.include?("\n") }`.
fn is_unjoinable_string(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "heredoc_beginning" {
        return true;
    }
    if node.kind_str() != "string" {
        return false;
    }
    // A line break the literal holds survives joining the lines, so the value is not the one it
    // started with. `'\n'` spells a backslash and an `n`, which is why only an escape the grammar
    // recorded counts.
    context.source.node_text(node).contains('\n')
        || named_children(node).iter().any(|child| {
            child.kind_str() == "escape_sequence" && is_newline_escape(*child, context)
        })
}

fn is_newline_escape(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    matches!(
        context.source.node_text(node),
        r"\n" | r"\012" | r"\x0a" | r"\x0A" | r"\u000a" | r"\u000A" | r"\u{a}" | r"\u{A}"
    )
}

/// `each_descendant(:begin, :sym).none?(&:multiline?)`.
fn is_multiline_sequence_or_symbol(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let counts = match node.kind_str() {
        // `defined?(x)` records its parentheses on the `defined?` node itself upstream rather than
        // wrapping the expression in a `begin`, which is the one place a pair of them is no node.
        "parenthesized_statements" => !is_defined_parentheses(node, context),
        kind if SYMBOL_KINDS.contains(&kind) => true,
        // A list of statements is upstream's `begin` only once more than one was written; a single
        // statement is itself.
        kind if SEQUENCES.contains(&kind) => {
            named_children(node)
                .iter()
                .filter(|child| child.kind_str() != "comment")
                .count()
                > 1
        }
        _ => false,
    };
    counts && line(node.start_byte(), context) != line(last_byte(&node.byte_range()), context)
}

/// Whether the parentheses are the ones a `defined?` was written with, which hold one expression and
/// belong to the keyword.
fn is_defined_parentheses(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind_str() == "unary"
            && parent
                .field("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "defined?")
    }) && named_children(node)
        .iter()
        .filter(|child| child.kind_str() != "comment")
        .count()
        == 1
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
    let call = block.parent_of(context)?;
    let outer = call.parent_of(context)?;
    let chained = outer.kind_str() == "call"
        && outer
            .field("receiver")
            .is_some_and(|receiver| receiver.id() == call.id())
        && outer.field("operator").is_some();
    chained.then_some(outer)
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

/// `max_line_length`, absent when the length cop is off and no join can be too long.
fn max_line_length(context: &RuleContext<'_>) -> Option<usize> {
    if !context.cop_enabled("Layout/LineLength") {
        return None;
    }
    Some(
        context
            .setting_of::<usize>("Layout/LineLength", "Max")
            .unwrap_or(120),
    )
}

/// A double quote, a backslash and then a single quote, which loses the line break for a `+`.
static MIXED_DOUBLE_SINGLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"" *\\\n{SPACE}*'"#)).expect("the mixed quote pattern compiles")
});

/// A single quote, a backslash and then a double quote.
static MIXED_SINGLE_DOUBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"' *\\\n{SPACE}*""#)).expect("the mixed quote pattern compiles")
});

/// The same quote on both sides of the break, where the two literals become one.
static SAME_DOUBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"" *\\\n{SPACE}*""#)).expect("the same quote pattern compiles")
});

static SAME_SINGLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"' *\\\n{SPACE}*'")).expect("the same quote pattern compiles")
});

/// The break in front of a chained call, whose dot has to stay against the receiver. Upstream looks
/// ahead rather than matching the dot, which this engine cannot do, so the dot is put back.
static BEFORE_CHAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"\n{SPACE}*(&?\.\w)")).expect("the chained call pattern compiles")
});

/// Any other line break, with or without a backslash.
static ANY_BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"{SPACE}*\\?\n{SPACE}*")).expect("the line break pattern compiles")
});

/// `to_single_line`.
fn to_single_line(source: &str) -> String {
    let source = MIXED_DOUBLE_SINGLE.replace_all(source, r#"" + '"#);
    let source = MIXED_SINGLE_DOUBLE.replace_all(&source, r#"' + ""#);
    let source = SAME_DOUBLE.replace_all(&source, "");
    let source = SAME_SINGLE.replace_all(&source, "");
    let source = BEFORE_CHAIN.replace_all(&source, "$1");
    ANY_BREAK.replace_all(&source, " ").into_owned()
}

/// The last byte a range covers, which is the one `node.last_line` is read from.
fn last_byte(range: &Range<usize>) -> usize {
    range.end.saturating_sub(1).max(range.start)
}

fn line(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}
