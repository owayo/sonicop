use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `on_dstr`: the only `dstr` written as one literal per quote is the run of adjacent literals
    // the grammar keeps as a `chained_string`.
    for node in context.nodes_of("chained_string") {
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
        let mut empty: Vec<Node<'_>> = children
            .iter()
            .copied()
            .filter(|child| child.kind_str() == "string" && child.named_child_count() == 0)
            .collect();
        if empty.is_empty() {
            continue;
        }
        // `scan(/(?<=\A)['"]*/)`: the run of quotes the literal opens with.
        let text = context.source.node_text(node);
        if text
            .bytes()
            .take_while(|byte| matches!(byte, b'\'' | b'"'))
            .count()
            < 3
        {
            continue;
        }
        // A literal made of nothing but empty strings still has to keep one of them.
        if empty.len() == children.len() {
            empty.remove(0);
        }
        offenses.push(
            context
                .offense(
                    "Delimiting a string with multiple quotes has no effect, use a single quote \
                     instead.",
                    node.byte_range(),
                )
                .corrected_by_all(empty.into_iter().map(|child| Edit {
                    start: child.start_byte(),
                    end: child.end_byte(),
                    replacement: String::new(),
                    safe: true,
                })),
        );
    }
}
