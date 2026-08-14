use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::variable_force::AssignmentKind;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for variable in &context.variable_analysis().variables {
        let Some(number) = numbered_parameter_name(&variable.name) else {
            continue;
        };
        for assignment in &variable.assignments {
            // The locals `/(?<_1>x)/ =~ y` declares are no `lvasgn` upstream: the parser leaves
            // them inside the `match_with_lvasgn` and the handler never sees them.
            if assignment.kind != AssignmentKind::Plain {
                continue;
            }
            // `NUMBERED_PARAMETER_RANGE`: only `_1` through `_9` are names Ruby itself takes, so
            // anything else is merely close enough to be confusing.
            let reserved = matches!(number.as_bytes(), [b'1'..=b'9']);
            let template = if reserved {
                "is reserved for numbered parameter"
            } else {
                "is similar to numbered parameter"
            };
            let message = format!("`_{number}` {template}; consider another name.");
            offenses.push(context.offense(message, assignment.node.byte_range()));
        }
    }
}

/// The digits of a name matching `/\A_(\d+)\z/`, as `to_i` would print them back: the message
/// names the number, so `_007` is reported as `_7`.
fn numbered_parameter_name(name: &str) -> Option<String> {
    let digits = name.strip_prefix('_')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    Some(if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    })
}
