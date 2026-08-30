use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::lint::node_equality::identical;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `%s` instead of `%s`.";
/// `"#{MSG.chop} and remove the unused `%<unused_code>s` block argument."`.
const UNUSED_BLOCK_ARG_MSG: &str =
    "Use `%s` instead of `%s` and remove the unused `%s` block argument.";

const ARRAY_CONVERTER_METHODS: &[&str] = &[
    "assoc", "chunk", "flatten", "rassoc", "sort", "sort_by", "to_a",
];

/// The node kinds upstream's `Node#literal?` lists, as the grammar spells them.
const LITERAL_KINDS: &[&str] = &[
    "string",
    "chained_string",
    "subshell",
    "character",
    "integer",
    "float",
    "complex",
    "rational",
    "simple_symbol",
    "delimited_symbol",
    "hash_key_symbol",
    "array",
    "string_array",
    "symbol_array",
    "hash",
    "regex",
    "true",
    "false",
    "nil",
    "range",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        match node.field("block") {
            Some(block) => on_block(context, offenses, &locals, node, block),
            None => on_block_pass(context, offenses, node),
        }
    }
}

fn on_block(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    block: Node<'_>,
) {
    if !handleable(context, node) {
        return;
    }
    // `kv_each(node) { |target, method| register_kv_offense(target, method) and return }`: the
    // walk goes on when the offense was declined, which is how `keys.each { |k, v| }` still
    // reaches the unused-argument check.
    if let Some((inner, method)) = kv_each(context, node) {
        if register_kv_offense(context, offenses, node, inner, method) {
            return;
        }
    }
    let Some((key, value)) = each_arguments(context, node, block) else {
        return;
    };
    check_unused_block_args(context, offenses, locals, node, block, key, value);
}

/// `handleable?`: the receiver has to look like a hash that the block does not rewrite.
fn handleable(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    // `use_array_converter_method_as_preceding?`: `hash.to_a.each { |k, v| }` walks an array of
    // pairs, where both block arguments mean something else.
    if let Some(preceding) = node.field("receiver") {
        if preceding.kind_str() == "call"
            && method_name(context, preceding)
                .is_some_and(|name| ARRAY_CONVERTER_METHODS.contains(&name))
        {
            return false;
        }
    }
    let Some(root) = root_receiver(node) else {
        return false;
    };
    if hash_mutated(context, node, root) {
        return false;
    }
    !LITERAL_KINDS.contains(&root.kind_str()) || root.kind_str() == "hash"
}

/// `root_receiver`: the leftmost receiver of the chain.
fn root_receiver<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut receiver = node.field("receiver")?;
    while let Some(inner) = receiver.field("receiver") {
        receiver = inner;
    }
    Some(receiver)
}

/// `hash_mutated?`: whether the block writes back into the same receiver, which `each_key` and
/// `each_value` would not let it do.
fn hash_mutated(context: &RuleContext<'_>, node: Node<'_>, root: Node<'_>) -> bool {
    send_node::any_descendant(node, &mut |candidate| {
        let target = match candidate.kind_str() {
            // `h[k] = v` is `(send h :[]= k v)`: this builder does not emit `index` nodes.
            "assignment" => candidate
                .field("left")
                .filter(|left| left.kind_str() == "element_reference")
                .and_then(|left| left.field("object")),
            "call" => method_name(context, candidate)
                .filter(|name| *name == "[]=")
                .and_then(|_| candidate.field("receiver")),
            _ => None,
        };
        target.is_some_and(|target| identical(target, root, context))
    })
}

/// `kv_each`: `(any_block $(call (call _ {:keys :values}) :each) ...)`.
fn kv_each<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str)> {
    if method_name(context, node)? != "each" || !send_node::arguments(node).is_empty() {
        return None;
    }
    let inner = node.field("receiver")?;
    if inner.kind_str() != "call" || !send_node::arguments(inner).is_empty() {
        return None;
    }
    match method_name(context, inner)? {
        "keys" => Some((inner, "keys")),
        "values" => Some((inner, "values")),
        _ => None,
    }
}

