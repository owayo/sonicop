//! `nil.methods`, which is what the `NilMethods` mixin asks a chained call about.
//!
//! Upstream reads the list off the running interpreter, so it is the Ruby that RuboCop itself runs
//! on -- not the `TargetRubyVersion` -- that decides it. The list below is `nil.methods` of the
//! Ruby the reference implementation is checked against; the four methods that came and went in
//! older interpreters (`=~` was removed for `Object` but kept for `nil`, `then`/`yield_self` and
//! `to_c`/`rationalize` arrived in 2.5 and 2.6) are all in it.

use crate::rules::RuleContext;

/// `nil.methods`, sorted.
pub(super) const NIL_METHODS: &[&str] = &[
    "!",
    "!=",
    "!~",
    "&",
    "<=>",
    "==",
    "===",
    "=~",
    "^",
    "__id__",
    "__send__",
    "class",
    "clone",
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
    "method",
    "methods",
    "nil?",
    "object_id",
    "private_methods",
    "protected_methods",
    "public_method",
    "public_methods",
    "public_send",
    "rationalize",
    "remove_instance_variable",
    "respond_to?",
    "send",
    "singleton_class",
    "singleton_method",
    "singleton_methods",
    "tap",
    "then",
    "to_a",
    "to_c",
    "to_enum",
    "to_f",
    "to_h",
    "to_i",
    "to_r",
    "to_s",
    "yield_self",
    "|",
];

/// `other_stdlib_methods`: BigDecimal defines `nil.to_d`, which is not loaded by default.
const OTHER_STDLIB_METHODS: &[&str] = &["to_d"];

/// `nil_methods`: the interpreter's list, the one stdlib addition, and the cop's `AllowedMethods`.
pub(super) fn nil_methods(context: &RuleContext<'_>) -> Vec<String> {
    let mut methods: Vec<String> = NIL_METHODS
        .iter()
        .chain(OTHER_STDLIB_METHODS)
        .map(|&name| name.to_owned())
        .collect();
    methods.extend(
        context
            .setting::<Vec<String>>("AllowedMethods")
            .unwrap_or_default(),
    );
    methods
}
