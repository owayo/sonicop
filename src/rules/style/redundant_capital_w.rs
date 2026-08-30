//! `Style/RedundantCapitalW`: `%W` only earns its capital when something is interpolated.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_iter;

const MSG: &str = "Do not use `%W` unless interpolation is needed. If not, use `%w`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("string_array") {
        let Some(literal) = super::percent::PercentLiteral::new(node, context) else {
            continue;
        };
        if literal.percent_type != "%W" {
            continue;
        }
        // `requires_interpolation?`: an element that interpolates, or one whose text could not be
        // written between single quotes.
        let _cursor = node.walk();
        let interpolates = named_children_iter(node, context).any(|element| {
            holds_interpolation(element)
                || super::literal::double_quotes_required(context.source.node_text(element))
        });
        if interpolates {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by(Edit {
                    start: literal.begin.start,
                    end: literal.begin.end,
                    replacement: context
                        .source
                        .slice(literal.begin.clone())
                        .replace('W', "w"),
                    safe: true,
                })
                .corrections_anchored_at(literal.begin),
        );
    }
}

fn holds_interpolation(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.kind_str() == "interpolation" || node.named_children(&mut cursor).any(holds_interpolation)
}
