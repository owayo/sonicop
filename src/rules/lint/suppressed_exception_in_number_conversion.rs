use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, pair_key_symbol, top_level_constant};

use super::statements::statements;
use crate::rules::send_node::named_children_of;

/// `EXPECTED_EXCEPTION_CLASSES`: the two the conversion itself raises.
const EXPECTED_EXCEPTION_CLASSES: [&str; 4] = [
    "ArgumentError",
    "TypeError",
    "::ArgumentError",
    "::TypeError",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < RubyVersion::new(2, 6) {
        return;
    }
    // `Integer(x) rescue nil`: the whole modifier expression is what gets replaced.
    for node in context.nodes_of("rescue_modifier") {
        let (Some(body), Some(handler)) = (node.field("body"), node.field("handler")) else {
            continue;
        };
        if handler.kind_str() != "nil" || !is_numeric_method(body, context) {
            continue;
        }
        report(context, offenses, node, body);
    }
    // `begin Integer(x) rescue [classes]; [nil] end`, which the parser keeps as a `kwbegin`.
    for node in context.nodes_of("begin") {
        let children = named_children_of(node, context);
        let [body, clause] = children.as_slice() else {
            continue;
        };
        if clause.kind_str() != "rescue" || !is_numeric_method(*body, context) {
            continue;
        }
        if clause.field("variable").is_some() || !expected_exception_classes_only(*clause, context)
        {
            continue;
        }
        // `{(nil) nil?}`: the clause either answers `nil` or does nothing at all.
        let handled = match clause.field("body") {
            None => true,
            Some(body) => match statements(body).as_slice() {
                [] => true,
                [only] => only.kind_str() == "nil",
                _ => false,
            },
        };
        if handled {
            report(context, offenses, node, *body);
        }
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    method: Node<'_>,
) {
    if has_exception_keyword_argument(method, context) {
        return;
    }
    let mut parts: Vec<String> = arguments(method)
        .iter()
        .map(|argument| context.source.slice(argument.range()).to_owned())
        .collect();
    parts.push("exception: false".to_owned());
    let name = method.field("method").map_or_else(String::new, |name| {
        context.source.node_text(name).to_owned()
    });
    let prefix = match (method.field("receiver"), method.field("operator")) {
        (Some(receiver), Some(operator)) => format!(
            "{}{}",
            context.source.node_text(receiver),
            context.source.node_text(operator)
        ),
        _ => String::new(),
    };
    let prefer = format!("{prefix}{name}({})", parts.join(", "));
    let range = node.byte_range();
    offenses.push(
        context
            .offense(format!("Use `{prefer}` instead."), range.clone())
            .corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: prefer,
                safe: true,
            }),
    );
}

/// `numeric_method?`: one of the `Kernel` conversion functions, with the argument count each takes.
fn is_numeric_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    let Some(name) = node.field("method") else {
        return false;
    };
    // `#constructor_receiver?`: written bare, or reached through `Kernel`.
    if node
        .field("receiver")
        .is_some_and(|receiver| !top_level_constant(receiver, "Kernel", context))
    {
        return false;
    }
    let count = arguments(node).len();
    match context.source.node_text(name) {
        "Integer" | "BigDecimal" | "Complex" | "Rational" => (1..=2).contains(&count),
        "Float" => count == 1,
        _ => false,
    }
}

fn has_exception_keyword_argument(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    arguments(node).iter().any(|argument| {
        argument
            .parts()
            .iter()
            .any(|part| pair_key_symbol(*part, context) == Some("exception"))
    })
}

/// `expected_exception_classes_only?`: a clause that catches anything wider is doing more than the
/// keyword argument would.
fn expected_exception_classes_only(clause: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(exceptions) = clause.field("exceptions") else {
        return true;
    };
    named_children_of(exceptions, context)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .all(|exception| EXPECTED_EXCEPTION_CLASSES.contains(&context.source.node_text(exception)))
}
