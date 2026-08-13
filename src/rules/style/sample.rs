use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::send_node;

/// How many elements the indexing after `shuffle` asks for.
enum SampleSize {
    /// Upstream's `:unknown`: the index says something `sample` has no argument for.
    Unknown,
    /// Upstream's `nil`: one element, which `sample` gives without an argument.
    Whole,
    Count(i64),
}

/// One argument as upstream's parser groups them: a trailing run of `key: value` pairs is a single
/// `hash` there, however many pairs were written.
struct Argument<'tree> {
    first: Node<'tree>,
    range: Range<usize>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some((receiver, method, method_arguments)) = indexing(context, node) else {
            continue;
        };
        let Some((selector, shuffle_argument)) = shuffle_call(context, &locals, receiver) else {
            continue;
        };
        // `offensive?`, and the argument `sample` takes in place of the index, which upstream reads
        // off the same answer.
        let sample_argument = match method {
            "first" | "last" => method_arguments
                .first()
                .map(|argument| context.source.slice(argument.range.clone()).to_owned()),
            "[]" | "slice" => match sample_size(context, &method_arguments) {
                SampleSize::Unknown => continue,
                SampleSize::Whole => None,
                SampleSize::Count(count) => Some(count.to_string()),
            },
            // `sample_arg` has no branch for `at`, so an offensive one always corrects to a bare
            // `sample`.
            "at" => match sample_size(context, &method_arguments) {
                SampleSize::Unknown => continue,
                _ => None,
            },
            _ => continue,
        };
        let arguments: Vec<String> = sample_argument
            .into_iter()
            .chain(shuffle_argument)
            .collect();
        let correction = match arguments.is_empty() {
            true => "sample".to_owned(),
            false => format!("sample({})", arguments.join(", ")),
        };
        // `shuffle_node.loc.selector.join(node.source_range.end)`: a block written after the call is
        // no part of the `send` upstream reports.
        let range = selector.start..send_node::send_range(node, context).end;
        let message = format!(
            "Use `{correction}` instead of `{}`.",
            context.source.slice(range.clone())
        );
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: correction,
            safe: true,
        }));
    }
}

/// `(call $_ ${:first :last :[] :at :slice} $...)`: the receiver, the selector's name and the
/// arguments of a call that takes one element out of a collection.
fn indexing<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str, Vec<Argument<'tree>>)> {
    if node.kind() == "element_reference" {
        let object = node.child_by_field_name("object")?;
        let mut children = super::nodes::children(node);
        // `emit_index` is off, so `a[0]` is a call to `:[]` whose first child is the receiver.
        children.remove(0);
        return Some((object, "[]", group(&children)));
    }
    let receiver = node.child_by_field_name("receiver")?;
    let selector = node.child_by_field_name("method")?;
    let method = match context.source.node_text(selector) {
        "first" => "first",
        "last" => "last",
        "[]" => "[]",
        "at" => "at",
        "slice" => "slice",
        _ => return None,
    };
    let written = node
        .child_by_field_name("arguments")
        .map(|list| super::nodes::children(list))
        .unwrap_or_default();
    Some((receiver, method, group(&written)))
}

/// `(call _ :shuffle $...)`: where the `shuffle` selector starts, and the source of its first
/// argument.
fn shuffle_call(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    node: Node<'_>,
) -> Option<(Range<usize>, Option<String>)> {
    // A receiverless `shuffle` is a bare identifier here and a `send` upstream, but only where the
    // name is not a local variable in scope.
    if node.kind() == "identifier" {
        return (context.source.node_text(node) == "shuffle" && !locals.is_lvar(node))
            .then(|| (node.byte_range(), None));
    }
    if node.kind() != "call" || node.child_by_field_name("block").is_some() {
        return None;
    }
    let selector = node.child_by_field_name("method")?;
    if context.source.node_text(selector) != "shuffle" {
        return None;
    }
    let written = node
        .child_by_field_name("arguments")
        .map(|list| super::nodes::children(list))
        .unwrap_or_default();
    let first = group(&written)
        .first()
        .map(|argument| context.source.slice(argument.range.clone()).to_owned());
    Some((selector.byte_range(), first))
}

