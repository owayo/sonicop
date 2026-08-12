//! `Layout/SpaceInLambdaLiteral`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let require_space = context
        .setting::<String>("EnforcedStyle")
        .as_deref()
        .map(|style| style == "require_space")
        .unwrap_or(false);
    for lambda in context.nodes_of("lambda") {
        // `node.parent.arguments?`: an empty parameter list is no list at all upstream.
        let Some(parameters) = lambda
            .child_by_field_name("parameters")
            .filter(|parameters| parameters.named_child_count() > 0)
        else {
            continue;
        };
        // `space_after_arrow`: everything between the end of `->` and the parameter list.
        let Some(arrow) = lambda.child(0).filter(|arrow| arrow.kind() == "->") else {
            continue;
        };
        let space = arrow.end_byte()..parameters.start_byte();
        if require_space {
            if !space.is_empty() {
                continue;
            }
            let range = lambda.start_byte()..parameters.end_byte();
            offenses.push(
                context
                    .offense(
                        "Use a space between `->` and `(` in lambda literals.",
                        range,
                    )
                    .corrected_by(Edit {
                        start: parameters.start_byte(),
                        end: parameters.start_byte(),
                        replacement: " ".to_owned(),
                        safe: true,
                    })
                    .corrections_anchored_at(parameters.byte_range()),
            );
        } else if !space.is_empty() {
            offenses.push(
                context
                    .offense(
                        "Do not use spaces between `->` and `(` in lambda literals.",
                        space.clone(),
                    )
                    .corrected_by(Edit {
                        start: space.start,
                        end: space.end,
                        replacement: String::new(),
                        safe: true,
                    }),
            );
        }
    }
}
