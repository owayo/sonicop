use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::element_line_breaks::{ARRAYS, children_line_break, elements, line_of};

const MSG: &str = "Add a line break before the first element of a multi-line array.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_implicit: bool = context
        .setting("AllowImplicitArrayLiterals")
        .unwrap_or(false);
    let ignore_last: bool = context.setting("AllowMultilineFinalElement").unwrap_or(false);
    for node in context.nodes_of_any(ARRAYS) {
        // `node.loc.begin`: only the bracketless list has none, and then the array is checked only
        // when it stands on the right of an assignment.
        let delimited = !matches!(node.kind_str(), "right_assignment_list" | "exceptions");
        if !delimited && !assignment_on_same_line(node, context) {
            continue;
        }
        // `node.bracketed?` is `square_brackets? || percent_literal?`: a `%w[]` is bracketed too.
        if allow_implicit && !delimited {
            continue;
        }
        let children = elements(node, context);
        if children.is_empty() {
            continue;
        }
        offenses.extend(children_line_break(
            context, node, &children, ignore_last, MSG,
        ));
    }
}

/// `assignment_on_same_line?`: the text in front of the list closes with the `=` that assigned it.
fn assignment_on_same_line(node: tree_sitter::Node<'_>, context: &RuleContext<'_>) -> bool {
    let line = context.source.line(line_of(node.start_byte(), context));
    let column = context.source.line_column(node.start_byte()).1 - 1;
    let prefix: String = line.chars().take(column).collect();
    prefix.trim_end().ends_with('=')
}
