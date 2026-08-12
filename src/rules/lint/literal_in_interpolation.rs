use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Literal interpolation detected.";

/// The literals that print as themselves without being made of other nodes (`BASIC_LITERALS`).
const BASIC_LITERALS: &[&str] = &[
    "string",
    "integer",
    "float",
    "simple_symbol",
    "delimited_symbol",
    "hash_key_symbol",
    "bare_symbol",
    "true",
    "false",
    "nil",
    "complex",
    "rational",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for interpolation in context.nodes_of("interpolation") {
        let Some(literal) = last_statement(interpolation) else {
            continue;
        };
        if !offending(literal, interpolation, context) {
            continue;
        }
        let expanded = regexp_slashes(interpolation, value(literal, context), context);
        // `%W[]` and `%I[]` split their contents into words before the interpolation is expanded,
        // so a value holding a space would become more than the one word it stands for.
        if in_array_percent_literal(interpolation)
            && (expanded.is_empty() || expanded.chars().any(char::is_whitespace))
        {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, literal.byte_range())
                .corrected_by(Edit {
                    start: interpolation.start_byte(),
                    end: interpolation.end_byte(),
                    replacement: expanded,
                    safe: true,
                }),
        );
    }
}

/// The value of the interpolation: the last thing written in it, since the ones before it are
/// evaluated and thrown away.
fn last_statement<'tree>(interpolation: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = interpolation.walk();
    interpolation
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body" | "empty_statement"))
        .last()
}

fn offending(literal: Node<'_>, interpolation: Node<'_>, context: &RuleContext<'_>) -> bool {
    prints_as_self(literal)
        && !(space_literal(literal, context) && ends_heredoc_line(literal, context))
        // `Lint/ArrayLiteralInRegexp` has this one.
        && !(literal.kind() == "array"
            && interpolation
                .parent()
                .is_some_and(|parent| parent.kind() == "regex"))
}

/// `prints_as_self?`: a literal whose source is what it would print, or one built only out of
/// those. A literal holding an interpolation is a `dstr` or a `dsym` upstream and prints as
/// whatever the interpolation evaluates to, so it is neither.
fn prints_as_self(node: Node<'_>) -> bool {
    match node.kind() {
        // A sign written against a numeric literal is part of the literal upstream.
        "unary" => {
            matches!(
                node.child_by_field_name("operator").map(|op| op.kind()),
                Some("-" | "+")
            ) && node
                .child_by_field_name("operand")
                .is_some_and(|operand| numeric(operand.kind()))
        }
        "array" | "hash" | "pair" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .all(prints_as_self)
        }
        // A range missing an end is not one upstream reads as a literal at all.
        "range" => {
            node.child_by_field_name("begin").is_some()
                && node.child_by_field_name("end").is_some()
                && ["begin", "end"].iter().all(|field| {
                    node.child_by_field_name(field)
                        .is_some_and(prints_as_self)
                })
        }
        "string" | "delimited_symbol" | "bare_symbol" => {
            !interpolated(node) && BASIC_LITERALS.contains(&node.kind())
        }
        // The words of a `%w[]` are plain strings, and the whole literal is an array.
        "string_array" | "symbol_array" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .all(|child| !interpolated(child))
        }
        kind => BASIC_LITERALS.contains(&kind),
    }
}

fn numeric(kind: &str) -> bool {
    matches!(kind, "integer" | "float" | "complex" | "rational")
}

fn interpolated(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
}

/// A string of nothing but spaces, written where the interpolation is the last thing on a heredoc
/// line: removing it would leave trailing whitespace that another cop then reports.
fn space_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind() == "string" && {
        let text = context.source.node_text(node);
        string_value(node, context).trim().is_empty() && !text.is_empty()
    }
}

fn ends_heredoc_line(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !context.in_heredoc(node.byte_range()) {
        return false;
    }
    let (line, _) = context.source.line_column(node.end_byte());
    let text = context.source.line(line).trim_end_matches('\n');
    let (_, column) = context.source.line_column(node.end_byte());
    text.chars().count() == column
}

