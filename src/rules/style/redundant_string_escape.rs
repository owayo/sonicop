//! `Style/RedundantStringEscape`: a backslash inside a string literal that escapes nothing.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::all_children_of;

/// One literal as `on_str` sees it: the text to scan, plus what the delimiters make of a backslash.
struct Literal {
    contents: Range<usize>,
    /// The ranges inside `contents` that belong to an interpolation, which is a literal of its own.
    interpolations: Vec<Range<usize>>,
    /// `interpolation_not_enabled?`: nothing in here is escaped at all.
    literal_text: bool,
    /// `node.heredoc?`.
    heredoc: bool,
    /// `percent_array_literal?`.
    percent_array: bool,
    /// `delimiter?`: the two characters the literal is written between.
    delimiters: Vec<char>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for literal in literals(context) {
        scan(context, &literal, offenses);
    }
}

/// Every literal `on_str` would be called for.
fn literals(context: &RuleContext<'_>) -> Vec<Literal> {
    let mut found = Vec::new();
    for node in context.nodes_of("string") {
        let opening = delimiter_text(node, context, 0);
        let closing = delimiter_text(node, context, 1);
        let percent_array = in_percent_array(node, context).is_some();
        let delimiters = match (percent_array, opening.is_empty()) {
            // A word inside a percent array is delimited by the array's brackets.
            (true, _) => array_delimiters(node, context),
            // A part of an interpolated string borrows the string's own delimiters.
            (_, true) => borrowed_delimiters(node, context),
            _ => vec![
                opening.chars().next_back().unwrap_or('"'),
                closing.chars().next().unwrap_or('"'),
            ],
        };
        found.push(Literal {
            contents: node.byte_range(),
            interpolations: interpolation_ranges(node),
            literal_text: delimiters.contains(&'\'') || opening.starts_with("%q"),
            heredoc: false,
            percent_array,
            delimiters,
        });
    }
    for array in context.nodes_of("string_array") {
        let upper = delimiter_text(array, context, 0).starts_with("%W");
        let delimiters = array_delimiters(array, context);
        for word in super::nodes::children_in(array, context) {
            if word.kind_str() != "bare_string" {
                continue;
            }
            found.push(Literal {
                contents: word.byte_range(),
                interpolations: interpolation_ranges(word),
                literal_text: !upper,
                heredoc: false,
                percent_array: true,
                delimiters: delimiters.clone(),
            });
        }
    }
    for (opener, body) in heredocs(context) {
        let Some(terminator) = super::nodes::children_in(body, context)
            .into_iter()
            .find(|child| child.kind_str() == "heredoc_end")
        else {
            continue;
        };
        found.push(Literal {
            contents: body.start_byte()..terminator.start_byte(),
            interpolations: interpolation_ranges(body),
            // `heredoc_with_disabled_interpolation?`: `<<~'TEXT'`.
            literal_text: context.source.node_text(opener).ends_with('\''),
            heredoc: true,
            percent_array: false,
            delimiters: Vec::new(),
        });
    }
    found
}

