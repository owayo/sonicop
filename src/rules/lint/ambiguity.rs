//! The two diagnostics the parser's lexer raises for an argument that begins with an operator.
//!
//! Upstream reads `processed_source.diagnostics` and reports what the lexer already decided:
//! reaching a `/`, `*`, `**`, `&`, `+` or `-` while it expects the first argument of a command
//! call, with a space in front of it and none behind. Nothing here parses that state -- the tree
//! records it, since only a lexer in that state produces an argument list written without
//! parentheses, and the operator is then the first character of its first argument.
//!
//! Two things the tree records as an argument list are nevertheless never lexed in that state, so
//! neither warning reaches them: an argument list belonging to a keyword that is not a call, and a
//! `->` that opens a lambda literal rather than a unary minus.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Edit;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

/// One ambiguity the lexer would have warned about.
pub(super) struct Ambiguity<'tree> {
    /// The operator itself, which is what the diagnostic points at.
    pub operator: Range<usize>,
    /// The call, `yield` or `super` whose arguments the correction parenthesizes.
    pub owner: Node<'tree>,
    /// The list those arguments were written in.
    arguments: Node<'tree>,
}

/// Keywords whose arguments the lexer never reads from `expr_arg`.
///
/// `return`, `break` and `next` leave the lexer in `expr_mid`, where an operator opens a literal
/// with nothing to guess at, and `redo` and `retry` take no arguments at all. `yield` and `super`
/// are absent on purpose: to the lexer they are ordinary command calls and they do warn.
const KEYWORDS_WITHOUT_ARGUMENTS: &[&str] = &["break", "next", "redo", "retry", "return"];

/// Every argument list written without parentheses whose first argument opens with `prefixes`.
pub(super) fn scan<'tree>(
    context: &'tree RuleContext<'_>,
    prefixes: &[&str],
) -> Vec<Ambiguity<'tree>> {
    let mut found = Vec::new();
    for list in context.nodes_of("argument_list") {
        // A list written with parentheses leaves the lexer nothing to guess at.
        if list
            .child(0)
            .is_some_and(|first| context.source.node_text(first) == "(")
        {
            continue;
        }
        let Some(owner) = list.parent() else {
            continue;
        };
        if KEYWORDS_WITHOUT_ARGUMENTS.contains(&owner.kind_str()) {
            continue;
        }
        let Some(first) = named_children(list).into_iter().next() else {
            continue;
        };
        let text = context.source.node_text(first);
        let Some(prefix) = prefixes
            .iter()
            .find(|prefix| text.starts_with(**prefix))
            .copied()
        else {
            continue;
        };
        let start = first.start_byte();
        let bytes = context.source.text().as_bytes();
        // `->` is matched as a lambda literal before the `-` can be read as a prefix.
        if prefix == "-" && bytes.get(start + 1) == Some(&b'>') {
            continue;
        }
        // A space in front and none behind is what makes the operator ambiguous.
        if start == 0 || !matches!(bytes[start - 1], b' ' | b'\t') {
            continue;
        }
        let after = start + prefix.len();
        if bytes
            .get(after)
            .is_none_or(|byte| byte.is_ascii_whitespace())
        {
            continue;
        }
        found.push(Ambiguity {
            operator: start..after,
            owner,
            arguments: list,
        });
    }
    found
}

impl Ambiguity<'_> {
    /// `add_parentheses`: the space that opened the argument list becomes the `(`.
    pub(super) fn parenthesize(&self, context: &RuleContext<'_>) -> Vec<Edit> {
        let opening = self.arguments.start_byte() - 1;
        let closing = self.owner.end_byte();
        let _ = context;
        vec![
            Edit {
                start: opening,
                end: opening + 1,
                replacement: "(".to_owned(),
                safe: true,
            },
            Edit {
                start: closing,
                end: closing,
                replacement: ")".to_owned(),
                safe: true,
            },
        ]
    }
}
