use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal::{character_value, inspect_string, inspect_symbol, string_value};

const MSG: &str = "Literal interpolation detected.";

/// The literals that print as themselves without being made of other nodes (`BASIC_LITERALS`).
const BASIC_LITERALS: &[&str] = &[
    "string",
    // `?x` is a `str` upstream, indistinguishable from the one-character string it names.
    "character",
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
        .filter(|child| {
            !matches!(
                child.kind_str(),
                "comment" | "heredoc_body" | "empty_statement"
            )
        })
        .last()
}

fn offending(literal: Node<'_>, interpolation: Node<'_>, context: &RuleContext<'_>) -> bool {
    prints_as_self(literal)
        && !(space_literal(literal, context) && ends_heredoc_line(literal, context))
        // `Lint/ArrayLiteralInRegexp` has this one. `%w[]` and `%i[]` are an `array` upstream just
        // as `[]` is, so `array_type?` covers all three and the grammar's three kinds have to be
        // named here.
        && !(matches!(literal.kind_str(), "array" | "string_array" | "symbol_array")
            && interpolation
                .parent_of(context)
                .is_some_and(|parent| parent.kind_str() == "regex"))
}

/// `prints_as_self?`: a literal whose source is what it would print, or one built only out of
/// those. A literal holding an interpolation is a `dstr` or a `dsym` upstream and prints as
/// whatever the interpolation evaluates to, so it is neither.
fn prints_as_self(node: Node<'_>) -> bool {
    match node.kind_str() {
        // A sign written against a numeric literal is part of the literal upstream.
        "unary" => {
            matches!(
                node.field("operator").map(|op| op.kind_str()),
                Some("-" | "+")
            ) && node
                .field("operand")
                .is_some_and(|operand| numeric(operand.kind_str()))
        }
        "array" | "hash" | "pair" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).all(prints_as_self)
        }
        // A range missing an end is not one upstream reads as a literal at all.
        "range" => {
            node.field("begin").is_some()
                && node.field("end").is_some()
                && ["begin", "end"]
                    .iter()
                    .all(|field| node.field(field).is_some_and(prints_as_self))
        }
        "string" | "delimited_symbol" | "bare_symbol" => {
            !interpolated(node) && BASIC_LITERALS.contains(&node.kind_str())
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
        .any(|child| child.kind_str() == "interpolation")
}

/// A string of nothing but spaces, written where the interpolation is the last thing on a heredoc
/// line: removing it would leave trailing whitespace that another cop then reports.
fn space_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "string" && {
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
        .filter(|parent| matches!(parent.kind_str(), "bare_string" | "bare_symbol"))
        .and_then(|parent| parent.parent())
        .is_some_and(|array| matches!(array.kind_str(), "string_array" | "symbol_array"))
}

/// A `/` in a slash-delimited regexp has to keep the number of backslashes it compiles to, which
/// is not the number written: 0-2 compile to 1, 3-6 to 3, and so on.
fn regexp_slashes(interpolation: Node<'_>, value: String, context: &RuleContext<'_>) -> String {
    let slash_literal = interpolation
        .parent_of(context)
        .filter(|parent| parent.kind_str() == "regex")
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
    match node.kind_str() {
        "integer" => integer_value(context.source.node_text(node)),
        "float" => float_value(context.source.node_text(node)),
        "unary" => signed_numeric(node, context),
        "string" => escape_string_content(&string_value(node, context)),
        // A character literal's source never starts with a quote, so upstream takes the branch of
        // `autocorrected_value_for_string` that only escapes the double quote.
        "character" => character_value(context.source.node_text(node)).replace('"', "\\\""),
        "simple_symbol" | "delimited_symbol" => symbol_content(node, context).replace('"', "\\\""),
        "array" | "string_array" | "symbol_array" => array_value(node, context),
        "hash" => hash_value(node, context),
        "nil" => String::new(),
        _ => context.source.node_text(node).replace('"', "\\\""),
    }
}

fn signed_numeric(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let Some(operand) = node.field("operand") else {
        return context.source.node_text(node).replace('"', "\\\"");
    };
    let negative = node
        .field("operator")
        .is_some_and(|operator| operator.kind_str() == "-");
    let magnitude = value(operand, context);
    match (negative, matches!(operand.kind_str(), "integer" | "float")) {
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
    match node.kind_str() {
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
                .field("key")
                .map_or_else(String::new, |key| value_in_hash(key, context));
            let held = pair
                .field("value")
                .map_or_else(String::new, |held| value_in_hash(held, context));
            format!("{key}=>{held}")
        })
        .collect();
    format!("{{{}}}", pairs.join(", "))
}

/// Inside a hash, a string or a symbol prints as `inspect` writes it rather than as its bare text,
/// because that is what `Hash#to_s` does with them.
fn value_in_hash(node: Node<'_>, context: &RuleContext<'_>) -> String {
    match node.kind_str() {
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
    match node.kind_str() {
        "hash_key_symbol" => context.source.node_text(node).to_owned(),
        "delimited_symbol" => string_value(node, context),
        _ => symbol_content(node, context).to_owned(),
    }
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
