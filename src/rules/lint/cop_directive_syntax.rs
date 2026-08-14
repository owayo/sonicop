use crate::diagnostic::Offense;
use crate::directives::directive_syntax_problem;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for comment in context.comment_ranges() {
        let Some(problem) = directive_syntax_problem(context.source.slice(comment.clone())) else {
            continue;
        };
        offenses.push(context.offense(
            format!("Malformed directive comment detected. {problem}"),
            comment.clone(),
        ));
    }
}
