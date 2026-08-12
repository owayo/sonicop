//! `Style/StabbyLambdaParentheses`: whether `->a, b { }` wraps its arguments.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG_REQUIRE: &str = "Wrap stabby lambda arguments with parentheses.";
const MSG_NO_REQUIRE: &str = "Do not wrap stabby lambda arguments with parentheses.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let require_parentheses = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "require_parentheses");

    for node in context.nodes_of("lambda") {
        // `stabby_lambda_with_args?`: only a literal that named arguments is in scope.
        let Some(parameters) = node.child_by_field_name("parameters") else {
            continue;
        };
        if super::nodes::children(parameters).is_empty() {
            continue;
        }
        let text = context.source.node_text(parameters);
        let parenthesized = text.starts_with('(');
        if parenthesized == require_parentheses {
            continue;
        }
        let range = parameters.byte_range();
        let offense = context.offense(
            if require_parentheses {
                MSG_REQUIRE
            } else {
                MSG_NO_REQUIRE
            },
            range.clone(),
        );
        let edits = if require_parentheses {
            // `corrector.wrap(node, '(', ')')`.
            vec![
                Edit {
                    start: range.start,
                    end: range.start,
                    replacement: "(".to_owned(),
                    safe: true,
                },
                Edit {
                    start: range.end,
                    end: range.end,
                    replacement: ")".to_owned(),
                    safe: true,
                },
            ]
        } else {
            vec![
                Edit {
                    start: range.start,
                    end: range.start + 1,
                    replacement: String::new(),
                    safe: true,
                },
                Edit {
                    start: range.end - 1,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                },
            ]
        };
        offenses.push(offense.corrected_by_all(edits));
    }
}
