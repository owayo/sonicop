//! `Layout/SpaceBeforeSemicolon`.

use tree_sitter::Node;

use super::support::whitespace_before;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    // `space_required_after?`: a block's `{` is a `tLCURLY`, and the block-brace cop asks for the
    // space this one would otherwise take out. A hash literal's `{` is a `tLBRACE` and is not
    // exempt.
    let block_braces: Vec<usize> = block_brace_offsets(context);
    let keep_space_after_lcurly = context
        .setting_of::<String>("Layout/SpaceInsideBlockBraces", "EnforcedStyle")
        .as_deref()
        .unwrap_or("space")
        == "space";

    for node in context.nodes() {
        // Upstream walks the lexer's tokens, so a semicolon inside a string or a heredoc body is
        // not one. Only a semicolon the parser recognised is a node here.
        if node.kind() != ";" {
            continue;
        }
        let space = whitespace_before(text, node.start_byte());
        if space.is_empty() || space.start == 0 {
            continue;
        }
        // `same_line?(token1, token2)`: a semicolon opening its line has no token before it there.
        let previous = text.as_bytes()[space.start - 1];
        if previous == b'\n' || previous == b'\r' {
            continue;
        }
        if keep_space_after_lcurly && block_braces.contains(&(space.start - 1)) {
            continue;
        }
        offenses.push(
            context
                .offense("Space found before semicolon.", space.clone())
                .corrected_by(Edit {
                    start: space.start,
                    end: space.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// Where the `{` of a block or a lambda body was written, which is what upstream's lexer calls a
/// `tLCURLY` or a `tLAMBEG`.
fn block_brace_offsets(context: &RuleContext<'_>) -> Vec<usize> {
    context
        .nodes_of("block")
        .filter_map(|node| opening_brace(node))
        .collect()
}

fn opening_brace(node: Node<'_>) -> Option<usize> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == "{")
        .map(|child| child.start_byte())
}
