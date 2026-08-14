//! `Layout/FirstArrayElementLineBreak`.

use super::element_line_breaks::{check_children_line_break, literal_elements};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Add a line break before the first element of a multi-line array.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_implicit = context
        .setting::<bool>("AllowImplicitArrayLiterals")
        .unwrap_or(false);
    let ignore_last = context
        .setting::<bool>("AllowMultilineFinalElement")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["array", "string_array", "symbol_array", "right_assignment_list"])
    {
        // `node.loc.begin`: an array written without brackets only counts when an assignment
        // opened it on the same line.
        let bracketed = node.kind_str() != "right_assignment_list";
        if !bracketed && !assignment_on_same_line(context, node.start_byte()) {
            continue;
        }
        // `node.bracketed?` is `square_brackets? || percent_literal?`, so only an array written
        // without any opening at all is the implicit one.
        if allow_implicit && !bracketed {
            continue;
        }
        let children = literal_elements(node);
        check_children_line_break(
            context,
            MSG,
            node.start_byte(),
            &children,
            ignore_last,
            offenses,
        );
    }
}

/// `assignment_on_same_line?`: what stands before the array on its line ends with `=`.
fn assignment_on_same_line(context: &RuleContext<'_>, start: usize) -> bool {
    let line = context.source.line_column(start).0;
    let column = context.source.line_column(start).1 - 1;
    let prefix: String = context.source.line(line).chars().take(column).collect();
    prefix.trim_end().ends_with('=')
}
