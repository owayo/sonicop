//! `Layout/ClosingHeredocIndentation`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

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
        if !matches!(
            text[opener.byte_range()].as_bytes().get(2),
            Some(b'~' | b'-')
        ) {
            continue;
        }
        let mut cursor = body.walk();
        let Some(delimiter) = body
            .named_children(&mut cursor)
            .find(|child| child.kind() == "heredoc_end")
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
        let argument = argument_send(opener);
        if let Some(send) = argument.or_else(|| chained_send(opener)) {
            let outermost = outermost_send(send);
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
    text.strip_suffix('\n').unwrap_or(text)
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

/// `node.argument?`: the heredoc is one of the arguments of the call it was written in.
fn argument_send<'tree>(opener: Node<'tree>) -> Option<Node<'tree>> {
    opener
        .parent()
        .filter(|parent| parent.kind() == "argument_list")
        .and_then(|list| list.parent())
        .filter(|call| call.kind() == "call")
}

/// `node.chained?`: the heredoc is the receiver of the call it was written in.
fn chained_send<'tree>(opener: Node<'tree>) -> Option<Node<'tree>> {
    opener
        .parent()
        .filter(|parent| parent.child_by_field_name("receiver") == Some(opener))
        .filter(|parent| parent.kind() == "call")
}

/// `find_node_used_heredoc_argument`: the outermost call the heredoc's own call is nested in.
///
/// A call written as another call's argument is that call's direct child upstream, so the argument
/// list the grammar puts between them is stepped over rather than ending the walk.
fn outermost_send<'tree>(send: Node<'tree>) -> Node<'tree> {
    let mut current = send;
    loop {
        let parent = current
            .parent()
            .and_then(|parent| match parent.kind() {
                "argument_list" => parent.parent(),
                _ => Some(parent),
            })
            .filter(|parent| parent.kind() == "call");
        match parent {
            Some(parent) => current = parent,
            None => return current,
        }
    }
}
