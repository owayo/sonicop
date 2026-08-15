use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::regexp_source;

const MSG: &str = "Duplicate element inside regexp character class";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("regex") {
        let Some(pattern) = regexp_source::parse(node, context) else {
            continue;
        };
        for index in pattern.tree.expressions() {
            let expression = &pattern.tree.nodes[index];
            // `[a&&b]` holds two sets of its own, and a member of one is no duplicate of a member
            // of the other.
            if expression.kind != "set" || expression.token == "intersection" {
                continue;
            }
            let mut seen: Vec<&str> = Vec::new();
            for &child in &expression.children {
                let member = &pattern.tree.nodes[child];
                let range = pattern.range(member.ts..member.te);
                // An interpolation was blanked to spaces, and every space after the first would
                // otherwise be read as a duplicate of the one before it.
                if pattern
                    .interpolations
                    .iter()
                    .any(|hole| hole.start < range.end && range.start < hole.end)
                {
                    continue;
                }
                let source = context.source.slice(range.clone());
                if seen.contains(&source) {
                    offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
                        start: range.start,
                        end: range.end,
                        replacement: String::new(),
                        safe: true,
                    }));
                }
                seen.push(source);
            }
        }
    }
}
