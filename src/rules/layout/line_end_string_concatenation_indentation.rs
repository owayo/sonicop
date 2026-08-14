use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

const MSG_ALIGN: &str = "Align parts of a string concatenated with backslash.";
const MSG_INDENT: &str = "Indent the first part of a string concatenated with backslash.";

/// `PARENT_TYPES_FOR_INDENTED`: the places a concatenation is indented under whatever the style says,
/// because there is nothing on the line above to align to.
const PARENT_TYPES_FOR_INDENTED: &[&str] = &["begin", "block", "def", "defs", "if"];

/// The node kinds the grammar adds for statement lists, which upstream's parser has no node for
/// other than its `begin`.
const CONTAINERS: &[&str] = &[
    "program",
    "body_statement",
    "then",
    "else",
    "do",
    "block_body",
    "parenthesized_statements",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width: usize = context
        .setting("IndentationWidth")
        .or_else(|| context.setting_of("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let indented = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "aligned".to_owned())
        == "indented";
    for node in context.nodes_of("chained_string") {
        let children = named_children(node);
        // `strings_concatenated_with_backslash?`: the whole literal runs over more than one line
        // while no single part of it does.
        if line(node.start_byte(), context) == line(node.end_byte(), context) {
            continue;
        }
        if children.iter().any(|child| {
            child.kind_str() != "string"
                || line(child.start_byte(), context) != line(child.end_byte(), context)
        }) {
            continue;
        }
        if children.is_empty() {
            continue;
        }
        match indented || is_always_indented(node, context) {
            false => offenses.extend(check_aligned(&children, 1, context)),
            true => {
                offenses.extend(check_indented(node, &children, width, context));
                offenses.extend(check_aligned(&children, 2, context));
            }
        }
    }
}

/// `check_aligned`: each part opens where the one before it did.
fn check_aligned(children: &[Node<'_>], start: usize, context: &RuleContext<'_>) -> Vec<Offense> {
    let Some(first) = children.get(start.saturating_sub(1)) else {
        return Vec::new();
    };
    let mut base = column(*first, context);
    let mut offenses = Vec::new();
    for child in children.iter().skip(start) {
        let delta = base as i64 - column(*child, context) as i64;
        if delta != 0 {
            offenses.push(offense(*child, delta, MSG_ALIGN, context));
        }
        base = column(*child, context);
    }
    offenses
}

/// `check_indented`: the second part sits one indentation width in from what the first part's line
/// opens with.
fn check_indented(
    node: Node<'_>,
    children: &[Node<'_>],
    width: usize,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    let second = children.get(1)?;
    let delta = base_column(node, *children.first()?, context) as i64 + width as i64
        - column(*second, context) as i64;
    (delta != 0).then(|| offense(*second, delta, MSG_INDENT, context))
}

/// `base_column`: the column a hash pair opens at, or the first non-blank of the first part's line.
fn base_column(node: Node<'_>, first: Node<'_>, context: &RuleContext<'_>) -> usize {
    if node
        .parent()
        .is_some_and(|parent| parent.kind_str() == "pair")
    {
        return column(node.parent().expect("just checked"), context);
    }
    let text = context.source.line(line(first.start_byte(), context));
    text.chars()
        .take_while(|character| character.is_whitespace())
        .count()
}

/// `always_indented?`.
fn is_always_indented(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match upstream_parent(node, context) {
        // A parent of `nil` is in the list: a lone statement has nothing above it to align to.
        None => true,
        Some(kind) => PARENT_TYPES_FOR_INDENTED.contains(&kind),
    }
}

/// The type upstream's parser gives the node's parent, or `None` when the node is the whole file.
fn upstream_parent(node: Node<'_>, context: &RuleContext<'_>) -> Option<&'static str> {
    let mut current = node;
    loop {
        let parent = current.parent_of(context)?;
        if CONTAINERS.contains(&parent.kind_str()) {
            // A list of more than one statement is a `begin`; one statement is that statement, so
            // the parent to read is the container's own.
            if parent.kind_str() == "parenthesized_statements" || statements(parent) > 1 {
                return Some("begin");
            }
            current = parent;
            continue;
        }
        return Some(match parent.kind_str() {
            // The `do ... end` and `{ ... }` the grammar writes as nodes of their own are the
            // `block` upstream wraps around the call. Reaching the call itself instead means the
            // literal was written on its receiver or argument side, where the parent is the `send`.
            "do_block" | "block" | "lambda" => "block",
            "call" | "argument_list" => "send",
            "method" => "def",
            "singleton_method" => "defs",
            "unless" | "elsif" | "conditional" | "if_modifier" | "unless_modifier" => "if",
            other => other,
        });
    }
}

fn statements(container: Node<'_>) -> usize {
    named_children(container)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .count()
}

/// The offense, whose correction shifts the part's line by `delta` columns.
fn offense(
    child: Node<'_>,
    delta: i64,
    message: &'static str,
    context: &RuleContext<'_>,
) -> Offense {
    let offense = context.offense(message, child.byte_range());
    match shift(child, delta, context) {
        Some(edit) => offense.corrected_by(edit),
        None => offense,
    }
}

/// `AlignmentCorrector.correct` for a part written on one line: the blanks in front of it grow or
/// shrink by `delta`.
fn shift(child: Node<'_>, delta: i64, context: &RuleContext<'_>) -> Option<Edit> {
    let start = child.start_byte();
    let text = context.source.text();
    if delta > 0 {
        // `insert_before(range, ' ' * column_delta) unless range.resize(1).source == "\n"`.
        if text[start..].starts_with('\n') {
            return None;
        }
        return Some(Edit {
            start,
            end: start,
            replacement: " ".repeat(usize::try_from(delta).ok()?),
            safe: true,
        });
    }
    let width = usize::try_from(-delta).ok()?;
    // `calculate_range`: the blanks removed are the ones the part opens with when it opens with one,
    // and otherwise the ones written in front of it.
    let range = match text[start..].starts_with(' ') {
        true => start..start + width,
        false => start.checked_sub(width)?..start,
    };
    let removed = text.get(range.clone())?;
    (!removed.is_empty() && removed.bytes().all(|byte| matches!(byte, b' ' | b'\t'))).then(|| {
        Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }
    })
}

/// `node.loc.column`, which is zero-based.
fn column(node: Node<'_>, context: &RuleContext<'_>) -> usize {
    context.source.line_column(node.start_byte()).1 - 1
}

fn line(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}
