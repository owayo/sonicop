//! `Layout/HeredocIndentation`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `minimum_target_ruby_version 2.3`: `<<~` is what the cop asks for, and it did not exist
    // before then.
    if context.target_ruby_version() < RubyVersion::new(2, 3) {
        return;
    }
    let width = indentation_width(context);
    let text = context.source.text();
    for (opener, body, terminator) in heredocs(context) {
        let body_source = &text[body.clone()];
        if body_source.trim().is_empty() {
            continue;
        }
        let body_indent = indent_level(body_source);
        let base_indent = indent_level(
            context
                .source
                .line(context.source.line_column(opener.start_byte()).0),
        );
        let opener_source = &text[opener.byte_range()];
        let indent_type = match opener_source.as_bytes().get(2) {
            Some(b'~') => Some('~'),
            Some(b'-') => Some('-'),
            _ => None,
        };
        let squish = heredoc_squish(context, opener);
        if indent_type == Some('~') {
            if base_indent + width == body_indent {
                continue;
            }
        } else if body_indent != 0 && !squish {
            continue;
        }
        if line_too_long(context, body_source, base_indent + width, body_indent) {
            continue;
        }
        let message = match indent_type {
            Some('~') => format!("Use {width} spaces for indentation in a heredoc."),
            _ => format!(
                "Use {width} spaces for indentation in a heredoc by using `<<~` instead of `<<{}`.",
                indent_type.map(String::from).unwrap_or_default()
            ),
        };
        let mut edits = Vec::new();
        if indent_type == Some('~') || squish {
            edits.push(Edit {
                start: body.start,
                end: body.end,
                replacement: indented_body(body_source, body_indent, base_indent + width),
                safe: true,
            });
            edits.push(Edit {
                start: terminator.start,
                end: terminator.end,
                replacement: indented_end(&text[terminator.clone()], base_indent),
                safe: true,
            });
        }
        if indent_type != Some('~') {
            edits.push(Edit {
                start: opener.start_byte(),
                end: opener.end_byte(),
                replacement: squiggly(opener_source),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(message, body.clone())
                .corrected_by_all(edits),
        );
    }
}

/// Every heredoc as upstream's parser locates it: the opener, `loc.heredoc_body` and
/// `loc.heredoc_end`.
///
/// The grammar draws those two differently -- its body reaches from the opener's own line and stops
/// at the terminator *word*, leaving the terminator's indentation on the body's side -- so both are
/// redrawn to whole lines here.
fn heredocs<'ctx, 'tree>(
    context: &'ctx RuleContext<'tree>,
) -> Vec<(Node<'tree>, Range<usize>, Range<usize>)> {
    let openers: Vec<Node<'tree>> = context.nodes_of("heredoc_beginning").collect();
    if openers.is_empty() {
        return Vec::new();
    }
    context
        .nodes_of("heredoc_body")
        .enumerate()
        .filter_map(|(index, node)| {
            let opener = *openers.get(index)?;
            let mut cursor = node.walk();
            let end = node
                .named_children(&mut cursor)
                .find(|child| child.kind_str() == "heredoc_end")?;
            let first_body_line = context.source.line_column(node.start_byte()).0 + 1;
            let terminator_line = context.source.line_column(end.start_byte()).0;
            let body = context.source.line_start(first_body_line)
                ..context.source.line_start(terminator_line);
            let terminator = context.source.line_start(terminator_line)..end.end_byte();
            Some((opener, body, terminator))
        })
        .collect()
}

/// `Heredoc#indent_level`: the narrowest indentation of the lines that hold something, and zero
/// when none of them do.
fn indent_level(source: &str) -> usize {
    source
        .split_inclusive('\n')
        .filter_map(|line| {
            let indent = &line[..line.len() - line.trim_start().len()];
            // A line of nothing but blanks has its own line break inside its indentation, which is
            // what upstream rejects it by.
            (!indent.ends_with('\n')).then(|| indent.chars().count())
        })
        .min()
        .unwrap_or(0)
}

/// `indented_body`: every line's first `body_indent` blanks become `correct` spaces. With nothing
/// indented, the empty match at each line start puts the whole indentation in.
fn indented_body(body: &str, body_indent: usize, correct: usize) -> String {
    let replacement = " ".repeat(correct);
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        let mut taken = 0;
        let mut cursor = line.char_indices();
        while taken < body_indent {
            match cursor.next() {
                Some((_, character))
                    if character.is_whitespace() && character != '\n' && character != '\r' =>
                {
                    taken += 1;
                }
                _ => break,
            }
        }
        match taken == body_indent {
            true => {
                let consumed: usize = line.chars().take(taken).map(char::len_utf8).sum();
                out.push_str(&replacement);
                out.push_str(&line[consumed..]);
            }
            false => out.push_str(line),
        }
    }
    out
}

/// `indented_end`: the terminator is pulled out to the opener's own indentation, and left alone
/// when it already reaches past it.
fn indented_end(terminator: &str, correct: usize) -> String {
    let indent = indent_level(terminator);
    match indent < correct {
        true => {
            let consumed: usize = terminator.chars().take(indent).map(char::len_utf8).sum();
            format!("{}{}", " ".repeat(correct), &terminator[consumed..])
        }
        false => terminator.to_owned(),
    }
}

/// `adjust_minus`: `source.sub(/<<-?/, '<<~')`.
fn squiggly(opener: &str) -> String {
    match opener.starts_with("<<-") {
        true => format!("<<~{}", &opener[3..]),
        false => format!("<<~{}", &opener[2..]),
    }
}

/// `line_too_long?`: re-indenting must not push a line past `Layout/LineLength`, which by default
/// exempts heredocs outright.
fn line_too_long(
    context: &RuleContext<'_>,
    body: &str,
    expected_indent: usize,
    actual_indent: usize,
) -> bool {
    // `max_line_length` answers with nothing when the length cop is off, and `AllowHeredoc` is on
    // by default, so this whole test normally waves the heredoc through.
    if !context
        .setting_of::<bool>("Layout/LineLength", "Enabled")
        .unwrap_or(true)
        || context
            .setting_of::<bool>("Layout/LineLength", "AllowHeredoc")
            .unwrap_or(true)
    {
        return false;
    }
    let max = context
        .setting_of::<i64>("Layout/LineLength", "Max")
        .unwrap_or(120);
    let longest = body
        .lines()
        .map(|line| line.trim_end_matches('\r').chars().count() as i64)
        .max()
        .unwrap_or(0);
    longest + expected_indent as i64 - actual_indent as i64 >= max
}

/// `heredoc_squish?`: with the Rails extensions enabled, `<<-FOO.squish` is written to be flattened
/// anyway, so the cop rewrites it as well.
fn heredoc_squish(context: &RuleContext<'_>, opener: Node<'_>) -> bool {
    if !context
        .setting_of::<bool>("AllCops", "ActiveSupportExtensionsEnabled")
        .unwrap_or(false)
    {
        return false;
    }
    opener.parent().is_some_and(|parent| {
        parent.kind_str() == "call"
            && parent.field("receiver") == Some(opener)
            && parent
                .field("method")
                .map(|method| &context.source.text()[method.byte_range()])
                .is_some_and(|name| name == "squish" || name == "squish!")
    })
}

fn indentation_width(context: &RuleContext<'_>) -> usize {
    context
        .setting::<usize>("IndentationWidth")
        .or_else(|| context.setting_of::<usize>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2)
}
