//! `Style/TallyMethod`: counting occurrences by hand where `tally` already does it.

use tree_sitter::Node;

use super::select_by::body_statements;
use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, symbol_name};

const MSG_EACH_WITH_OBJECT: &str = "Use `tally` instead of `each_with_object`.";
const MSG_GROUP_BY: &str = "Use `tally` instead of `group_by` and `transform_values`.";

/// `COUNTING_METHODS`.
const COUNTING_METHODS: &[&str] = &["count", "size", "length"];

/// `minimum_target_ruby_version 2.7`.
const MINIMUM_VERSION: RubyVersion = RubyVersion::new(2, 7);

/// The version that made `it` a block parameter rather than a receiverless call.
///
/// **The gate belongs on the parser's reading, not on the cop's opinion.** Below it, upstream's
/// parser gives `array.group_by { it }` a plain `block` whose body is `(send nil :it)`, so the
/// `itblock` patterns match nothing and the cop stays quiet whatever it thinks of the code.
const IT_VERSION: RubyVersion = RubyVersion::new(3, 4);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM_VERSION {
        return;
    }
    for call in context.nodes_of("call") {
        let Some(method) = call.field("method") else {
            continue;
        };
        match context.source.node_text(method) {
            "each_with_object" => check_each_with_object(context, call, method, offenses),
            "transform_values" => check_transform_values(context, call, offenses),
            _ => {}
        }
    }
}

/// `check_each_with_object`.
fn check_each_with_object(
    context: &RuleContext<'_>,
    call: Node<'_>,
    method: Node<'_>,
    offenses: &mut Vec<Offense>,
) {
    let Some(block) = call.field("block") else {
        return;
    };
    if !is_zero_default_hash(context, call) || !counts_into_hash(context, block) {
        return;
    }
    offenses.push(replace_with_tally(
        context,
        MSG_EACH_WITH_OBJECT,
        method,
        block.end_byte(),
    ));
}

/// `(send (const {nil? cbase} :Hash) :new (int 0))` as the only argument.
fn is_zero_default_hash(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    let call_arguments = arguments(call);
    let [only] = call_arguments.as_slice() else {
        return false;
    };
    let [argument] = only.parts() else {
        return false;
    };
    if argument.kind_str() != "call"
        || argument
            .field("method")
            .map(|name| context.source.node_text(name))
            != Some("new")
    {
        return false;
    }
    if !argument
        .field("receiver")
        .is_some_and(|receiver| is_named_constant(receiver, "Hash", context))
    {
        return false;
    }
    let inner = arguments(*argument);
    let [zero] = inner.as_slice() else {
        return false;
    };
    matches!(zero.parts(), [only] if only.kind_str() == "integer"
        && context.source.node_text(*only) == "0")
}

/// `(op_asgn (send (lvar _hash) :[] (lvar _elem)) :+ (int 1))` under a two-parameter block.
fn counts_into_hash(context: &RuleContext<'_>, block: Node<'_>) -> bool {
    let Some((element, hash)) = two_parameters(context, block) else {
        return false;
    };
    let statements = body_statements(block);
    let [statement] = statements.as_slice() else {
        return false;
    };
    if statement.kind_str() != "operator_assignment"
        || statement
            .field("operator")
            .map(|operator| context.source.node_text(operator))
            != Some("+=")
    {
        return false;
    }
    if statement
        .field("right")
        .map(|right| context.source.node_text(right))
        != Some("1")
    {
        return false;
    }
    let Some(left) = statement.field("left") else {
        return false;
    };
    left.kind_str() == "element_reference"
        && left
            .field("object")
            .is_some_and(|object| names(object, &hash, context))
        && matches!(super::nodes::children(left).as_slice(),
            [_, index] if names(*index, &element, context))
}

/// The element and hash names a block takes, whether declared or numbered.
fn two_parameters(context: &RuleContext<'_>, block: Node<'_>) -> Option<(String, String)> {
    match block.field("parameters") {
        Some(parameters) => match super::nodes::children(parameters).as_slice() {
            [element, hash]
                if element.kind_str() == "identifier" && hash.kind_str() == "identifier" =>
            {
                Some((
                    context.source.node_text(*element).to_owned(),
                    context.source.node_text(*hash).to_owned(),
                ))
            }
            _ => None,
        },
        // `(numblock ... 2 ...)`: the block reads `_1` and `_2` without declaring them.
        None => Some(("_1".to_owned(), "_2".to_owned())),
    }
}

