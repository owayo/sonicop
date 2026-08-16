use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{
    arguments, has_interpolation, is_string, string_text, symbol_name, top_level_constant,
};

/// `DATA_METHOD_NAMES`: the methods `Data` already answers, which a member of the same name hides.
const DATA_METHOD_NAMES: &[&str] = &[
    "!",
    "!=",
    "!~",
    "<=>",
    "==",
    "===",
    "__id__",
    "__send__",
    "class",
    "clone",
    "deconstruct",
    "deconstruct_keys",
    "define_singleton_method",
    "display",
    "dup",
    "enum_for",
    "eql?",
    "equal?",
    "extend",
    "freeze",
    "frozen?",
    "hash",
    "inspect",
    "instance_eval",
    "instance_exec",
    "instance_of?",
    "instance_variable_defined?",
    "instance_variable_get",
    "instance_variable_set",
    "instance_variables",
    "is_a?",
    "itself",
    "kind_of?",
    "members",
    "method",
    "methods",
    "nil?",
    "object_id",
    "private_methods",
    "protected_methods",
    "public_method",
    "public_methods",
    "public_send",
    "remove_instance_variable",
    "respond_to?",
    "send",
    "singleton_class",
    "singleton_method",
    "singleton_methods",
    "tap",
    "then",
    "to_enum",
    "to_h",
    "to_s",
    "with",
    "yield_self",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
            continue;
        };
        // `on_send` は `csend` に呼ばれない。`alias on_csend on_send` を書いていない cop は
        // `x&.foo` を構造的に一切見ないので、ここで落とさないと過剰検出になる。
        if !crate::rules::send_node::is_plain_send(node, context) {
            continue;
        }
        if context.source.node_text(method) != "define"
            || !top_level_constant(receiver, "Data", context)
        {
            continue;
        }
        for argument in arguments(node) {
            let node = argument.first();
            // `MEMBER_NAME_TYPES`: a `sym` prints back with a colon and a `str` with quotes, which
            // is the difference between the two spellings of the message.
            let (name, quoted) = if let Some(name) = symbol_name(node, context) {
                (name, format!(":{name}"))
            } else if is_string(node, context) && !has_interpolation(node) {
                let name = string_text(node, context);
                (name, format!("\"{name}\""))
            } else {
                continue;
            };
            if !DATA_METHOD_NAMES.contains(&name) {
                continue;
            }
            let message =
                format!("`{quoted}` member overrides `Data#{name}` and it may be unexpected.");
            offenses.push(context.offense(message, argument.range()));
        }
    }
}
