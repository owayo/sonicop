use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(5);
    let count_keywords: bool = context.setting("CountKeywordArgs").unwrap_or(true);
    let max_optional: usize = context.setting("MaxOptionalParameters").unwrap_or(3);

    // RuboCop runs two independent checks: `on_def`/`on_defs` counts the optional parameters of a
    // method, while `on_args` counts every parameter list -- blocks included, since `on_args` fires
    // for a block's `args` node too.
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        report_optional_parameters(context, offenses, node, max_optional);
    }
    for node in
        context.nodes_of_any(&["method_parameters", "block_parameters", "lambda_parameters"])
    {
        report_parameter_count(context, offenses, node, max, count_keywords);
    }
}

fn report_optional_parameters(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    max: usize,
) {
    let count = node
        .child_by_field_name("parameters")
        .map_or(0, |parameters| {
            let mut cursor = parameters.walk();
            parameters
                .named_children(&mut cursor)
                .filter(|parameter| parameter.kind() == "optional_parameter")
                .count()
        });
    if count <= max {
        return;
    }
    // Reported against the whole definition, not the parameter list: RuboCop's `on_def` adds the
    // offense on the `def` node itself, so both offenses can land on the same line at once.
    offenses.push(context.offense(
        format!("Method has too many optional parameters. [{count}/{max}]"),
        node.byte_range(),
    ));
}

fn report_parameter_count(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    max: usize,
    count_keywords: bool,
) {
    if struct_or_data_initialize(context, node) {
        return;
    }
    let mut cursor = node.walk();
    // An explicit block argument is never counted: making it implicit is a rename away, so
    // counting it would push authors toward a change the cop does not actually want.
    let count = node
        .named_children(&mut cursor)
        .filter(|parameter| parameter.kind() != "block_parameter")
        .filter(|parameter| count_keywords || parameter.kind() != "keyword_parameter")
        .count();
    if count <= max || argument_to_lambda_or_proc(context, node) {
        return;
    }
    offenses.push(context.offense(
        format!("Avoid parameter lists longer than {max} parameters. [{count}/{max}]"),
        node.byte_range(),
    ));
}

/// `Struct.new(...) { def initialize(...) }` and `Data.define(...) { ... }` mirror the member list
/// in `initialize`, so counting its parameters would only report the member list twice.
///
/// RuboCop matches the block as the *direct* parent of the `def`, so the exemption is lost as soon
/// as the block holds a second statement. tree-sitter always interposes a `body_statement`, which
/// is why the def has to be the block's only statement here rather than merely one of them.
fn struct_or_data_initialize(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(method) = node.parent().filter(|parent| parent.kind() == "method") else {
        return false;
    };
    if method
        .child_by_field_name("name")
        .is_none_or(|name| context.source.node_text(name) != "initialize")
    {
        return false;
    }
    let Some(body) = method
        .parent()
        .filter(|parent| parent.kind() == "body_statement" && parent.named_child_count() == 1)
    else {
        return false;
    };
    let Some(block) = body
        .parent()
        .filter(|parent| matches!(parent.kind(), "block" | "do_block"))
    else {
        return false;
    };
    // The pattern spells the block's parameter list as `(args)`, so a block that takes parameters
    // of its own does not qualify.
    if block.child_by_field_name("parameters").is_some() {
        return false;
    }
    block
        .parent()
        .filter(|parent| parent.kind() == "call")
        .is_some_and(|call| {
            matches!(
                constructor_call(context, call),
                Some(("Struct", "new") | ("Data", "define"))
            )
        })
}

/// `lambda`, `proc` and `Proc.new` take whatever arity their body needs, so RuboCop leaves their
/// parameter lists alone.
fn argument_to_lambda_or_proc(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "lambda" {
        return true;
    }
    if !matches!(parent.kind(), "block" | "do_block") {
        return false;
    }
    let Some(call) = parent.parent().filter(|node| node.kind() == "call") else {
        return false;
    };
    let Some(method) = call.child_by_field_name("method") else {
        return false;
    };
    match call.child_by_field_name("receiver") {
        None => matches!(context.source.node_text(method), "lambda" | "proc"),
        Some(_) => matches!(constructor_call(context, call), Some(("Proc", "new"))),
    }
}

/// The receiver constant and method name of `Const.method(...)`, for the receiver being a plain
/// (possibly `::`-rooted) constant -- RuboCop's `#global_const?` accepts exactly those two shapes,
/// so a namespaced `Foo::Struct` must not match.
fn constructor_call<'a>(
    context: &'a RuleContext<'_>,
    call: Node<'_>,
) -> Option<(&'a str, &'a str)> {
    let receiver = call.child_by_field_name("receiver")?;
    let method = call.child_by_field_name("method")?;
    let name = context
        .source
        .node_text(receiver)
        .strip_prefix("::")
        .unwrap_or_else(|| context.source.node_text(receiver));
    if !matches!(receiver.kind(), "constant" | "scope_resolution") || name.contains("::") {
        return None;
    }
    Some((name, context.source.node_text(method)))
}
