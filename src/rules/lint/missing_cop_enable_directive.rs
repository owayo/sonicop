use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::cop_directives::{Mode, directives, is_department};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `MaxRangeSize` is `.inf` by default, which makes every bounded range acceptable and leaves
    // only the disables that run to the end of the file.
    if context
        .setting::<serde_yaml_ng::Value>("MaxRangeSize")
        .is_some_and(|value| value.as_f64().is_some_and(f64::is_finite))
    {
        return;
    }
    let parsed = directives(context);
    // `# rubocop:disable all` turns this cop off too, so its own offense never survives.
    let mut open: Vec<usize> = Vec::new();
    for (index, directive) in parsed.iter().enumerate() {
        match directive.mode {
            Mode::Disable if !directive.all => open.push(index),
            Mode::Disable => {}
            Mode::Enable => {
                if directive.all {
                    open.clear();
                    continue;
                }
                open.retain(|&disabled| {
                    !parsed[disabled].names.iter().any(|name| {
                        directive.names.iter().any(|enabled| {
                            enabled == name
                                || (is_department(enabled) && name.starts_with(enabled.as_str()))
                                || (is_department(name) && enabled.starts_with(name.as_str()))
                        })
                    })
                });
            }
        }
    }
    for index in open {
        let directive = &parsed[index];
        // `message` names the first cop of the range, which for a department is the department.
        let Some(name) = directive.names.first() else {
            continue;
        };
        let kind = if is_department(name) { "department" } else { "cop" };
        offenses.push(context.offense(
            format!("Re-enable {name} {kind} with `# rubocop:enable` after disabling it."),
            directive.comment.clone(),
        ));
    }
}
