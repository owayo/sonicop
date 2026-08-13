use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;
use crate::rules::support::final_pos;

const MSG: &str = "Avoid leaving a trailing comma in attribute declarations.";

const METHODS: [&str; 4] = ["attr_reader", "attr_writer", "attr_accessor", "attr"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        // `attribute_accessor?` insists on no receiver; a lone `def` argument carries no comma.
        if !METHODS.contains(&context.source.node_text(method))
            || node.child_by_field_name("receiver").is_some()
        {
            continue;
        }
        let arguments = arguments(node);
        if arguments.len() < 2 {
            continue;
        }
        let last = arguments[arguments.len() - 1].first();
        if !matches!(last.kind(), "method" | "singleton_method") {
            continue;
        }
        // `range_with_surrounding_space(arguments[-2], side: :right).end.resize(1)`: the one
        // character standing after the declaration's last name, which is the comma that swallowed
        // the definition written on the next line. The default `whitespace: false` stops the walk
        // at the first line break, so blanks that only start after one are not reached.
        let comma = final_pos(
            context.source.text(),
            arguments[arguments.len() - 2].range().end,
            true,
            true,
            false,
        );
        offenses.push(context.offense(MSG, comma..comma + 1).corrected_by(Edit {
            start: comma,
            end: comma + 1,
            replacement: String::new(),
            safe: true,
        }));
    }
}
