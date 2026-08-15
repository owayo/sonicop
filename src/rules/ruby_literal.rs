//! Ruby's own reading and writing of a literal: what a string in the source stands for, and how
//! `String#inspect` and `Symbol#inspect` write a value back out.
//!
//! Three cops need this and none of them can settle for the raw source: an escape has to be
//! resolved before a value can be compared with one from the configuration, and written back the
//! way Ruby would have written it.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `Symbol#inspect`: quotes only go on a name that could not be written bare.
pub(crate) fn inspect_symbol(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_')
        && !name.starts_with(|character: char| character.is_ascii_digit());
    if plain {
        format!(":{name}")
    } else {
        format!(":{}", inspect_string(name))
    }
}

/// `String#inspect`.
pub(crate) fn inspect_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            '\u{1b}' => out.push_str("\\e"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The text a string literal stands for, with its escapes resolved. Single quotes resolve only the
/// two escapes they have, which is why the grammar leaves their contents in one piece.
pub(crate) fn string_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let raw = context.source.node_text(node);
    let verbatim = raw.starts_with('\'') || raw.starts_with("%q");
    let mut value = String::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind_str() {
            "string_content" if verbatim => {
                push_verbatim(context.source.node_text(child), &mut value);
            }
            "string_content" => value.push_str(context.source.node_text(child)),
            "escape_sequence" => unescape(context.source.node_text(child), &mut value),
            _ => {}
        }
    }
    value
}

/// One escape sequence, resolved. A caller that has to keep the parts of an interpolated string in
/// the order they were written cannot go through [`string_value`], which gathers the literal parts and
/// drops what sits between them.
pub(crate) fn unescape_text(text: &str) -> String {
    let mut value = String::new();
    unescape(text, &mut value);
    value
}

/// A single-quoted string resolves `\\` and `\'` and leaves every other backslash alone.
fn push_verbatim(text: &str, out: &mut String) {
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' && matches!(characters.peek(), Some('\\' | '\'')) {
            out.push(characters.next().expect("peeked"));
        } else {
            out.push(character);
        }
    }
}

/// The one character `?x` stands for. It takes the escapes a double-quoted string takes, and
/// upstream's parser has already resolved them by the time a cop sees the `str` node.
pub(crate) fn character_value(text: &str) -> String {
    let body = text.strip_prefix('?').unwrap_or(text);
    let mut value = String::new();
    match body.starts_with('\\') {
        true => unescape(body, &mut value),
        false => value.push_str(body),
    }
    value
}

fn unescape(escape: &str, out: &mut String) {
    let body = &escape[1..];
    let mut characters = body.chars();
    let Some(first) = characters.next() else {
        return;
    };
    match first {
        'n' => out.push('\n'),
        't' => out.push('\t'),
        'r' => out.push('\r'),
        's' => out.push(' '),
        'a' => out.push('\u{7}'),
        'b' => out.push('\u{8}'),
        'e' => out.push('\u{1b}'),
        'f' => out.push('\u{c}'),
        'v' => out.push('\u{b}'),
        '\n' => {}
        '0'..='7' => push_code_point(u32::from_str_radix(body, 8).ok(), out),
        'x' => push_code_point(u32::from_str_radix(characters.as_str(), 16).ok(), out),
        'u' => push_unicode(characters.as_str(), out),
        _ => out.push(first),
    }
}

fn push_unicode(body: &str, out: &mut String) {
    let Some(list) = body
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        push_code_point(u32::from_str_radix(body, 16).ok(), out);
        return;
    };
    for point in list.split_whitespace() {
        push_code_point(u32::from_str_radix(point, 16).ok(), out);
    }
}

fn push_code_point(code: Option<u32>, out: &mut String) {
    if let Some(character) = code.and_then(char::from_u32) {
        out.push(character);
    }
}
