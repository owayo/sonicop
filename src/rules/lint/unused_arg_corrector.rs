//! `UnusedArgCorrector`: the one correction `Lint/UnusedBlockArgument` and
//! `Lint/UnusedMethodArgument` both hand to their offenses.
//!
//! Upstream is a corrector class the two cops call into, rather than a correction written twice, so
//! an argument the one renames is renamed the same way by the other.

use crate::diagnostic::Edit;
use crate::rules::RuleContext;

use super::variable_force::{Argument, Declaration, Variable};

/// RuboCop's `UnusedArgCorrector` leaves keyword arguments alone -- prefixing one would rename the
/// keyword itself -- and deletes an explicit block argument instead of renaming it, since an
/// unused `&block` is simply surplus.
pub(super) fn correction(context: &RuleContext<'_>, variable: &Variable<'_>) -> Option<Edit> {
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
        // `corrector.replace(node.loc.name, "_#{name}")`. Writing the whole name rather than
        // inserting a `_` in front of it matters when another cop is rewriting the same argument
        // in the same pass: a replacement of the same range clobbers and is deferred to the next
        // pass, while an insertion at its edge slips out of the range and lands on the neighbour.
        _ => {
            let name = context.source.node_text(variable.name_node);
            Some(Edit {
                start: variable.name_node.start_byte(),
                end: variable.name_node.end_byte(),
                replacement: format!("_{name}"),
                safe: true,
            })
        }
    }
}

/// Walks back over the whitespace and then the comma that separated the argument from the one
/// before it, so deleting the argument does not leave `|a, |` behind.
pub(super) fn removal_start(context: &RuleContext<'_>, start: usize) -> usize {
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
