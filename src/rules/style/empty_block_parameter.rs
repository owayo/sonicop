use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Omit pipes for the empty block parameters.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for block in context.nodes_of_any(&["block", "do_block"]) {
        // `-> () { }` belongs to `Style/EmptyLambdaParameter`; every other block is this cop's.
        if block
            .parent_of(context)
            .is_some_and(|parent| parent.kind_str() == "lambda")
        {
            continue;
        }
        let Some(parameters) = block.field("parameters") else {
            continue;
        };
        // `empty_and_without_delimiters?`: bars were written around nothing.
        if !super::nodes::children(parameters).is_empty() {
            continue;
        }
        let Some(begin) = block.child(0) else {
            continue;
        };
        offenses.push(
            context
                .offense(MSG, parameters.byte_range())
                .corrected_by(Edit {
                    start: begin.end_byte(),
                    end: parameters.end_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
