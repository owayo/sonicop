use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `STRING_INTERPOLATION_OR_ESCAPED_CHARACTER_PATTERN = /#(\{|@|\$)|\\/`: what a single-quoted
/// delimiter is actually protecting the body from.
fn body_needs_quotes(body: &str) -> bool {
    let bytes = body.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| match byte {
        b'\\' => true,
        b'#' => matches!(bytes.get(index + 1), Some(b'{' | b'@' | b'$')),
        _ => false,
    })
}

/// `on_heredoc`: the opening delimiter of every heredoc. Upstream reaches it through the `str` node,
/// whose source is exactly that opener; here the opener is a `heredoc_beginning` and the body hangs
/// off a sibling `heredoc_body`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("heredoc_beginning") {
        let source = context.source.node_text(node);
        let Some((heredoc_type, delimiter)) = opening_parts(source) else {
            continue;
        };
        if needs_quotes(node, source, heredoc_type, context) {
            continue;
        }
        let replacement = format!("{heredoc_type}{delimiter}");
        offenses.push(
            context
                .offense(
                    format!(
                        "Remove the redundant heredoc delimiter quotes, use `{replacement}` instead."
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `need_heredoc_delimiter_quotes?`.
fn needs_quotes(
    node: Node<'_>,
    source: &str,
    heredoc_type: &str,
    context: &RuleContext<'_>,
) -> bool {
    // `node.source.delete(heredoc_type(node))` deletes *characters*, not the substring, which is
    // what leaves `'FOO'` behind for `<<~'FOO'`.
    let stripped: String = source
        .chars()
        .filter(|character| !heredoc_type.contains(*character))
        .collect();
    let quote = stripped.chars().next();
    if quote != Some('\'') && quote != Some('"') {
        return true;
    }
    let Some(body) = send_node::heredoc_body(node, context) else {
        return true;
    };
    let children = super::nodes::children_in(body, context);
    let Some(end) = children
        .last()
        .filter(|last| last.kind_str() == "heredoc_end")
    else {
        return true;
    };
    // A delimiter that is not a plain word cannot lose its quotes.
    if context
        .source
        .node_text(*end)
        .trim()
        .chars()
        .any(|character| !character.is_alphanumeric() && character != '_')
    {
        return true;
    }
    if quote != Some('\'') {
        return false;
    }
    let content = children
        .iter()
        .take_while(|child| child.kind_str() != "heredoc_end")
        .fold(None::<std::ops::Range<usize>>, |span, child| {
            Some(match span {
                Some(range) => range.start..child.end_byte(),
                None => child.byte_range(),
            })
        });
    content.is_some_and(|range| body_needs_quotes(&context.source.text()[range]))
}

/// `OPENING_DELIMITER = /(<<[~-]?)['"`]?([^'"`]+)['"`]?/` applied to the opener: the squiggly or
/// dash marker, and the delimiter with its quotes taken off.
fn opening_parts(source: &str) -> Option<(&str, &str)> {
    let rest = source.strip_prefix("<<")?;
    let marker = match rest.as_bytes().first() {
        Some(b'~' | b'-') => 3,
        _ => 2,
    };
    let (heredoc_type, tail) = source.split_at(marker);
    let tail = tail.strip_prefix(['\'', '"', '`']).unwrap_or(tail);
    let delimiter = tail
        .find(['\'', '"', '`'])
        .map_or(tail, |offset| &tail[..offset]);
    if delimiter.is_empty() {
        return None;
    }
    Some((heredoc_type, delimiter))
}