/// `register_kv_offense`. Reports whether an offense was added, which is what decides whether the
/// unused-argument check still runs.
fn register_kv_offense(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    inner: Node<'_>,
    method: &str,
) -> bool {
    let (Some(parent_receiver), Some(inner_selector), Some(selector)) = (
        inner.field("receiver"),
        inner.field("method"),
        node.field("method"),
    ) else {
        return false;
    };
    if allowed_receiver(context, parent_receiver) {
        return false;
    }
    let send = send_node::send_range(node, context);
    // `target.receiver.loc.selector.join(target.source_range.end)`.
    let current = context.source.slice(inner_selector.start_byte()..send.end);
    let prefer = preferred(method);
    let message = format_message(&prefer, current);
    // `kv_range`: the two selectors and the dot between them.
    let range = inner_selector.start_byte()..selector.end_byte();

    // `correct_key_value_each`: the whole chain is rewritten, and the dot the *outer* call was
    // written with is the one that survives.
    let dot = node
        .field("operator")
        .map_or(".", |operator| context.source.node_text(operator));
    let replacement = format!("{}{dot}{prefer}", context.source.node_text(parent_receiver));
    offenses.push(context.offense(message, range).corrected_by(Edit {
        start: send.start,
        end: send.end,
        replacement,
        safe: false,
    }));
    true
}

/// `on_block_pass`: `(call $(call _ {:keys :values}) :each (block_pass (sym _)))`.
///
/// This one is checked without `handleable?`, so a receiver the block form would have declined is
/// still rewritten here.
fn on_block_pass(context: &RuleContext<'_>, offenses: &mut Vec<Offense>, node: Node<'_>) {
    if method_name(context, node) != Some("each") {
        return;
    }
    let arguments = send_node::arguments(node);
    let [argument] = arguments.as_slice() else {
        return;
    };
    let block_pass = argument.first();
    if block_pass.kind_str() != "block_argument" {
        return;
    }
    let symbol = send_node::named_children_of(block_pass, context);
    if !matches!(symbol.as_slice(), [only] if send_node::symbol_name(*only, context).is_some()) {
        return;
    }
    let Some((inner, method)) = kv_each_receiver(context, node) else {
        return;
    };
    let (Some(parent_receiver), Some(inner_selector), Some(selector)) = (
        inner.field("receiver"),
        inner.field("method"),
        node.field("method"),
    ) else {
        return;
    };
    if allowed_receiver(context, parent_receiver) {
        return;
    }
    let range = inner_selector.start_byte()..selector.end_byte();
    let prefer = preferred(method);
    let message = format_message(&prefer, context.source.slice(range.clone()));
    offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: prefer,
        safe: false,
    }));
}

/// The `(call _ {:keys :values})` half of the block-pass pattern, which unlike `kv_each` says
/// nothing about the arguments of the `each` it hangs off.
fn kv_each_receiver<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str)> {
    let inner = node.field("receiver")?;
    if inner.kind_str() != "call" || !send_node::arguments(inner).is_empty() {
        return None;
    }
    match method_name(context, inner)? {
        "keys" => Some((inner, "keys")),
        "values" => Some((inner, "values")),
        _ => None,
    }
}

/// `each_arguments`: `(block (call _ :each)(args $_key $_value) ...)`.
fn each_arguments<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    block: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    if method_name(context, node)? != "each" || !send_node::arguments(node).is_empty() {
        return None;
    }
    let parameters = send_node::named_children(block.field("parameters")?);
    match parameters.as_slice() {
        [key, value] => Some((*key, *value)),
        _ => None,
    }
}

