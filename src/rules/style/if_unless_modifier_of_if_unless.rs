use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// The node kinds upstream's `if_type?` covers: both keywords, both modifier forms and the
/// ternary.
const CONDITIONALS: &[&str] = &[
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["if_modifier", "unless_modifier"]) {
        let (Some(body), Some(condition)) = (
            node.child_by_field_name("body"),
            node.child_by_field_name("condition"),
        ) else {
            continue;
        };
        if !CONDITIONALS.contains(&body.kind()) {
            continue;
        }
        let Some(keyword) = super::conditional::token(node, &["if", "unless"]) else {
            continue;
        };
        let keyword_source = context.source.node_text(keyword);
        let message = format!("Avoid modifier `{keyword_source}` after another conditional.");
        offenses.push(
            context
                .offense(message, keyword.byte_range())
                .corrected_by_all([
                    // `corrector.wrap(node.if_branch, ...)`: the condition moves in front of the
                    // body it guards.
                    Edit {
                        start: body.start_byte(),
                        end: body.start_byte(),
                        replacement: format!(
                            "{keyword_source} {}\n",
                            context.source.node_text(condition)
                        ),
                        safe: true,
                    },
                    Edit {
                        start: body.end_byte(),
                        end: body.end_byte(),
                        replacement: "\nend".to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: body.end_byte(),
                        end: condition.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                ])
                // Both insertions hang off the body rather than off the keyword reported.
                .corrections_anchored_at(body.byte_range()),
        );
    }
}