/// `in_array_percent_literal?`: the interpolation is one word of a `%W[]` or `%I[]`.
fn in_array_percent_literal(interpolation: Node<'_>) -> bool {
    interpolation
        .parent()
        .filter(|parent| matches!(parent.kind(), "bare_string" | "bare_symbol"))
        .and_then(|parent| parent.parent())
        .is_some_and(|array| matches!(array.kind(), "string_array" | "symbol_array"))
}

/// A `/` in a slash-delimited regexp has to keep the number of backslashes it compiles to, which
/// is not the number written: 0-2 compile to 1, 3-6 to 3, and so on.
fn regexp_slashes(interpolation: Node<'_>, value: String, context: &RuleContext<'_>) -> String {
    let slash_literal = interpolation
        .parent()
        .filter(|parent| parent.kind() == "regex")
        .is_some_and(|regex| context.source.node_text(regex).starts_with('/'));
    if !slash_literal || !value.contains('/') {
        return value;
    }
    let mut out = String::with_capacity(value.len());
    let mut backslashes = 0usize;
    for character in value.chars() {
        match character {
            '\\' => backslashes += 1,
            '/' => {
                out.truncate(out.len() - backslashes);
                for _ in 0..(2 * ((backslashes + 1) / 4)) + 1 {
                    out.push('\\');
                }
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
        if character != '/' {
            out.push(character);
        } else {
            out.push('/');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The value a literal expands to
// ---------------------------------------------------------------------------

fn value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    match node.kind() {
        "integer" => integer_value(context.source.node_text(node)),
        "float" => float_value(context.source.node_text(node)),
        "unary" => signed_numeric(node, context),
        "string" => escape_string_content(&string_value(node, context)),
        "simple_symbol" | "delimited_symbol" => symbol_content(node, context).replace('"', "\\\""),
        "array" | "string_array" | "symbol_array" => array_value(node, context),
        "hash" => hash_value(node, context),
        "nil" => String::new(),
        _ => context.source.node_text(node).replace('"', "\\\""),
    }
}

fn signed_numeric(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let Some(operand) = node.child_by_field_name("operand") else {
        return context.source.node_text(node).replace('"', "\\\"");
    };
    let negative = node
        .child_by_field_name("operator")
        .is_some_and(|operator| operator.kind() == "-");
    let magnitude = value(operand, context);
    match (negative, matches!(operand.kind(), "integer" | "float")) {
        (true, true) => format!("-{magnitude}"),
        (_, true) => magnitude,
        _ => context.source.node_text(node).replace('"', "\\\""),
    }
}

/// `node.children.last.to_i.to_s`: the number the literal names, written in base ten.
fn integer_value(text: &str) -> String {
    let digits: String = text.chars().filter(|&character| character != '_').collect();
    let (radix, body) = match digits.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (16, &digits[2..]),
        Some("0b") => (2, &digits[2..]),
        Some("0o") => (8, &digits[2..]),
        Some("0d") => (10, &digits[2..]),
        _ if digits.len() > 1 && digits.starts_with('0') => (8, &digits[1..]),
        _ => (10, digits.as_str()),
    };
    i128::from_str_radix(body, radix).map_or_else(|_| text.to_owned(), |value| value.to_string())
}

/// `to_f.to_s`: Ruby always writes a decimal point, and switches to an exponent for the magnitudes
/// where writing every digit would be absurd.
fn float_value(text: &str) -> String {
    let digits: String = text.chars().filter(|&character| character != '_').collect();
    let Ok(number) = digits.parse::<f64>() else {
        return text.to_owned();
    };
    if !number.is_finite() {
        return format!("{number}");
    }
    let magnitude = number.abs();
    if number != 0.0 && !(1e-4..1e16).contains(&magnitude) {
        let exponent = magnitude.log10().floor() as i32;
        let mantissa = number / 10f64.powi(exponent);
        let mut written = format!("{mantissa}");
        if !written.contains('.') {
            written.push_str(".0");
        }
        return format!(
            "{written}e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        );
    }
    let mut written = format!("{number}");
    if !written.contains('.') && !written.contains('e') {
        written.push_str(".0");
    }
    written
}

/// The text between a symbol's delimiters, which is what upstream takes for the value.
fn symbol_content<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    let text = context.source.node_text(node);
    match node.kind() {
        "simple_symbol" => text.strip_prefix(':').unwrap_or(text),
        _ => {
            let opener = node.child(0).map_or(0, |open| open.end_byte());
            let closer = node
                .child(node.child_count().saturating_sub(1) as u32)
                .map_or(node.end_byte(), |close| close.start_byte());
            &context.source.text()[opener..closer.max(opener)]
        }
    }
}

fn array_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let text = context.source.node_text(node);
    if !text.starts_with('%') {
        return text.replace('"', "\\\"");
    }
    let opener = node
        .child(0)
        .map_or(node.start_byte(), |open| open.end_byte());
    let closer = node
        .child(node.child_count().saturating_sub(1) as u32)
        .map_or(node.end_byte(), |close| close.start_byte());
    let words: Vec<String> = context.source.text()[opener..closer.max(opener)]
        .split_whitespace()
        .map(inspect_string)
        .collect();
    format!("[{}]", words.join(", ")).replace('"', "\\\"")
}

fn hash_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let mut cursor = node.walk();
    let pairs: Vec<String> = node
        .named_children(&mut cursor)
        .map(|pair| {
            let key = pair
                .child_by_field_name("key")
                .map_or_else(String::new, |key| value_in_hash(key, context));
            let held = pair
                .child_by_field_name("value")
                .map_or_else(String::new, |held| value_in_hash(held, context));
            format!("{key}=>{held}")
        })
        .collect();
    format!("{{{}}}", pairs.join(", "))
}

/// Inside a hash, a string or a symbol prints as `inspect` writes it rather than as its bare text,
/// because that is what `Hash#to_s` does with them.
fn value_in_hash(node: Node<'_>, context: &RuleContext<'_>) -> String {
    match node.kind() {
        "integer" => integer_value(context.source.node_text(node)),
        "float" => float_value(context.source.node_text(node)),
        "unary" => signed_numeric(node, context),
        "string" => escape_string_content(&inspect_string(&string_value(node, context))),
        "simple_symbol" | "delimited_symbol" | "hash_key_symbol" => {
            escape_string_content(&inspect_symbol(&symbol_name(node, context)))
        }
        "array" | "string_array" | "symbol_array" => array_value(node, context),
        "hash" => hash_value(node, context),
        _ => context.source.node_text(node).replace('"', "\\\""),
    }
}

fn symbol_name(node: Node<'_>, context: &RuleContext<'_>) -> String {
    match node.kind() {
        "hash_key_symbol" => context.source.node_text(node).to_owned(),
        "delimited_symbol" => string_value(node, context),
        _ => symbol_content(node, context).to_owned(),
    }
}

/// `Symbol#inspect`: quotes only go on a name that could not be written bare.
fn inspect_symbol(name: &str) -> String {
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
fn inspect_string(value: &str) -> String {
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

/// `escape_string_content`: what has to be written twice for the value to survive being put back
/// inside the double-quoted string the interpolation was in.
fn escape_string_content(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' | '"' => {
                out.push('\\');
                out.push(character);
            }
            '#' if matches!(characters.peek(), Some('@' | '{' | '$')) => {
                out.push_str("\\#");
            }
            other => out.push(other),
        }
    }
    out
}

/// The text a string literal stands for, with its escapes resolved. Single quotes resolve only the
/// two escapes they have, which is why the grammar leaves their contents in one piece.
fn string_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let raw = context.source.node_text(node);
    let verbatim = raw.starts_with('\'') || raw.starts_with("%q");
    let mut value = String::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
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
