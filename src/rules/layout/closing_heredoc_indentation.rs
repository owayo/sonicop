//! `Layout/ClosingHeredocIndentation`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::is_send_like;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let openers: Vec<Node<'_>> = context.nodes_of("heredoc_beginning").collect();
    if openers.is_empty() {
        return;
    }
    let text = context.source.text();
    // The opener and the body sit far apart in the tree but both appear in source order, so the
    // nth of one belongs to the nth of the other.
    for (index, body) in context.nodes_of("heredoc_body").enumerate() {
        let Some(opener) = openers.get(index).copied() else {
            break;
        };
        // `SIMPLE_HEREDOC`: a `<<EOS` terminator has to sit at column 0 whatever the opener does.
        // An opener whose delimiter is empty matches nothing at all and so is not simple either.
        if heredoc_type(&text[opener.byte_range()]) == Some("<<") {
            continue;
        }
        let mut cursor = body.walk();
        let Some(delimiter) = body
            .named_children(&mut cursor)
            .find(|child| child.kind_str() == "heredoc_end")
        else {
            continue;
        };

        let opening_line = source_line(context, opener.start_byte());
        let closing_line = source_line(context, delimiter.start_byte());
        let opening_indent = indent_level(opening_line);
        let closing_indent = indent_level(closing_line);
        if opening_indent == closing_indent {
            continue;
        }
        let argument = argument_send(context, opener);
        if let Some(send) = argument.or_else(|| chained_send(context, opener)) {
            let outermost = outermost_send(context, send);
            if indent_level(source_line(context, outermost.start_byte())) == closing_indent {
                continue;
            }
        }

        // `loc.heredoc_end` covers the whole terminator line, from column 0 to the delimiter.
        let line = context.source.line_column(delimiter.start_byte()).0;
        let range = context.source.line_start(line)..delimiter.end_byte();
        let (closing, opening) = (closing_line.trim(), opening_line.trim());
        let message = match argument {
            Some(_) => format!(
                "`{closing}` is not aligned with `{opening}` or beginning of method definition."
            ),
            None => format!("`{closing}` is not aligned with `{opening}`."),
        };
        offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: reindented(closing_line, closing_indent, opening_indent),
            safe: true,
        }));
    }
}

/// The physical line an offset sits on, without its line break.
fn source_line<'a>(context: &'a RuleContext<'_>, offset: usize) -> &'a str {
    let line = context.source.line_column(offset).0;
    let text = context.source.line(line);
    crate::rules::support::chomp(text)
}

/// `indent_level`: `source_line[/\A */].length`, which counts spaces only.
fn indent_level(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

/// `closing_text.gsub(/^\s{closing_indent}/, ' ' * opening_indent)` over a single line. The first
/// `closing_indent` characters are the spaces `indent_level` counted, so the match is that prefix.
fn reindented(line: &str, closing_indent: usize, opening_indent: usize) -> String {
    format!("{}{}", " ".repeat(opening_indent), &line[closing_indent..])
}

/// `Heredoc#heredoc_type`: the first capture of `/(<<[~-]?)['"`]?([^'"`]+)['"`]?/` over the
/// opener. An opener written with an empty delimiter, `<<""`, matches nowhere and has no type.
fn heredoc_type(source: &str) -> Option<&str> {
    let bytes = source.as_bytes();
    for start in 0..bytes.len() {
        if !source[start..].starts_with("<<") {
            continue;
        }
        let mut cursor = start + 2;
        if matches!(bytes.get(cursor), Some(b'~' | b'-')) {
            cursor += 1;
        }
        let prefix = &source[start..cursor];
        // The quote is optional, so a delimiter is looked for both behind it and in its place.
        for probe in [cursor + usize::from(is_quote(bytes.get(cursor))), cursor] {
            if bytes.get(probe).is_some_and(|byte| !is_quote(Some(byte))) {
                return Some(prefix);
            }
        }
    }
    None
}

fn is_quote(byte: Option<&u8>) -> bool {
    matches!(byte, Some(b'\'' | b'"' | b'`'))
}

/// `node.argument?`: the heredoc is one of the arguments of the send it was written in. An
/// attribute or index write is a send too, so its right-hand side counts.
fn argument_send<'tree>(
    context: &RuleContext<'_>,
    opener: Node<'tree>,
) -> Option<Node<'tree>> {
    let parent = opener.parent()?;
    match parent.kind_str() {
        "argument_list" => parent.parent().filter(|call| call.kind_str() == "call"),
        "assignment" => (parent.field("right") == Some(opener)
            && parent
                .field("left")
                .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference")))
        .then_some(parent),
        _ if is_send_like(context, parent) => {
            (receiver_of(parent) != Some(opener)).then_some(parent)
        }
        _ => None,
    }
}

/// The node upstream calls the send's receiver, whichever shape the grammar gave the call.
fn receiver_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "element_reference" => node.child(0),
        "binary" => node.field("left"),
        "unary" => node.field("operand"),
        _ => node.field("receiver"),
    }
}

/// `node.chained?`: the heredoc is the receiver of the send it was written in.
fn chained_send<'tree>(
    context: &RuleContext<'_>,
    opener: Node<'tree>,
) -> Option<Node<'tree>> {
    let parent = opener.parent()?;
    if !is_send_like(context, parent) {
        return None;
    }
    (receiver_of(parent) == Some(opener)).then_some(parent)
}

/// `find_node_used_heredoc_argument`: the outermost send the heredoc's own send is nested in.
///
/// A call written as another call's argument is that call's direct child upstream, so the argument
/// list the grammar puts between them is stepped over rather than ending the walk.
fn outermost_send<'tree>(context: &RuleContext<'_>, send: Node<'tree>) -> Node<'tree> {
    let mut current = send;
    loop {
        let parent = current
            .parent()
            .and_then(|parent| match parent.kind_str() {
                "argument_list" => parent.parent(),
                _ => Some(parent),
            })
            .filter(|parent| is_send_like(context, *parent) || is_setter(*parent));
        match parent {
            Some(parent) => current = parent,
            None => return current,
        }
    }
}

/// An assignment the parser files under `send`, because its target is an attribute or an index.
fn is_setter(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference"))
}
