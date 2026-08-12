use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Avoid the use of the case equality operator `===`.";

/// Node kinds whose source has to be parenthesized before a method call can be hung off it.
const NEEDS_PARENTHESES: &[&str] = &[
    "and",
    "or",
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "range",
    "assignment",
    "operator_assignment",
    "binary",
    // `a[b]` is a call to the operator method `:[]`, which upstream also wraps.
    "element_reference",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_on_constant: bool = context.setting("AllowOnConstant").unwrap_or(false);
    let allow_on_self_class: bool = context.setting("AllowOnSelfClass").unwrap_or(false);

    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((selector, left, right)) = case_equality(context, node) else {
            continue;
        };
        // `offending_receiver?`: the configuration can spare a constant or `self.class`.
        if (allow_on_constant && is_constant(left))
            || (allow_on_self_class && is_self_class(context, left))
        {
            continue;
        }
        // A regexp is left alone, and so is a constant whose name reads as a value rather than as a
        // class: `FOO === x` is not a type test.
        if left.kind() == "regex" || (is_constant(left) && !module_name(context, left)) {
            continue;
        }

        let offense = context.offense(MSG, selector.byte_range());
        offenses.push(match replacement(context, node, left, right) {
            Some(replacement) => offense.corrected_by(Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement,
                safe: true,
            }),
            None => offense,
        });
    }
}

/// `(send $_ :=== $_)`: the operator written either way round.
fn case_equality<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    match node.kind() {
        "binary" => {
            let operator = node.child_by_field_name("operator")?;
            (context.source.node_text(operator) == "===").then_some((
                operator,
                node.child_by_field_name("left")?,
                node.child_by_field_name("right")?,
            ))
        }
        _ => {
            let method = node.child_by_field_name("method")?;
            if context.source.node_text(method) != "===" {
                return None;
            }
            let arguments = node
                .child_by_field_name("arguments")
                .map(super::nodes::children)
                .unwrap_or_default();
            match arguments.as_slice() {
                [only] => Some((method, node.child_by_field_name("receiver")?, *only)),
                _ => None,
            }
        }
    }
}

/// `replacement`: only a range, a class name or `self.class` names something the call can test.
fn replacement(
    context: &RuleContext<'_>,
    node: Node<'_>,
    left: Node<'_>,
    right: Node<'_>,
) -> Option<String> {
    // A `call` spelling reaches upstream as a `send` whose receiver is the left-hand side, so the
    // rewrite is the same; only the node it replaces differs.
    let _ = node;
    let source = context.source.node_text(left);
    if left.kind() == "parenthesized_statements" {
        let mut cursor = left.walk();
        let inner = left.named_children(&mut cursor).next()?;
        if inner.kind() != "range" {
            return None;
        }
        return Some(format!(
            "{source}.include?({})",
            context.source.node_text(right)
        ));
    }
    if is_constant(left) {
        return Some(format!("{}.is_a?({source})", parenthesized(context, right)));
    }
    is_self_class(context, left)
        .then(|| format!("{}.is_a?({source})", parenthesized(context, right)))
}

/// `parenthesize_if_needed`: `Array === a + b` has to become `(a + b).is_a?(Array)`.
fn parenthesized(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let source = context.source.node_text(node);
    match NEEDS_PARENTHESES.contains(&node.kind()) || is_operator_call(context, node) {
        true => format!("({source})"),
        false => source.to_owned(),
    }
}

/// `node.send_type? && (node.operator_method? || node.unary_operation?)`, for the shapes the
/// grammar does not spell as a binary expression. `defined?` is a node type of its own upstream, so
/// it is not one of them.
fn is_operator_call(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "unary" => node
            .child_by_field_name("operator")
            .is_none_or(|operator| context.source.node_text(operator) != "defined?"),
        "call" => node
            .child_by_field_name("method")
            .is_some_and(|method| method.kind() == "operator"),
        _ => false,
    }
}

fn is_constant(node: Node<'_>) -> bool {
    matches!(node.kind(), "constant" | "scope_resolution")
}

/// `module_name?`: the last part of the name holds a lower-case letter, which a screaming constant
/// does not.
fn module_name(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let text = match node.kind() {
        "scope_resolution" => node
            .child_by_field_name("name")
            .map_or("", |name| context.source.node_text(name)),
        _ => context.source.node_text(node),
    };
    text.chars().any(|character| character.is_lowercase())
}

fn is_self_class(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "call"
        && node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| receiver.kind() == "self")
        && node
            .child_by_field_name("method")
            .is_some_and(|method| context.source.node_text(method) == "class")
}
