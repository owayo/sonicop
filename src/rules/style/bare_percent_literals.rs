//! `Style/BarePercentLiterals`: whether a percent string is opened with `%(` or `%Q(`.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let percent_q = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "percent_q");

    // `node.heredoc?` and `node.loc?(:begin)` both fall out of the opener: a heredoc is a node kind
    // of its own here, and a literal written with quotes has no percent type.
    for node in context.nodes_of("string") {
        let Some(literal) = super::percent::PercentLiteral::new(node, context) else {
            continue;
        };
        let (good, bad) = if percent_q && literal.percent_type == "%" {
            ("Q", "")
        } else if !percent_q && literal.percent_type == "%Q" {
            ("", "Q")
        } else {
            continue;
        };
        let replacement = if literal.percent_type == "%Q" {
            format!("%{}", literal.opening)
        } else {
            format!("%Q{}", literal.opening)
        };
        offenses.push(
            context
                .offense(
                    format!("Use `%{good}` instead of `%{bad}`."),
                    literal.begin.clone(),
                )
                .corrected_by(Edit {
                    start: literal.begin.start,
                    end: literal.begin.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}
