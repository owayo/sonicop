use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Redundant `.rb` file extension detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if node.child_by_field_name("receiver").is_some() {
            continue;
        }
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if !matches!(
            context.source.node_text(method),
            "require" | "require_relative"
        ) {
            continue;
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            continue;
        };
        let [name] = super::nodes::children(arguments)[..] else {
            continue;
        };
        // `$str_type?`: an interpolated or multi-line literal is a `dstr` upstream.
        if name.kind() != "string"
            || name.start_position().row != name.end_position().row
            || super::nodes::children(name)
                .iter()
                .any(|child| child.kind() == "interpolation")
        {
            continue;
        }
        let Some(value) = super::literal::node_value(context, name) else {
            continue;
        };
        if !value.value.ends_with(".rb") {
            continue;
        }
        // `range_between(end_pos - 4, end_pos - 1)`: the three characters before the closing
        // delimiter, whatever the literal actually holds there.
        let end = name.end_byte();
        if end < name.start_byte() + 4 {
            continue;
        }
        let range = end - 4..end - 1;
        // A backslash right before the extension means the path is escaped.
        if context.source.text()[name.start_byte()..range.start].ends_with('\\') {
            continue;
        }
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}