/// `check_transform_values`.
fn check_transform_values(context: &RuleContext<'_>, call: Node<'_>, offenses: &mut Vec<Offense>) {
    let Some(receiver) = call.field("receiver") else {
        return;
    };
    if !is_group_by_identity(context, receiver) {
        return;
    }
    let counted = match call.field("block") {
        // `.transform_values { |v| v.count }`.
        Some(block) => counts_block_value(context, block),
        // `.transform_values(&:count)`.
        None => block_pass_name(context, call).is_some_and(|name| COUNTING_METHODS.contains(&name)),
    };
    if !counted {
        return;
    }
    let Some(selector) = receiver.field("method") else {
        return;
    };
    offenses.push(replace_with_tally(
        context,
        MSG_GROUP_BY,
        selector,
        call.end_byte(),
    ));
}

/// `{(call _ :group_by (block_pass (sym :itself))) (block (call _ :group_by) (args (arg _x)) (lvar _x)) ...}`.
fn is_group_by_identity(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind_str() != "call"
        || node
            .field("method")
            .map(|name| context.source.node_text(name))
            != Some("group_by")
    {
        return false;
    }
    match node.field("block") {
        None => block_pass_name(context, node) == Some("itself"),
        Some(block) => {
            let statements = body_statements(block);
            let [statement] = statements.as_slice() else {
                return false;
            };
            match single_parameter(context, block) {
                Some(name) => names(*statement, &name, context),
                None => false,
            }
        }
    }
}

/// `.transform_values { |v| v.count }` and its numbered and `it` forms.
fn counts_block_value(context: &RuleContext<'_>, block: Node<'_>) -> bool {
    let Some(value) = single_parameter(context, block) else {
        return false;
    };
    let statements = body_statements(block);
    let [statement] = statements.as_slice() else {
        return false;
    };
    statement.kind_str() == "call"
        && arguments(*statement).is_empty()
        && statement.field("block").is_none()
        && statement
            .field("method")
            .is_some_and(|name| COUNTING_METHODS.contains(&context.source.node_text(name)))
        && statement
            .field("receiver")
            .is_some_and(|receiver| names(receiver, &value, context))
}

/// The one name a block reads its value by, declared or implicit.
fn single_parameter(context: &RuleContext<'_>, block: Node<'_>) -> Option<String> {
    match block.field("parameters") {
        Some(parameters) => match super::nodes::children(parameters).as_slice() {
            [only] if only.kind_str() == "identifier" => {
                Some(context.source.node_text(*only).to_owned())
            }
            _ => None,
        },
        // `numblock` reads `_1` and `itblock` reads `it`; the body tells which.
        None => Some(implicit_name(context, block)),
    }
}

/// Which implicit name the body reads. `_1` is the one a `numblock` has, `it` an `itblock`.
fn implicit_name(context: &RuleContext<'_>, block: Node<'_>) -> String {
    if context.target_ruby_version() < IT_VERSION {
        return "_1".to_owned();
    }
    let mut stack: Vec<Node<'_>> = block.field("body").into_iter().collect();
    while let Some(node) = stack.pop() {
        if node.kind_str() == "identifier" && context.source.node_text(node) == "it" {
            return "it".to_owned();
        }
        crate::rules::push_named_children(node, &mut stack);
    }
    "_1".to_owned()
}

/// `(block_pass (sym %name))`: the only argument is `&:name`.
fn block_pass_name<'a>(context: &'a RuleContext<'_>, call: Node<'_>) -> Option<&'a str> {
    let call_arguments = arguments(call);
    let [only] = call_arguments.as_slice() else {
        return None;
    };
    let [argument] = only.parts() else {
        return None;
    };
    if argument.kind_str() != "block_argument" {
        return None;
    }
    symbol_name(argument.named_child(0)?, context)
}

fn is_named_constant(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == name,
        "scope_resolution" => {
            node.field("scope").is_none()
                && node
                    .field("name")
                    .is_some_and(|inner| context.source.node_text(inner) == name)
        }
        _ => false,
    }
}

fn names(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "identifier" && context.source.node_text(node) == name
}

/// `corrector.replace(replacement_range(start_node, end_node), 'tally')`.
fn replace_with_tally(
    context: &RuleContext<'_>,
    message: &str,
    selector: Node<'_>,
    end: usize,
) -> Offense {
    context
        .offense(message, selector.byte_range())
        .corrected_by(Edit {
            start: selector.start_byte(),
            end,
            replacement: "tally".to_owned(),
            safe: true,
        })
}
