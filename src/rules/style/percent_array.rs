//! The shared half of `Style/SymbolArray` and `Style/WordArray`, which upstream keeps in
//! `mixin/percent_array.rb`, `mixin/array_min_size.rb` and `correctors/percent_literal_corrector.rb`.

use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::literal::{Decoded, Quoting, decode, escape_string, needs_escaping};
use crate::rules::node_ext::NodeExt;

/// One array literal written with brackets, ready to be judged against the percent form.
pub(super) struct Bracketed<'tree> {
    pub node: Node<'tree>,
    pub items: Vec<Node<'tree>>,
}

/// One word of a percent array, found by scanning rather than by reading the tree.
///
/// The grammar merges a word into the one before it when a backslash follows the blank between
/// them, so `%w[{42} \xff]` reaches a cop as a single element there. Ruby splits on every blank
/// that is not itself escaped, and that is what the cops count.
pub(super) struct Element {
    pub range: std::ops::Range<usize>,
    pub interpolated: bool,
}

pub(super) fn elements(context: &RuleContext<'_>, node: Node<'_>) -> Vec<Element> {
    let Some(begin) = node.child(0) else {
        return Vec::new();
    };
    let Some(close) = node.child(node.child_count().saturating_sub(1) as u32) else {
        return Vec::new();
    };
    if begin.id() == close.id() {
        return Vec::new();
    }
    let interpolations = interpolation_ranges(node);
    let text = context.source.text().as_bytes();
    let (start, end) = (begin.end_byte(), close.start_byte());

    let mut found = Vec::new();
    let mut offset = start;
    while offset < end {
        if crate::rules::support::separates_words(text[offset]) {
            offset += 1;
            continue;
        }
        let word = offset;
        let mut interpolated = false;
        while offset < end {
            if let Some(span) = interpolations.iter().find(|span| span.start == offset) {
                interpolated = true;
                offset = span.end;
                continue;
            }
            if text[offset] == b'\\' {
                offset = next_boundary(context, offset + 1, end);
                continue;
            }
            if crate::rules::support::separates_words(text[offset]) {
                break;
            }
            offset = next_boundary(context, offset, end);
        }
        found.push(Element {
            range: word..offset,
            interpolated,
        });
    }
    found
}

fn next_boundary(context: &RuleContext<'_>, offset: usize, end: usize) -> usize {
    let text = context.source.text();
    let mut next = (offset + 1).min(end);
    while next < end && !text.is_char_boundary(next) {
        next += 1;
    }
    next
}

fn interpolation_ranges(node: Node<'_>) -> Vec<std::ops::Range<usize>> {
    let mut found = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "interpolation" && current.id() != node.id() {
            found.push(current.byte_range());
            continue;
        }
        let mut cursor = current.walk();
        stack.extend(current.named_children(&mut cursor));
    }
    found
}

/// `allowed_bracket_array?`: the literal cannot take the percent form.
pub(super) fn allowed_bracket_array(context: &RuleContext<'_>, array: &Bracketed<'_>) -> bool {
    let min_size: usize = context.setting("MinSize").unwrap_or(0);
    comments_in_array(context, array.node)
        || array.items.len() < min_size
        || invalid_percent_array_context(array.node)
}

/// `comments_in_array?`: a comment anywhere but the literal's last line, which the percent form
/// would have nowhere to put.
///
/// The comment spans are in source order, so the run covering the literal is found by bisection
/// rather than by reading every comment in the file once per literal.
fn comments_in_array(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let comments = context.comment_ranges();
    let first = context.source.line_start(node.start_position().row + 1);
    let last = context.source.line_start(node.end_position().row + 1);
    let start = comments.partition_point(|comment| comment.start < first);
    comments
        .get(start)
        .is_some_and(|comment| comment.start < last)
}

/// `invalid_percent_array_context?`: Ruby cannot parse a percent array where a block would follow.
fn invalid_percent_array_context(node: Node<'_>) -> bool {
    let Some(list) = node.parent() else {
        return false;
    };
    if list.kind_str() != "argument_list"
        || list.child(0).map(|child| child.kind_str()) == Some("(")
    {
        return false;
    }
    list.parent()
        .is_some_and(|call| call.kind_str() == "call" && call.field("block").is_some())
}

/// `PercentLiteralCorrector#correct`: the bracketed literal written out as a percent one.
pub(super) fn percent_replacement(
    context: &RuleContext<'_>,
    array: &Bracketed<'_>,
    prefix: char,
    values: &[String],
) -> String {
    let escape = values.iter().any(|value| needs_escaping(value));
    let letter = match escape {
        true => prefix.to_ascii_uppercase(),
        false => prefix,
    };
    let percent_type = format!("%{letter}");
    let delimiters = preferred_delimiters(context, &percent_type);
    let contents: Vec<String> = values
        .iter()
        .map(|value| fix_escaped_content(value, escape, delimiters))
        .collect();

    let text = context.source.text();
    let source = &text[array.node.byte_range()];
    let body = match array.node.start_position().row == array.node.end_position().row {
        true => contents.join(" "),
        false => multiline_body(context, array, &contents, source),
    };
    format!("{percent_type}{}{body}{}", delimiters.0, delimiters.1)
}