/// `each_match_range(str_contents_range, /(\\.)/)` and the offence each match may be.
fn scan(context: &RuleContext<'_>, literal: &Literal, offenses: &mut Vec<Offense>) {
    let source = context.source.text();
    let text = context.source.slice(literal.contents.clone());
    let mut characters = text.char_indices().peekable();
    while let Some((offset, character)) = characters.next() {
        if character != '\\' {
            continue;
        }
        let Some((_, escaped)) = characters.next() else {
            break;
        };
        let start = literal.contents.start + offset;
        if literal
            .interpolations
            .iter()
            .any(|range| range.start <= start && start < range.end)
        {
            continue;
        }
        if allowed_escape(source, literal, start, escaped) {
            continue;
        }
        let end = start + character.len_utf8() + escaped.len_utf8();
        offenses.push(
            context
                .offense(
                    format!("Redundant escape of {escaped} inside string literal."),
                    start..end,
                )
                .corrected_by(Edit {
                    start,
                    end: start + character.len_utf8(),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `allowed_escape?`.
fn allowed_escape(source: &str, literal: &Literal, start: usize, escaped: char) -> bool {
    literal.literal_text
        || escaped == '\n'
        || escaped == '\\'
        || escaped.is_alphanumeric()
        || (escaped == ' ' && (literal.percent_array || literal.heredoc))
        || disabling_interpolation(source, start)
        || (!literal.heredoc && literal.delimiters.contains(&escaped))
}

/// `disabling_interpolation?`: the backslash is what keeps `#{`, `#$` or `#@` from interpolating.
fn disabling_interpolation(source: &str, start: usize) -> bool {
    let after = &source[start..];
    let mut characters = after.chars();
    characters.next();
    // `/\A\\#[{$@]/`.
    if characters.next() == Some('#') && matches!(characters.next(), Some('{' | '$' | '@')) {
        return true;
    }
    // `/\A[^\\]#\\[{$@]/` two characters further left, and `'\#\{'` one character further right.
    let before: Vec<char> = source[..start].chars().rev().take(2).collect();
    let mut ahead = after.chars();
    ahead.next();
    if before.first() == Some(&'#')
        && before.get(1).is_some_and(|character| *character != '\\')
        && matches!(ahead.next(), Some('{' | '$' | '@'))
    {
        return true;
    }
    let mut ahead = after.chars();
    ahead.next();
    ahead.next() == Some('#')
        && ahead.next() == Some('\\')
        && matches!(ahead.next(), Some('{' | '$' | '@'))
}

/// The delimiter written at one end of a literal, which the grammar keeps as an unnamed token.
fn delimiter_text<'a>(node: Node<'_>, context: &'a RuleContext<'_>, end: usize) -> &'a str {
    let _cursor = node.walk();
    let children: Vec<Node<'_>> = all_children_of(node, context)
        .into_iter()
        .filter(|c| !c.is_named())
        .collect();
    let token = match end {
        0 => children.first(),
        _ => children.last(),
    };
    token.map_or("", |token| context.source.node_text(*token))
}

/// The brackets a percent array is written between.
fn array_delimiters(node: Node<'_>, context: &RuleContext<'_>) -> Vec<char> {
    let array = in_percent_array(node, context).unwrap_or(node);
    let opening = delimiter_text(array, context, 0);
    let closing = delimiter_text(array, context, 1);
    vec![
        opening.chars().next_back().unwrap_or('['),
        closing.chars().next().unwrap_or(']'),
    ]
}

/// `literal_in_interpolated_or_multiline_string?`: a part with no delimiters of its own takes the
/// ones the string around it was written with.
fn borrowed_delimiters(node: Node<'_>, context: &RuleContext<'_>) -> Vec<char> {
    match node.parent() {
        Some(parent) if matches!(parent.kind_str(), "string" | "chained_string") => {
            let opening = delimiter_text(parent, context, 0);
            let closing = delimiter_text(parent, context, 1);
            vec![
                opening.chars().next_back().unwrap_or('"'),
                closing.chars().next().unwrap_or('"'),
            ]
        }
        // `return true unless node.loc.begin`: with nothing to compare against, every character
        // counts as a delimiter.
        _ => Vec::new(),
    }
}

/// The `%w` or `%W` array the word belongs to.
fn in_percent_array<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    let _ = context;
    (parent.kind_str() == "string_array").then_some(parent)
}

/// The ranges an interpolation covers, which are literals of their own.
fn interpolation_ranges(node: Node<'_>) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut stack: Vec<Node<'_>> = Vec::new();
    crate::rules::push_named_children(node, &mut stack);
    while let Some(child) = stack.pop() {
        if child.kind_str() == "interpolation" {
            found.push(child.byte_range());
            continue;
        }
        crate::rules::push_named_children(child, &mut stack);
    }
    found
}

/// Every heredoc of the file, as its opener paired with its body.
fn heredocs<'ctx, 'tree>(context: &'ctx RuleContext<'tree>) -> Vec<(Node<'ctx>, Node<'ctx>)> {
    let openers: Vec<Node<'ctx>> = context.nodes_of("heredoc_beginning").collect();
    context
        .nodes_of("heredoc_body")
        .enumerate()
        .filter_map(|(index, body)| openers.get(index).map(|opener| (*opener, body)))
        .collect()
}
