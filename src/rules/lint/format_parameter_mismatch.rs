use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send};

use super::format_string::{is_valid, parse};
use super::literals::literal_type;
use crate::rules::node_ext::NodeExt;

const MSG_INVALID: &str = "Format string is invalid because formatting sequence types \
     (numbered, named or unnumbered) are mixed.";

/// What `count_matches` answers when the shape is not one it can count.
enum Count {
    Unknown,
    Known(isize, String, usize),
}

/// One argument, as the node it begins with and the span it covers.
struct Arg<'tree> {
    first: Node<'tree>,
    range: Range<usize>,
}

/// One call, in the terms `SendNode` presents it.
struct Call<'tree> {
    method: String,
    selector: Range<usize>,
    receiver: Option<Node<'tree>>,
    given: Vec<Arg<'tree>>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["call", "binary"]) {
        let Some(call) = read_call(node, context) else {
            continue;
        };
        if !is_format_string(&call, context) {
            continue;
        }
        if !valid_format_string(&call, context) {
            offenses.push(context.offense(MSG_INVALID, call.selector.clone()));
            continue;
        }
        if splat_arguments(&call, context) {
            continue;
        }
        let Count::Known(passed, shown, expected) = count_matches(&call, context) else {
            continue;
        };
        let first = call.given[0].first;
        if expected == 0 && matches!(literal_type(first, context), Some("dstr" | "array")) {
            continue;
        }
        let mismatched = if passed < 0 {
            (expected as isize) < passed.abs()
        } else {
            expected as isize != passed
        };
        if !mismatched {
            continue;
        }
        let name = if call.method == "%" {
            "String#%".to_owned()
        } else {
            call.method.clone()
        };
        offenses.push(context.offense(
            format!(
                "Number of arguments ({passed}) to `{name}` doesn't match the number of fields \
 ({shown})."
            ),
            call.selector.clone(),
        ));
    }
}

/// `RESTRICT_ON_SEND = %i[format sprintf %]`, in the two shapes tree-sitter writes them.
fn read_call<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Call<'tree>> {
    match node.kind_str() {
        "call" => {
            if !is_plain_send(node, context) {
                return None;
            }
            let method = node.field("method")?;
            let name = context.source.node_text(method).to_owned();
            if !matches!(name.as_str(), "format" | "sprintf" | "%") {
                return None;
            }
            Some(Call {
                method: name,
                selector: method.byte_range(),
                receiver: node.field("receiver"),
                given: arguments(node)
                    .iter()
                    .map(|argument| Arg {
                        first: argument.first(),
                        range: argument.range(),
                    })
                    .collect(),
            })
        }
        "binary" => {
            let operator = node.field("operator")?;
            if context.source.node_text(operator) != "%" {
                return None;
            }
            Some(Call {
                method: "%".to_owned(),
                selector: operator.byte_range(),
                receiver: node.field("left"),
                given: {
                    let right = node.field("right")?;
                    vec![Arg {
                        first: right,
                        range: right.byte_range(),
                    }]
                },
            })
        }
        _ => None,
    }
}

/// `format_string?`.
fn is_format_string(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    called_on_string(call, context) && method_with_format_args(call, context)
}

/// `called_on_string?`: `{(send {nil? const_type?} _ {str dstr} ...) (send {str dstr} ...)}`.
fn called_on_string(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    let receiver_is_string = call
        .receiver
        .is_some_and(|receiver| is_string(receiver, context));
    if receiver_is_string {
        return true;
    }
    let receiver_allows = call
        .receiver
        .is_none_or(|receiver| matches!(receiver.kind_str(), "constant" | "scope_resolution"));
    receiver_allows
        && call
            .given
            .first()
            .is_some_and(|first| is_string(first.first, context))
}

fn is_string(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    matches!(literal_type(node, context), Some("str" | "dstr"))
}

fn method_with_format_args(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    is_format_method(call, context) || is_percent(call, context)
}

/// `format_method?`: `format`/`sprintf`, either bare or on `Kernel`, with a literal first argument.
fn is_format_method(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    if !matches!(call.method.as_str(), "format" | "sprintf") {
        return false;
    }
    if let Some(receiver) = call.receiver
        && matches!(receiver.kind_str(), "constant" | "scope_resolution")
        && context.source.node_text(receiver) != "Kernel"
    {
        return false;
    }
    call.given.len() > 1 && is_string(call.given[0].first, context)
}

/// `percent?`.
fn is_percent(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    if call.method != "%" {
        return false;
    }
    let receiver_is_string = call
        .receiver
        .is_some_and(|receiver| is_string(receiver, context));
    let first_is_array = call
        .given
        .first()
        .is_some_and(|first| first.first.kind_str() == "array");
    if !receiver_is_string && !first_is_array {
        return false;
    }
    !(receiver_is_string && is_heredoc(call, context))
}

/// `heredoc?`: the first argument is written with `<<`.
fn is_heredoc(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    call.given
        .first()
        .is_some_and(|first| context.source.slice(first.range.clone()).starts_with("<<"))
}

/// `invalid_format_string?`, negated.
fn valid_format_string(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    let source = if is_format_method(call, context) {
        context.source.slice(call.given[0].range.clone())
    } else {
        match call.receiver {
            Some(receiver) => context.source.node_text(receiver),
            None => return true,
        }
    };
    is_valid(&parse(source))
}

/// `count_matches`.
fn count_matches(call: &Call<'_>, context: &RuleContext<'_>) -> Count {
    if is_format_method(call, context) && !is_heredoc(call, context) {
        let passed = isize::try_from(call.given.len()).unwrap_or(isize::MAX) - 1;
        let (shown, expected) = expected_fields(context.source.slice(call.given[0].range.clone()));
        return Count::Known(passed, shown, expected);
    }
    if is_percent(call, context)
        && call
            .given
            .first()
            .is_some_and(|first| first.first.kind_str() == "array")
    {
        let list = call.given[0].first;
        let mut cursor = list.walk();
        let passed = list
            .named_children(&mut cursor)
            .filter(|child| child.kind_str() != "comment")
            .count();
        let Some(receiver) = call.receiver else {
            return Count::Unknown;
        };
        if !is_string(receiver, context) {
            return Count::Unknown;
        }
        let (shown, expected) = expected_fields(context.source.node_text(receiver));
        return Count::Known(isize::try_from(passed).unwrap_or(isize::MAX), shown, expected);
    }
    Count::Unknown
}

/// `expected_fields_count`.
fn expected_fields(source: &str) -> (String, usize) {
    let sequences = parse(source);
    if sequences.iter().any(|sequence| sequence.name.is_some()) {
        return ("1".to_owned(), 1);
    }
    // A `N$` selector is a Ruby integer, which has no width; only its digits are known here.
    if let Some(highest) = sequences
        .iter()
        .filter_map(super::format_string::Sequence::max_digit_dollar_num)
        .max_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)))
        && highest != "0"
    {
        let value = highest.parse().unwrap_or(usize::MAX);
        return (highest, value);
    }
    let total: usize = sequences
        .iter()
        .filter(|sequence| !sequence.is_percent)
        .map(super::format_string::Sequence::arity)
        .sum();
    (total.to_string(), total)
}

/// `splat_args?`.
fn splat_arguments(call: &Call<'_>, context: &RuleContext<'_>) -> bool {
    if is_percent(call, context) {
        return false;
    }
    call.given
        .iter()
        .skip(1)
        .any(|argument| argument.first.kind_str() == "splat_argument")
}
