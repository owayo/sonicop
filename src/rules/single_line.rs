//! `CheckSingleLineSuitability`: whether a piece of code written over several lines could be written
//! on one instead, and the rewrite that would do it.
//!
//! Two cops ask the question from opposite directions. `Layout/RedundantLineBreak` reports an
//! expression that *could* be joined, and `Style/SingleLineDoEndBlock` reports a `do ... end` block
//! written on one line unless joining blocks is what the configuration asks for. Both read the same
//! answer, so both read it from here.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

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

/// `\s` as Ruby reads it, which is the ASCII run and not the Unicode one this engine defaults to.
const SPACE: &str = r"[ \t\r\n\f\x0B]";

/// `suitable_as_single_line?`: whether the span could be written on one line without changing what
/// it means or running past `Layout/LineLength`.
pub(in crate::rules) fn is_suitable_as_single_line(
    range: &Range<usize>,
    descendants: &[Node<'_>],
    maximum: Option<usize>,
    context: &RuleContext<'_>,
) -> bool {
    !is_too_long(range, maximum, context)
        && !has_comment_within(range, context)
        && is_safe_to_split(descendants, context)
}

/// `too_long?`, which measures the whole lines the expression sits on rather than the expression.
fn is_too_long(range: &Range<usize>, maximum: Option<usize>, context: &RuleContext<'_>) -> bool {
    let Some(maximum) = maximum else {
        return false;
    };
    let lines: Vec<&str> = (line(range.start, context)..=line(last_byte(range), context))
        .map(|number| source_line(number, context))
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
fn is_safe_to_split(descendants: &[Node<'_>], context: &RuleContext<'_>) -> bool {
    descendants.iter().all(|node| {
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

/// `max_line_length`, absent when the length cop is off and no join can be too long.
pub(in crate::rules) fn max_line_length(context: &RuleContext<'_>) -> Option<usize> {
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
pub(in crate::rules) fn to_single_line(source: &str) -> String {
    let source = MIXED_DOUBLE_SINGLE.replace_all(source, r#"" + '"#);
    let source = MIXED_SINGLE_DOUBLE.replace_all(&source, r#"' + ""#);
    let source = SAME_DOUBLE.replace_all(&source, "");
    let source = SAME_SINGLE.replace_all(&source, "");
    let source = BEFORE_CHAIN.replace_all(&source, "$1");
    ANY_BREAK.replace_all(&source, " ").into_owned()
}

/// `processed_source.lines[n]`, which is the line without the break that ends it. The source keeps
/// that break, and a line measured or matched with it on is one character longer than upstream's.
pub(in crate::rules) fn source_line<'a>(number: usize, context: &'a RuleContext<'_>) -> &'a str {
    context
        .source
        .line(number)
        .trim_end_matches('\n')
        .trim_end_matches('\r')
}

/// The nodes `each_descendant` reaches from `node`, with the subtree of `skip` left out.
///
/// A cop reporting a call whose block upstream keeps in a node of its own has to leave that block
/// out, since upstream never walks into it from the call.
pub(in crate::rules) fn descendants<'tree>(
    node: Node<'tree>,
    skip: Option<usize>,
) -> Vec<Node<'tree>> {
    let mut descendants = Vec::new();
    let mut stack = named_children(node);
    while let Some(node) = stack.pop() {
        if Some(node.id()) == skip {
            continue;
        }
        descendants.push(node);
        stack.extend(named_children(node));
    }
    descendants
}

/// The last byte a range covers, which is the one `node.last_line` is read from.
pub(in crate::rules) fn last_byte(range: &Range<usize>) -> usize {
    range.end.saturating_sub(1).max(range.start)
}

fn line(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}
