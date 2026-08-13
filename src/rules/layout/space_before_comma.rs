//! `Layout/SpaceBeforeComma`.

use super::support::whitespace_before;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    // `space_required_after?`: a block's `{` is a `tLCURLY`, and the block-brace cop asks for the
    // space this one would otherwise take out.
    let keep_space_after_lcurly = context
        .setting_of::<String>("Layout/SpaceInsideBlockBraces", "EnforcedStyle")
        .as_deref()
        .unwrap_or("space")
        == "space";
    let block_braces: Vec<usize> = context
        .nodes_of("block")
        .filter_map(|node| {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|child| child.kind_str() == "{")
                .map(|child| child.start_byte())
        })
        .collect();

    for node in context.nodes() {
        // Upstream walks the lexer's tokens, so a comma inside a string is not one.
        if node.kind_str() != "," {
            continue;
        }
        let space = whitespace_before(text, node.start_byte());
        if space.is_empty() || space.start == 0 {
            continue;
        }
        // `same_line?(token1, token2)`: a comma opening its line has no token before it there.
        if matches!(text.as_bytes()[space.start - 1], b'\n' | b'\r') {
            continue;
        }
        if keep_space_after_lcurly && block_braces.contains(&(space.start - 1)) {
            continue;
        }
        offenses.push(
            context
                .offense("Space found before comma.", space.clone())
                .corrected_by(Edit {
                    start: space.start,
                    end: space.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}