/// The written argument nodes as upstream's parser groups them.
fn group<'tree>(written: &[Node<'tree>]) -> Vec<Argument<'tree>> {
    let mut arguments: Vec<Argument<'tree>> = Vec::new();
    let mut hash: Vec<Node<'tree>> = Vec::new();
    for node in written {
        if matches!(node.kind(), "pair" | "hash_splat_argument") {
            hash.push(*node);
            continue;
        }
        if let Some(pair) = hash.first() {
            let range = pair.start_byte()..hash[hash.len() - 1].end_byte();
            arguments.push(Argument {
                first: *pair,
                range,
            });
            hash.clear();
        }
        arguments.push(Argument {
            first: *node,
            range: node.byte_range(),
        });
    }
    if let Some(pair) = hash.first() {
        let range = pair.start_byte()..hash[hash.len() - 1].end_byte();
        arguments.push(Argument {
            first: *pair,
            range,
        });
    }
    arguments
}

fn sample_size(context: &RuleContext<'_>, arguments: &[Argument<'_>]) -> SampleSize {
    match arguments {
        [only] => match only.first.kind() {
            "range" => range_size(context, only.first),
            _ => match integer_value(context, only.first) {
                // Only the first element and the last one are what `sample` gives on its own.
                Some(0 | -1) => SampleSize::Whole,
                _ => SampleSize::Unknown,
            },
        },
        [low, count] => match integer_value(context, low.first) {
            Some(0) => match integer_value(context, count.first) {
                Some(value) => SampleSize::Count(value),
                None => SampleSize::Unknown,
            },
            _ => SampleSize::Unknown,
        },
        // `sample_size`'s `case` has no branch for any other count, so it answers `nil`.
        _ => SampleSize::Whole,
    }
}

/// `range_size`: how many elements a literal range starting at zero covers.
fn range_size(context: &RuleContext<'_>, node: Node<'_>) -> SampleSize {
    let bound = |field: &str| match node.child_by_field_name(field) {
        // A beginless or endless side is `nil` upstream, which `range_size` reads as zero.
        None => Some(0),
        Some(child) => integer_value(context, child),
    };
    let (Some(low), Some(high)) = (bound("begin"), bound("end")) else {
        return SampleSize::Unknown;
    };
    if low != 0 || high < 0 {
        return SampleSize::Unknown;
    }
    let inclusive = node
        .child_by_field_name("operator")
        .is_some_and(|operator| context.source.node_text(operator) == "..");
    SampleSize::Count(high + i64::from(inclusive))
}

/// `(int _)`: the parser folds a leading sign into the literal, so `-1` is one too.
fn integer_value(context: &RuleContext<'_>, node: Node<'_>) -> Option<i64> {
    let (node, negative) = match node.kind() {
        "unary" => {
            let operator = context
                .source
                .node_text(node.child_by_field_name("operator")?);
            if !matches!(operator, "-" | "+") {
                return None;
            }
            (node.child_by_field_name("operand")?, operator == "-")
        }
        _ => (node, false),
    };
    if node.kind() != "integer" {
        return None;
    }
    let text: String = context
        .source
        .node_text(node)
        .chars()
        .filter(|character| *character != '_')
        .collect();
    let (radix, digits) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (16, &text[2..]),
        Some("0b") => (2, &text[2..]),
        Some("0o") => (8, &text[2..]),
        Some("0d") => (10, &text[2..]),
        _ if text.len() > 1 && text.starts_with('0') => (8, &text[1..]),
        _ => (10, &text[..]),
    };
    let value = i64::from_str_radix(digits, radix).ok()?;
    Some(if negative { -value } else { value })
}
