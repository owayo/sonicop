use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

const MSG: &str = "Ternary operators must not be nested. Prefer `if` or `else` constructs instead.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut ignored: Vec<Range<usize>> = Vec::new();
    let mut reported: HashSet<(usize, usize)> = HashSet::new();

    for node in context.nodes_of("conditional") {
        let nested: Vec<Node<'_>> = super::nodes::children_in(node, context)
            .into_iter()
            .flat_map(|node| super::conditional::descendants(node, context))
            .filter(|descendant| descendant.kind_str() == "conditional")
            .collect();
        for descendant in nested {
            // `add_offense` keeps one offense per range, so a ternary reached from two enclosing
            // ones is reported once.
            if !reported.insert((descendant.start_byte(), descendant.end_byte())) {
                continue;
            }
            let offense = context.offense(MSG, descendant.byte_range());
            // `part_of_ignored_node?`: the rewrite of an enclosing ternary already covers this
            // text, so the offense is reported without one of its own.
            let covered = ignored
                .iter()
                .any(|range| range.start <= node.start_byte() && range.end >= node.end_byte());
            if covered {
                offenses.push(offense);
                continue;
            }
            match autocorrect(context, node) {
                Some(edits) => {
                    offenses.push(
                        offense
                            .corrected_by_all(edits)
                            .corrections_anchored_at(node.byte_range()),
                    );
                    ignored.push(node.byte_range());
                }
                None => offenses.push(offense),
            }
        }
    }
}

/// The `if`/`else` the outer ternary becomes. Every call the corrector makes is kept apart so that
/// what another cop edits inside the branches survives.
fn autocorrect(context: &RuleContext<'_>, node: Node<'_>) -> Option<Vec<Edit>> {
    let question = super::conditional::token(node, &["?"])?.byte_range();
    let consequence = node.field("consequence")?;
    let colon = super::conditional::token(node, &[":"])?.byte_range();
    let branch = context.source.node_text(consequence);
    Some(vec![
        Edit {
            start: node.start_byte(),
            end: node.start_byte(),
            replacement: "if ".to_owned(),
            safe: true,
        },
        Edit {
            start: node.end_byte(),
            end: node.end_byte(),
            replacement: "\nend".to_owned(),
            safe: true,
        },
        replace_with_surrounding_space(context, question, "\n"),
        replace_with_surrounding_space(context, colon, "\nelse\n"),
        Edit {
            start: consequence.start_byte(),
            end: consequence.end_byte(),
            replacement: without_parentheses(branch).to_owned(),
            safe: true,
        },
    ])
}

/// `range_with_surrounding_space(range: range, whitespace: true)`.
fn replace_with_surrounding_space(
    context: &RuleContext<'_>,
    range: Range<usize>,
    replacement: &str,
) -> Edit {
    // `range_with_surrounding_space(range: range, whitespace: true)`. The fourth stage is Ruby's
    // `\s`, which takes the vertical tab that `is_ascii_whitespace` left out.
    let taken = support::range_with_surrounding_space(
        range,
        context.source.text(),
        support::Side::Both,
        false,
        true,
        true,
    );
    Edit {
        start: taken.start,
        end: taken.end,
        replacement: replacement.to_owned(),
        safe: true,
    }
}

fn without_parentheses(source: &str) -> &str {
    match source.starts_with('(') && source.ends_with(')') {
        true => &source[1..source.len() - 1],
        false => source,
    }
}
