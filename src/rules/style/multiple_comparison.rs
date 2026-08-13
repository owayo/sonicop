use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let threshold = context
        .setting::<usize>("ComparisonsThreshold")
        .unwrap_or(2);
    let allow_method_comparison = context.setting("AllowMethodComparison").unwrap_or(true);
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("binary") {
        if !is_or(node, context) {
            continue;
        }
        // `root_of_or_node`: only the outermost `||` of a chain reports.
        if node.parent().is_some_and(|parent| is_or(parent, context)) {
            continue;
        }
        if !is_nested_comparison(node, context, &locals, allow_method_comparison) {
            continue;
        }
        let mut found = Search {
            variables: Vec::new(),
            values: Vec::new(),
            skipped: Vec::new(),
        };
        found.walk(node, context, &locals, allow_method_comparison);
        let (Some(variable), false) = (found.variables.first().copied(), found.values.is_empty())
        else {
            continue;
        };
        if found.values.len() < threshold {
            continue;
        }
        let (Some(first), Some(last)) = (
            found.values.first().and_then(Node::parent),
            found.values.last().and_then(Node::parent),
        ) else {
            continue;
        };
        let range = first.start_byte()..last.end_byte();
        if found
            .skipped
            .iter()
            .any(|node| range.start <= node.start_byte() && node.end_byte() <= range.end)
        {
            continue;
        }
        let elements: Vec<&str> = found
            .values
            .iter()
            .map(|value| context.source.node_text(*value))
            .collect();
        let replacement = format!(
            "[{}].include?({})",
            elements.join(", "),
            context.source.node_text(variable)
        );
        offenses.push(
            context
                .offense(
                    "Avoid comparing a variable with multiple items in a conditional, use \
                     `Array#include?` instead.",
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `find_offending_var`'s accumulators.
struct Search<'tree> {
    variables: Vec<Node<'tree>>,
    values: Vec<Node<'tree>>,
    skipped: Vec<Node<'tree>>,
}

impl<'tree> Search<'tree> {
    fn walk(
        &mut self,
        node: Node<'tree>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
        allow_method_comparison: bool,
    ) {
        if is_or(node, context) {
            let (Some(left), Some(right)) = (
                node.field("left"),
                node.field("right"),
            ) else {
                return;
            };
            self.walk(left, context, locals, allow_method_comparison);
            self.walk(right, context, locals, allow_method_comparison);
            return;
        }
        let Some((variable, value)) = comparison(node, context, locals, allow_method_comparison)
        else {
            return;
        };
        // `simple_double_comparison?`: `x == y` names two variables and no value at all.
        if is_lvar(variable, locals) && is_lvar(value, locals) {
            return;
        }
        // The value being a method call makes the rewrite change what runs, so the comparison is
        // set aside rather than collected.
        if allow_method_comparison && is_call(value, context, locals) {
            self.skipped.push(node);
            return;
        }
        if !self
            .variables
            .iter()
            .any(|seen| context.source.node_text(*seen) == context.source.node_text(variable))
        {
            self.variables.push(variable);
        }
        if self.variables.len() > 1 {
            return;
        }
        self.values.push(value);
    }
}

/// `nested_comparison?`: both halves of the `||` are comparisons, or nested `||`s that are.
fn is_nested_comparison(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    allow_method_comparison: bool,
) -> bool {
    let (Some(left), Some(right)) = (
        node.field("left"),
        node.field("right"),
    ) else {
        return false;
    };
    [left, right].into_iter().all(|part| {
        comparison(part, context, locals, allow_method_comparison).is_some()
            || (is_or(part, context)
                && is_nested_comparison(part, context, locals, allow_method_comparison))
    })
}

/// `simple_comparison`: the variable and the value of an `==`, either way round.
fn comparison<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    allow_method_comparison: bool,
) -> Option<(Node<'tree>, Node<'tree>)> {
    if node.kind_str() != "binary"
        || context
            .source
            .node_text(node.field("operator")?)
            != "=="
    {
        return None;
    }
    let (left, right) = (
        node.field("left")?,
        node.field("right")?,
    );
    let (variable, value) = match is_variable(left, context, locals) {
        true => (left, right),
        false if is_variable(right, context, locals) => (right, left),
        false => return None,
    };
    if is_call(variable, context, locals) && !allow_method_comparison {
        return None;
    }
    Some((variable, value))
}

/// `{lvar call}`: what may stand where the pattern names the variable being compared.
fn is_variable(node: Node<'_>, context: &RuleContext<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    is_lvar(node, locals) || is_call(node, context, locals)
}

fn is_lvar(node: Node<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    node.kind_str() == "identifier" && locals.is_lvar(node)
}

/// `call_type?`: the shapes the grammar writes what upstream's parser calls a `send` or `csend` in.
fn is_call(node: Node<'_>, context: &RuleContext<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    match node.kind_str() {
        "call" | "element_reference" => true,
        "identifier" => !locals.is_lvar(node),
        // `-1` is folded into one `int` by upstream's parser, so only a sign applied to something
        // other than an adjacent numeric literal is a call.
        "unary" => !is_signed_literal(node, context),
        "binary" => node
            .field("operator")
            .is_some_and(|operator| {
                super::nodes::is_operator_method(context.source.node_text(operator))
            }),
        _ => false,
    }
}

/// Whether the node is a numeric literal upstream's parser folded a leading sign into.
fn is_signed_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (Some(operator), Some(operand)) = (
        node.field("operator"),
        node.field("operand"),
    ) else {
        return false;
    };
    matches!(context.source.node_text(operator), "-" | "+")
        && operator.end_byte() == operand.start_byte()
        && matches!(
            operand.kind_str(),
            "integer" | "float" | "rational" | "complex"
        )
}

/// Whether the node is what upstream's parser builds an `or` for.
fn is_or(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "||" | "or"))
}
