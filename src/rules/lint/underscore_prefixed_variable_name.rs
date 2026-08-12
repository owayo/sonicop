use super::variable_force::{Analysis, Argument, Declaration, Scope, Variable};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Do not use prefix `_` for a variable that is used.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_keyword_block_arguments: bool = context
        .setting("AllowKeywordBlockArguments")
        .unwrap_or(false);
    let analysis = Analysis::run(context.root_node(), context.source);
    for scope in &analysis.scopes {
        for &index in &scope.variables {
            let variable = &analysis.variables[index];
            // A name a zero-arity `super` or a `binding` call reads is not one the author wrote a
            // read for, so the underscore still says what it meant to.
            if !variable.should_be_unused()
                || !variable.referenced_explicitly
                || (allow_keyword_block_arguments && keyword_block_argument(variable, scope))
            {
                continue;
            }
            offenses.push(context.offense(MSG, variable.name_node.byte_range()));
        }
    }
}

fn keyword_block_argument(variable: &Variable<'_>, scope: &Scope<'_>) -> bool {
    variable.kind == Declaration::Argument(Argument::Keyword)
        && matches!(scope.node.kind(), "block" | "do_block" | "lambda")
}
