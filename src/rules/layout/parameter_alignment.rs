//! `Layout/ParameterAlignment`.

use super::support::{AlignmentPass, definition_parameters, display_column, line_indentation};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const ALIGN_PARAMS_MSG: &str =
    "Align the parameters of a method definition if they span more than one line.";
const FIXED_INDENT_MSG: &str = "Use one level of indentation for parameters following the first \
                                line of a multi-line method definition.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "with_first_parameter".to_owned());
    let fixed = style == "with_fixed_indentation";
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let message = if fixed {
        FIXED_INDENT_MSG
    } else {
        ALIGN_PARAMS_MSG
    };

    let mut pass = AlignmentPass::new();
    for definition in context.nodes_of_any(&["method", "singleton_method"]) {
        let items = definition_parameters(definition);
        if items.len() < 2 {
            continue;
        }
        // `target_method_lineno` is the `def` keyword's line, which is where the definition starts.
        let base = if fixed {
            line_indentation(context, definition.start_byte()) + width
        } else {
            display_column(context, items[0].start)
        };
        for (item, delta) in AlignmentPass::misaligned(context, &items, base) {
            pass.register(
                context,
                item.clone(),
                item,
                delta,
                |_| message.to_owned(),
                offenses,
            );
        }
    }
}
