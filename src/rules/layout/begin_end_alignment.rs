//! `Layout/BeginEndAlignment`.

use super::support::{character_column, end_keyword, end_keyword_alignment, start_line_range};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // Unlike `Layout/EndAlignment`, this cop measures from the start of the line by default:
    // `||= begin` puts the keyword where nothing else lines up with it.
    let align_with_begin = context
        .setting::<String>("EnforcedStyleAlignWith")
        .as_deref()
        == Some("begin");
    for node in context.nodes_of("begin") {
        let (Some(keyword), Some(end)) = (
            node.child(0).filter(|child| child.kind() == "begin"),
            end_keyword(node),
        ) else {
            continue;
        };
        let base = match align_with_begin {
            true => keyword.byte_range(),
            false => start_line_range(context, node.start_byte()),
        };
        let column = match align_with_begin {
            true => character_column(context, node.start_byte()),
            false => character_column(context, start_line_range(context, node.start_byte()).start),
        };
        if let Some(offense) = end_keyword_alignment(context, end.byte_range(), base, column) {
            offenses.push(offense);
        }
    }
}
