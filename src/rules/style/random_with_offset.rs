use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::is_plain_send;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Prefer ranges when generating random numbers instead of integers with offsets.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["binary", "call"]) {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        let Some(replacement) = corrected(context, node) else {
            continue;
        };
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement,
            safe: true,
        }));
    }
}

/// The range the offset and the `rand` call stand for, for each of the three shapes the cop
/// matches.
fn corrected(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    match node.kind_str() {
        "binary" => {
            let operator = context
                .source
                .node_text(node.field("operator")?);
            if !matches!(operator, "+" | "-") {
                return None;
            }
            let (left, right) = (
                node.field("left")?,
                node.field("right")?,
            );
            // `integer_op_rand?`, then `rand_op_integer?`.
            if let Some(random) = random_call(context, right)
                && let Some(offset) = integer_value(context, left)
            {
                let (low, high) = random.boundaries;
                return Some(match operator {
                    "+" => random.range(offset + low, offset + high),
                    _ => random.range(offset - high, offset - low),
                });
            }
            let random = random_call(context, left)?;
            let offset = integer_value(context, right)?;
            let (low, high) = random.boundaries;
            Some(match operator {
                "+" => random.range(low + offset, high + offset),
                _ => random.range(low - offset, high - offset),
            })
        }
        // `rand_modified?`.
        "call" => {
            let method = context
                .source
                .node_text(node.field("method")?);
            if !matches!(method, "succ" | "pred" | "next")
                || node.field("arguments").is_some()
            {
                return None;
            }
            let random = random_call(context, node.field("receiver")?)?;
            let (low, high) = random.boundaries;
            let step = match method {
                "pred" => -1,
                _ => 1,
            };
            Some(random.range(low + step, high + step))
        }
        _ => None,
    }
}

/// One `rand` call the cop can rewrite: what to write in front of the range, and the bounds the
/// argument stands for.
struct Random {
    prefix: String,
    boundaries: (i64, i64),
}

impl Random {
    fn range(&self, low: i64, high: i64) -> String {
        format!("{}({low}..{high})", self.prefix)
    }
}

/// `(send {nil? (const {nil? cbase} :Random) (const {nil? cbase} :Kernel)} :rand {int (range int
/// int)})`.
fn random_call(context: &RuleContext<'_>, node: Node<'_>) -> Option<Random> {
    if node.kind_str() != "call"
        || context
            .source
            .node_text(node.field("method")?)
            != "rand"
    {
        return None;
    }
    let prefix = match node.field("receiver") {
        None => "rand".to_owned(),
        Some(receiver) => {
            let source = context.source.node_text(receiver);
            if !matches!(source.trim_start_matches("::"), "Random" | "Kernel")
                || !matches!(receiver.kind_str(), "constant" | "scope_resolution")
            {
                return None;
            }
            format!("{source}.rand")
        }
    };
    let arguments = super::nodes::children_in(node.field("arguments")?, context);
    let [argument] = arguments.as_slice() else {
        return None;
    };
    let boundaries = match argument.kind_str() {
        "range" => {
            let low = integer_value(context, argument.field("begin")?)?;
            let high = integer_value(context, argument.field("end")?)?;
            let inclusive = context
                .source
                .node_text(argument.field("operator")?)
                == "..";
            (low, high - i64::from(!inclusive))
        }
        _ => (0, integer_value(context, *argument)? - 1),
    };
    Some(Random { prefix, boundaries })
}

/// `(int $_)`: the parser folds a leading sign into the literal, so `-1` is one too.
fn integer_value(context: &RuleContext<'_>, node: Node<'_>) -> Option<i64> {
    let (node, negative) = match node.kind_str() {
        "unary" => {
            let operator = context
                .source
                .node_text(node.field("operator")?);
            if !matches!(operator, "-" | "+") {
                return None;
            }
            (node.field("operand")?, operator == "-")
        }
        _ => (node, false),
    };
    if node.kind_str() != "integer" {
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
    Some(match negative {
        true => -value,
        false => value,
    })
}
