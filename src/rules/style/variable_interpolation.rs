//! `Style/VariableInterpolation`: `"#@name"` says what `"#{@name}"` says, with less to read.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// The node kinds `variable?` and `reference?` cover, minus the local variable a short
/// interpolation cannot name.
const VARIABLE_KINDS: &[&str] = &[
    "instance_variable",
    "class_variable",
    "global_variable",
    "nth_ref",
    "back_ref",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("interpolation") {
        // `#{...}` puts the expression in a `begin` node upstream, which is neither a variable nor
        // a reference; only the short form hands the cop the variable itself. Here the two are
        // told apart by the `#{` token, which the short form does not write.
        let Some(variable) = node.child(0).filter(|first| first.is_named()) else {
            continue;
        };
        if !VARIABLE_KINDS.contains(&variable.kind()) {
            continue;
        }
        let text = context.source.node_text(variable);
        offenses.push(
            context
                .offense(
                    format!(
                        "Replace interpolated variable `{text}` with expression `#{{{text}}}`."
                    ),
                    variable.byte_range(),
                )
                .corrected_by(Edit {
                    start: variable.start_byte(),
                    end: variable.end_byte(),
                    replacement: format!("{{{text}}}"),
                    safe: true,
                }),
        );
    }
}
