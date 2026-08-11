use super::variable_force::{
    Analysis, Argument, Declaration, Scope, Variable, block_method, body_node, is_lambda,
};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_empty: bool = context.setting("IgnoreEmptyBlocks").unwrap_or(true);
    let allow_unused_keywords: bool = context
        .setting("AllowUnusedKeywordArguments")
        .unwrap_or(false);
    let analysis = Analysis::run(context.root_node(), context.source);
    for scope in &analysis.scopes {
        for &index in &scope.variables {
            let variable = &analysis.variables[index];
            if !block_argument(variable, scope)
                || (ignore_empty && body_node(scope).is_none())
                || (allow_unused_keywords && keyword_argument(variable))
                || used_block_local(variable)
                || variable.should_be_unused()
                || variable.referenced
            {
                continue;
            }
            let message = message(context, &analysis, scope, variable);
            let offense = context.offense(message, variable.name_node.byte_range());
            offenses.push(match correction(context, variable) {
                Some(edit) => offense.corrected_by(edit),
                None => offense,
            });
        }
    }
}

/// `Variable#block_argument?`. A parameter of a method is another cop's business, and a variable
/// that is merely assigned inside a block is nobody's.
fn block_argument(variable: &Variable<'_>, scope: &Scope<'_>) -> bool {
    variable.is_argument() && matches!(scope.node.kind(), "block" | "do_block" | "lambda")
}

fn keyword_argument(variable: &Variable<'_>) -> bool {
    variable.kind == Declaration::Argument(Argument::Keyword)
}

/// A block local variable exists precisely to keep a name out of the enclosing scope, so one that
/// is written to is doing its job even though nothing reads it.
fn used_block_local(variable: &Variable<'_>) -> bool {
    variable.kind == Declaration::BlockLocal && !variable.assignments.is_empty()
}

fn message(
    context: &RuleContext<'_>,
    analysis: &Analysis<'_>,
    scope: &Scope<'_>,
    variable: &Variable<'_>,
) -> String {
    let name = &variable.name;
    if variable.kind == Declaration::BlockLocal {
        return format!("Unused block local variable - `{name}`.");
    }
    let arguments: Vec<&Variable<'_>> = scope
        .variables
        .iter()
        .map(|&index| &analysis.variables[index])
        .filter(|variable| block_argument(variable, scope))
        .collect();
    let none_referenced = !arguments.iter().any(|argument| argument.referenced);
    let augmentation = if is_lambda(scope.node, context.source) {
        let mut message = underscore_message(name);
        if none_referenced {
            message.push_str(
                " Also consider using a proc without arguments instead of a lambda if you want it \
                 to accept any arguments but don't care about them.",
            );
        }
        message
    } else if none_referenced && block_method(scope.node, context.source) != Some("define_method") {
        if arguments.len() > 1 {
            "You can omit all the arguments if you don't care about them.".to_owned()
        } else {
            "You can omit the argument if you don't care about it.".to_owned()
        }
    } else {
        underscore_message(name)
    };
    format!("Unused block argument - `{name}`. {augmentation}")
}

fn underscore_message(name: &str) -> String {
    format!(
        "If it's necessary, use `_` or `_{name}` as an argument name to indicate that it won't be used."
    )
}

/// RuboCop's `UnusedArgCorrector` leaves keyword arguments alone -- prefixing one would rename the
/// keyword itself -- and deletes an explicit block argument instead of renaming it, since an
/// unused `&block` is simply surplus.
fn correction(context: &RuleContext<'_>, variable: &Variable<'_>) -> Option<Edit> {
    match variable.kind {
        Declaration::Argument(Argument::Keyword) => None,
        Declaration::Argument(Argument::Block) => {
            let start = removal_start(context, variable.declaration.start_byte());
            Some(Edit {
                start,
                end: variable.declaration.end_byte(),
                replacement: String::new(),
                safe: true,
            })
        }
        _ => Some(Edit {
            start: variable.name_node.start_byte(),
            end: variable.name_node.start_byte(),
            replacement: "_".to_owned(),
            safe: true,
        }),
    }
}

/// Walks back over the whitespace and then the comma that separated the argument from the one
/// before it, so deleting the argument does not leave `|a, |` behind.
fn removal_start(context: &RuleContext<'_>, start: usize) -> usize {
    let text = context.source.text().as_bytes();
    let mut cursor = start;
    while cursor > 0 && (text[cursor - 1] == b' ' || text[cursor - 1] == b'\t') {
        cursor -= 1;
    }
    if cursor > 0 && text[cursor - 1] == b',' {
        cursor -= 1;
    }
    cursor
}