/// `autocorrect_multiline_words`: each word keeps the line and indentation it was written on.
fn multiline_body(
    context: &RuleContext<'_>,
    array: &Bracketed<'_>,
    contents: &[String],
    source: &str,
) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let base_line = array.node.start_position().row + 1;
    let mut previous_line = base_line;
    let mut out = String::new();
    for (index, item) in array.items.iter().enumerate() {
        let first_line = item.start_position().row + 1;
        if first_line == previous_line {
            if index > 0 || first_line != base_line {
                out.push(' ');
            }
        } else {
            let begin = previous_line - base_line + 1;
            let end = first_line - base_line + 1;
            let joined = lines[begin.min(lines.len())..end.min(lines.len())].join("\n");
            let item_source = context.source.node_text(*item);
            let leading = joined
                .split_once(item_source)
                .map_or(joined.as_str(), |(before, _)| before);
            out.push('\n');
            out.push_str(leading);
        }
        previous_line = item.end_position().row + 1;
        out.push_str(&contents[index]);
    }
    // `end_content`: a closing bracket standing on its own line keeps that line.
    if let Some(last) = lines.last() {
        let indent: String = last.chars().take_while(|c| c.is_whitespace()).collect();
        if last[indent.len()..].starts_with(']') {
            out.push('\n');
            out.push_str(&indent);
        }
    }
    out
}

/// `fix_escaped_content`: one word's content, escaped as the chosen literal form needs.
fn fix_escaped_content(value: &str, escape: bool, delimiters: (char, char)) -> String {
    let content = match escape {
        true => escape_string(value),
        false => value.to_owned(),
    };
    substitute_escaped_delimiters(&content, delimiters)
}

/// `substitute_escaped_delimiters`: a bracketing pair used the same number of times each needs no
/// escaping, because the literal stays balanced.
fn substitute_escaped_delimiters(content: &str, delimiters: (char, char)) -> String {
    let (opening, closing) = delimiters;
    if opening != closing && content.matches(opening).count() == content.matches(closing).count() {
        return content.to_owned();
    }
    let mut out = String::with_capacity(content.len());
    for character in content.chars() {
        if character == opening || character == closing {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

pub(super) fn preferred_delimiters(context: &RuleContext<'_>, percent_type: &str) -> (char, char) {
    let configured: HashMap<String, String> = context
        .setting_of("Style/PercentLiteralDelimiters", "PreferredDelimiters")
        .unwrap_or_default();
    let value = configured
        .get(percent_type)
        .or_else(|| configured.get("default"))
        .map_or("[]", String::as_str);
    let mut characters = value.chars();
    (
        characters.next().unwrap_or('['),
        characters.next().unwrap_or(']'),
    )
}

/// The values of a percent array's words, decoded the way its opener escapes them.
pub(super) fn percent_values(
    context: &RuleContext<'_>,
    node: Node<'_>,
    elements: &[Element],
) -> Vec<Decoded> {
    let text = context.source.text();
    let Some(begin) = node.child(0) else {
        return Vec::new();
    };
    let opener = context.source.node_text(begin);
    let closing = node
        .child(node.child_count().saturating_sub(1) as u32)
        .and_then(|close| context.source.node_text(close).chars().next())
        .unwrap_or(']');
    let quoting = match opener.chars().nth(1) {
        Some('W' | 'I') => Quoting::Double,
        _ => Quoting::Word,
    };
    let delimiters = [opener.chars().next_back().unwrap_or('['), closing];
    elements
        .iter()
        .map(|element| decode(&text[element.range.clone()], quoting, &delimiters))
        .collect()
}

/// `build_bracketed_array_with_appropriate_whitespace`: the percent literal written back with
/// brackets, keeping the blanks it was laid out with.
pub(super) fn bracketed_replacement(
    context: &RuleContext<'_>,
    node: Node<'_>,
    items: &[Element],
    elements: &[String],
) -> String {
    if items.is_empty() {
        return "[]".to_owned();
    }
    let text = context.source.text();
    let begin = node
        .child(0)
        .map_or(node.start_byte(), |child| child.end_byte());
    let end = node
        .child(node.child_count().saturating_sub(1) as u32)
        .map_or(node.end_byte(), |child| child.start_byte());
    let leading = &text[begin..items[0].range.start];
    let between = match items.len() >= 2 {
        true => &text[items[0].range.end..items[1].range.start],
        false => " ",
    };
    let trailing = &text[items[items.len() - 1].range.end..end];
    format!(
        "[{leading}{}{trailing}]",
        elements.join(&format!(",{between}"))
    )
}

/// `check_percent_array`'s report: the message names the literal it would write instead.
pub(super) fn percent_array_offense(
    context: &RuleContext<'_>,
    node: Node<'_>,
    template: &str,
    bracketed: String,
) -> Offense {
    let prefer = match bracketed.contains('\n') {
        true => "an array literal `[...]`".to_owned(),
        false => format!("`{bracketed}`"),
    };
    context
        .offense(template.replace("%<prefer>s", &prefer), node.byte_range())
        .corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: bracketed,
            safe: true,
        })
}
