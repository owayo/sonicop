use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::literals::{is_constant, literal_type, recursive_basic_literal};
use crate::rules::send_node::has_interpolation;
use crate::rules::node_ext::NodeExt;

/// `RESTRICT_ON_SEND = COMPARISON_OPERATORS`, minus the one `yoda_compatible_condition?` drops:
/// `===` is not commutative, so reversing it would not mean the same thing.
const COMPARISON_OPERATORS: &[&str] = &["==", "!=", "<=", ">=", ">", "<"];

/// `EQUALITY_OPERATORS`, which the two `*_for_equality_operators_only` styles restrict to.
const EQUALITY_OPERATORS: &[&str] = &["==", "!="];

/// `PROGRAM_NAMES`: the two spellings of the global holding the program's name.
const PROGRAM_NAMES: &[&str] = &["$0", "$PROGRAM_NAME"];

/// `REVERSE_COMPARISON`: the operator that means the same thing with the operands swapped.
fn reverse_comparison(operator: &str) -> &str {
    match operator {
        "<" => ">",
        "<=" => ">=",
        ">" => "<",
        ">=" => "<=",
        other => other,
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "forbid_for_all_comparison_operators".to_owned());
    // `ENFORCE_YODA_STYLES` / `EQUALITY_ONLY_STYLES`.
    let enforce_yoda = style.starts_with("require_for_");
    let equality_only = style.ends_with("_for_equality_operators_only");

    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((lhs, operator, rhs)) = operands(node, context) else {
            continue;
        };
        let name = context.source.node_text(operator);
        if !COMPARISON_OPERATORS.contains(&name) {
            continue;
        }
        if (equality_only && !EQUALITY_OPERATORS.contains(&name))
            || file_constant_equal_program_name(context, lhs, name, rhs)
            || valid_yoda(context, lhs, rhs, enforce_yoda)
        {
            continue;
        }
        let range = node.byte_range();
        let source = context.source.slice(range.clone());
        // `corrected_code`: the two operands swapped, joined by the mirrored operator.
        let replacement = format!(
            "{} {} {}",
            context.source.node_text(rhs),
            reverse_comparison(name),
            context.source.node_text(lhs)
        );
        offenses.push(
            context
                .offense(
                    format!("Reverse the order of the operands `{source}`."),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    // `Safe: false`, so `-a` leaves this alone.
                    safe: false,
                }),
        );
    }
}

/// The receiver, the selector and the single argument of a comparison, however it was written.
///
/// `on_send` never sees a safe navigation call -- those reach `on_csend`, which this cop does not
/// define -- and a comparison with no argument at all cannot be reversed.
fn operands<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    if node.kind_str() == "binary" {
        return Some((
            node.field("left")?,
            node.field("operator")?,
            node.field("right")?,
        ));
    }
    if node.field("block").is_some() {
        return None;
    }
    let receiver = node.field("receiver")?;
    let dot = node.field("operator")?;
    if context.source.node_text(dot) != "." {
        return None;
    }
    let selector = node.field("method")?;
    // `node.first_argument`: a further argument cannot reach a comparison operator, but the
    // grammar would let one through.
    let arguments = super::nodes::children(node.field("arguments")?);
    let [only] = arguments.as_slice() else {
        return None;
    };
    Some((receiver, selector, *only))
}

/// `valid_yoda?`: whether the operands are already the way round the style asks for.
///
/// Two operands the parser knows the value of, or two it knows neither of, say nothing about which
/// order was meant; nor does an interpolated left-hand side, which reads as the subject of the
/// comparison however it parses.
fn valid_yoda(context: &RuleContext<'_>, lhs: Node<'_>, rhs: Node<'_>, enforce_yoda: bool) -> bool {
    let constant_lhs = constant_portion(context, lhs);
    let constant_rhs = constant_portion(context, rhs);
    if constant_lhs == constant_rhs || is_interpolation(context, lhs) {
        return true;
    }
    if enforce_yoda {
        constant_lhs
    } else {
        constant_rhs
    }
}

/// `constant_portion?`: `node.recursive_literal? || node.const_type?`.
fn constant_portion(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    recursive_basic_literal(node, context) || is_constant(node, context)
}

/// `interpolation?`: a `dstr`, or a regexp that interpolates.
fn is_interpolation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if literal_type(node, context) == Some("dstr") {
        return true;
    }
    node.kind_str() == "regex" && has_interpolation(node)
}

/// `file_constant_equal_program_name?`: `__FILE__ == $0` is how a script tells that it was run
/// rather than required, and reversing it would only obscure that.
fn file_constant_equal_program_name(
    context: &RuleContext<'_>,
    lhs: Node<'_>,
    operator: &str,
    rhs: Node<'_>,
) -> bool {
    EQUALITY_OPERATORS.contains(&operator)
        && context.source.node_text(lhs) == "__FILE__"
        && rhs.kind_str() == "global_variable"
        && PROGRAM_NAMES.contains(&context.source.node_text(rhs))
}
