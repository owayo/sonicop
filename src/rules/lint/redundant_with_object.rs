use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, send_range};

use super::blocks::{BLOCK_KINDS, BlockArgs};
use super::locals::LocalVariables;

const MSG_EACH_WITH_OBJECT: &str = "Use `each` instead of `each_with_object`.";
const MSG_WITH_OBJECT: &str = "Remove redundant `with_object`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for call in context.nodes_of("call") {
        let Some(block) = call.child_by_field_name("block") else {
            continue;
        };
        if !BLOCK_KINDS.contains(&block.kind()) {
            continue;
        }
        let Some(selector) = call.child_by_field_name("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !matches!(name, "each_with_object" | "with_object") {
            continue;
        }
        // `(call _ {:each_with_object :with_object} _)`: exactly one argument, the object the
        // block would have taken as its second parameter.
        if arguments(call).len() != 1 {
            continue;
        }
        // `(args (arg _))`, a `numblock` of arity 1, or an `itblock`: the block never names the
        // object it is handed.
        let args = BlockArgs::of(block, context, &locals);
        let matched = match &args {
            BlockArgs::Numbered(arity) => *arity == 1,
            BlockArgs::It => true,
            BlockArgs::Written(_) => args.single_plain_arg(),
        };
        if !matched {
            continue;
        }
        let range = selector.start_byte()..send_range(call, context).end;
        let offense = if name == "each_with_object" {
            context
                .offense(MSG_EACH_WITH_OBJECT, range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: "each".to_owned(),
                    safe: true,
                })
        } else {
            let dot = call
                .child_by_field_name("operator")
                .map(|dot| Edit {
                    start: dot.start_byte(),
                    end: dot.end_byte(),
                    replacement: String::new(),
                    safe: true,
                })
                .into_iter();
            context
                .offense(MSG_WITH_OBJECT, range.clone())
                .corrected_by_all(
                    [Edit {
                        start: range.start,
                        end: range.end,
                        replacement: String::new(),
                        safe: true,
                    }]
                    .into_iter()
                    .chain(dot),
                )
        };
        offenses.push(offense);
    }
}
