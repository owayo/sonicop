use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, pair_key_symbol, top_level_constant};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
            continue;
        };
        if context.source.node_text(method) != "new"
            || !top_level_constant(receiver, "Hash", context)
        {
            continue;
        }
        // `$[hash !braces?]`: the keyword arguments upstream folds into one `hash`, which the
        // grammar leaves as the bare run of pairs a braced literal would not produce.
        let arguments = arguments(node);
        let [argument] = arguments.as_slice() else {
            continue;
        };
        let pairs = argument.parts();
        if !pairs
            .iter()
            .all(|part| matches!(part.kind_str(), "pair" | "hash_splat_argument"))
        {
            continue;
        }
        // `Hash.new(capacity: 8)` sizes the table rather than setting a default. `pairs.one?`
        // counts only the written pairs, so a `**splat` beside the keyword does not save it.
        let written: Vec<_> = pairs
            .iter()
            .filter(|part| part.kind_str() == "pair")
            .collect();
        if let [only] = written.as_slice()
            && pair_key_symbol(**only, context) == Some("capacity")
        {
            continue;
        }
        let range = argument.range();
        offenses.push(
            context
                .offense("Use a hash literal instead of keyword arguments.", range.clone())
                .corrected_by_all([
                    Edit {
                        start: range.start,
                        end: range.start,
                        replacement: "{".to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: range.end,
                        end: range.end,
                        replacement: "}".to_owned(),
                        safe: true,
                    },
                ]),
        );
    }
}