/// `check_unused_block_args`: one of the two arguments going unread names which half of the hash
/// the block actually walks.
fn check_unused_block_args(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    block: Node<'_>,
    key: Node<'_>,
    value: Node<'_>,
) {
    let Some(body) = block.field("body") else {
        return;
    };
    let read = local_reads(context, locals, body);
    let value_unused = is_unused(context, value, &read);
    let key_unused = is_unused(context, key, &read);
    if value_unused && key_unused {
        return;
    }
    let (prefer, unused, unused_range) = if value_unused {
        ("each_key", value, key.end_byte()..value.end_byte())
    } else if key_unused {
        ("each_value", key, key.start_byte()..value.start_byte())
    } else {
        return;
    };
    let Some(selector) = node.field("method") else {
        return;
    };
    let message = UNUSED_BLOCK_ARG_MSG
        .replacen("%s", prefer, 1)
        .replacen("%s", context.source.node_text(selector), 1)
        .replacen("%s", context.source.node_text(unused), 1);
    offenses.push(
        context
            .offense(message, node.byte_range())
            .corrected_by_all([
                Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: prefer.to_owned(),
                    safe: false,
                },
                Edit {
                    start: unused_range.start,
                    end: unused_range.end,
                    replacement: String::new(),
                    safe: false,
                },
            ]),
    );
}

/// The source of every `lvar` the body reads, which is what upstream compares a block argument's
/// name against.
fn local_reads<'a>(
    context: &'a RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    body: Node<'_>,
) -> Vec<&'a str> {
    let mut found = Vec::new();
    send_node::any_descendant(body, &mut |node| {
        if node.kind_str() == "identifier" && locals.is_lvar(node) {
            found.push(context.source.node_text(node));
        }
        // `foo(bar:)` is `(pair (sym :bar) (lvar :bar))` once `bar` is a local, and the name a
        // block parameter binds always is one -- the grammar leaves the value unwritten, so the
        // read has no node of its own to find.
        if node.kind_str() == "pair" && node.field("value").is_none() {
            if let Some(key) = node.field("key") {
                if let Some(name) = send_node::symbol_name(key, context) {
                    found.push(name);
                }
            }
        }
        false
    });
    found
}

/// `unused_block_arg_exist?`.
fn is_unused(context: &RuleContext<'_>, argument: Node<'_>, read: &[&str]) -> bool {
    if argument.kind_str() == "destructured_parameter" {
        // `each_descendant(:arg, :restarg)`: every name the destructuring binds.
        let mut names = Vec::new();
        send_node::any_descendant(argument, &mut |node| {
            if node.kind_str() == "identifier" {
                names.push(context.source.node_text(node));
            }
            false
        });
        return names.iter().all(|name| !read.contains(name));
    }
    let name = context
        .source
        .node_text(argument)
        .strip_prefix('*')
        .unwrap_or_else(|| context.source.node_text(argument));
    !read.contains(&name)
}

/// `allowed_receiver?` with `receiver_name`, which spells a chain of receiverless calls as the
/// dotted name the configuration lists.
fn allowed_receiver(context: &RuleContext<'_>, receiver: Node<'_>) -> bool {
    let allowed: Vec<String> = context.setting("AllowedReceivers").unwrap_or_default();
    allowed.contains(&receiver_name(context, receiver))
}

fn receiver_name(context: &RuleContext<'_>, receiver: Node<'_>) -> String {
    let inner = receiver.field("receiver");
    if let Some(inner) = inner.filter(|inner| !is_constant(*inner)) {
        return receiver_name(context, inner);
    }
    if receiver.kind_str() != "call" {
        return context.source.node_text(receiver).to_owned();
    }
    let name = method_name(context, receiver).unwrap_or_default();
    match inner {
        Some(inner) => format!("{}.{name}", receiver_name(context, inner)),
        None => name.to_owned(),
    }
}

fn is_constant(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "constant" | "scope_resolution")
}

fn preferred(method: &str) -> String {
    format!("each_{}", &method[..method.len() - 1])
}

fn format_message(prefer: &str, current: &str) -> String {
    MSG.replacen("%s", prefer, 1).replacen("%s", current, 1)
}

fn method_name<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    node.field("method")
        .map(|method| context.source.node_text(method))
}
