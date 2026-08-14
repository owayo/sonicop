use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::blocks::{BLOCK_KINDS, BlockArgs};
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// The three selectors and what each collapses to once its block does nothing.
const REPLACEMENTS: [(&str, &str); 3] = [
    ("max_by", "max"),
    ("min_by", "min"),
    ("minmax_by", "minmax"),
];

/// `max_by { |x| x }`, which sorts by the element itself and so is just `max`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let original = context.source.node_text(selector);
        let Some((_, replacement)) = REPLACEMENTS
            .iter()
            .find(|(current, _)| *current == original)
        else {
            continue;
        };
        let Some(block) = node
            .field("block")
            .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
        else {
            continue;
        };
        let Some(body) = sole_statement(block) else {
            continue;
        };
        if body.kind_str() != "identifier" {
            continue;
        }
        let read = context.source.node_text(body);
        // `(args (arg $_x)) (lvar _x)` and its numbered and `it` spellings.
        let written = match BlockArgs::of(block, context, &locals) {
            BlockArgs::Written(params) => match params.as_slice() {
                [only] if only.kind_str() == "identifier" => {
                    let name = context.source.node_text(*only);
                    if name != read {
                        continue;
                    }
                    format!("{{ |{name}| {name} }}")
                }
                _ => continue,
            },
            BlockArgs::Numbered(1) if read == "_1" => "{ _1 }".to_owned(),
            BlockArgs::It if read == "it" => "{ it }".to_owned(),
            _ => continue,
        };
        // `range_between(send.loc.selector.begin_pos, node.loc.end.end_pos)`.
        let range = selector.start_byte()..block.end_byte();
        offenses.push(
            context
                .offense(
                    format!("Use `{replacement}` instead of `{original} {written}`."),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: (*replacement).to_owned(),
                    safe: true,
                }),
        );
    }
}

/// The block's body when it holds exactly one statement.
fn sole_statement<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    let body = block.field("body")?;
    let statements: Vec<Node<'tree>> = super::nodes::children(body)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    match statements.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}
