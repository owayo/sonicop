use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{
    arguments, has_interpolation, is_plain_send, string_text, symbol_name, top_level_constant,
};
use crate::rules::node_ext::NodeExt;

/// `Struct.instance_methods.sort` in Ruby 4.0.0, transcribed from `STRUCT_METHOD_NAMES`.
const STRUCT_METHOD_NAMES: [&str; 124] = [
    "!",
    "!=",
    "!~",
    "<=>",
    "==",
    "===",
    "[]",
    "[]=",
    "__id__",
    "__send__",
    "all?",
    "any?",
    "chain",
    "chunk",
    "chunk_while",
    "class",
    "clone",
    "collect",
    "collect_concat",
    "compact",
    "count",
    "cycle",
    "deconstruct",
    "deconstruct_keys",
    "define_singleton_method",
    "detect",
    "dig",
    "display",
    "drop",
    "drop_while",
    "dup",
    "each",
    "each_cons",
    "each_entry",
    "each_pair",
    "each_slice",
    "each_with_index",
    "each_with_object",
    "entries",
    "enum_for",
    "eql?",
    "equal?",
    "extend",
    "filter",
    "filter_map",
    "find",
    "find_all",
    "find_index",
    "first",
    "flat_map",
    "freeze",
    "frozen?",
    "grep",
    "grep_v",
    "group_by",
    "hash",
    "include?",
    "inject",
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
    "lazy",
    "length",
    "map",
    "max",
    "max_by",
    "member?",
    "members",
    "method",
    "methods",
    "min",
    "min_by",
    "minmax",
    "minmax_by",
    "nil?",
    "none?",
    "object_id",
    "one?",
    "partition",
    "private_methods",
    "protected_methods",
    "public_method",
    "public_methods",
    "public_send",
    "reduce",
    "reject",
    "remove_instance_variable",
    "respond_to?",
    "reverse_each",
    "select",
    "send",
    "singleton_class",
    "singleton_method",
    "singleton_methods",
    "size",
    "slice_after",
    "slice_before",
    "slice_when",
    "sort",
    "sort_by",
    "sum",
    "take",
    "take_while",
    "tally",
    "tap",
    "then",
    "to_a",
    "to_enum",
    "to_h",
    "to_s",
    "to_set",
    "uniq",
    "values",
    "values_at",
    "yield_self",
    "zip",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // `(send (const {nil? cbase} :Struct) :new ...)`.
        let (Some(receiver), Some(method)) = (
            node.field("receiver"),
            node.field("method"),
        ) else {
            continue;
        };
        if context.source.node_text(method) != "new"
            || !is_plain_send(node, context)
            || !top_level_constant(receiver, "Struct", context)
        {
            continue;
        }
        for (index, argument) in arguments(node).iter().enumerate() {
            let [member] = argument.parts() else {
                continue;
            };
            let member = *member;
            let (name, quoted) = match symbol_name(member, context) {
                Some(name) => (name, format!(":{name}")),
                None => {
                    // `Struct.new("Name", ...)` names the struct rather than a member.
                    if index == 0 || member.kind_str() != "string" || has_interpolation(member) {
                        continue;
                    }
                    let name = string_text(member, context);
                    (name, format!("\"{name}\""))
                }
            };
            if !STRUCT_METHOD_NAMES.contains(&name) {
                continue;
            }
            offenses.push(context.offense(
                format!("`{quoted}` member overrides `Struct#{name}` and it may be unexpected."),
                member.byte_range(),
            ));
        }
    }
}
