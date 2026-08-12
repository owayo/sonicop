use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `OnNormalIfUnless` skips the modifier and ternary forms; an `elsif` is an `if` upstream and
    // reports under its own keyword.
    for node in context.nodes_of_any(&["if", "unless", "elsif"]) {
        if node.start_position().row == node.end_position().row {
            continue;
        }
        let Some(consequence) = node.child_by_field_name("consequence") else {
            continue;
        };
        let Some(then) = super::conditional::token(consequence, &["then"]) else {
            continue;
        };
        // `node.loc.begin.line != node.if_branch&.loc&.line`: a body starting on the `then`'s own
        // line still needs it.
        if super::nodes::children(consequence)
            .first()
            .is_some_and(|branch| branch.start_position().row == then.start_position().row)
        {
            continue;
        }
        let message = format!("Do not use `then` for multi-line `{}`.", node.kind());
        offenses.push(
            context
                .offense(message, then.byte_range())
                .corrected_by(Edit {
                    start: super::ranges::extended_left(
                        context.source.text(),
                        then.start_byte(),
                        true,
                    ),
                    end: then.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
