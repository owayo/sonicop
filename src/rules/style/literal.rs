//! Reading and writing Ruby string and symbol literals.
//!
//! A cop that rewrites `['a', 'b']` into `%w[a b]` has to work in values rather than in source:
//! upstream reads the parsed content out of the node and writes a fresh literal around it. These
//! are the two halves of that -- the parser's unescaping, and `Util#to_string_literal`'s escaping.

use tree_sitter::Node;

use crate::rules::RuleContext;

/// How a literal's body escapes what it holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Quoting {
    /// `'...'` and `%q(...)`: only the backslash and the closing delimiter can be escaped.
    Single,
    /// `"..."`, `%Q(...)` and `%W[...]`: the full set of escapes.
    Double,
    /// A word of `%w[...]`: a backslash only bites on blanks, delimiters and itself.
    Word,
}

/// The value of a literal body, as the parser hands it to a cop.
pub(super) fn decode(body: &str, quoting: Quoting, delimiters: &[char]) -> String {
    let mut out = String::with_capacity(body.len());
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        let Some(next) = characters.next() else {
            out.push('\\');
            break;
        };
        match quoting {
            Quoting::Single => match next {
                '\\' => out.push('\\'),
                _ if delimiters.contains(&next) => out.push(next),
                _ => {
                    out.push('\\');
                    out.push(next);
                }
            },
            Quoting::Word => match next {
                '\\' => out.push('\\'),
                _ if next.is_whitespace() || delimiters.contains(&next) => out.push(next),
                _ => {
                    out.push('\\');
                    out.push(next);
                }
            },
            Quoting::Double => decode_double_escape(next, &mut characters, &mut out),
        }
    }
    out
}

fn decode_double_escape(
    next: char,
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
) {
    match next {
        'n' => out.push('\n'),
        't' => out.push('\t'),
        'r' => out.push('\r'),
        'f' => out.push('\x0c'),
        'v' => out.push('\x0b'),
        'a' => out.push('\x07'),
        'b' => out.push('\x08'),
        'e' => out.push('\x1b'),
        's' => out.push(' '),
        '\n' => {}
        'u' => decode_unicode(characters, out),
        'x' => {
            let mut value = 0u32;
            let mut digits = 0;
            while digits < 2 {
                match characters.peek().and_then(|c| c.to_digit(16)) {
                    Some(digit) => {
                        value = value * 16 + digit;
                        characters.next();
                        digits += 1;
                    }
                    None => break,
                }
            }
            push_code_point(value, out);
        }
        '0'..='7' => {
            let mut value = next.to_digit(8).unwrap_or(0);
            let mut digits = 1;
            while digits < 3 {
                match characters.peek().and_then(|c| c.to_digit(8)) {
                    Some(digit) => {
                        value = value * 8 + digit;
                        characters.next();
                        digits += 1;
                    }
                    None => break,
                }
            }
            push_code_point(value, out);
        }
        other => out.push(other),
    }
}

fn decode_unicode(characters: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    if characters.peek() == Some(&'{') {
        characters.next();
        let mut value = 0u32;
        let mut seen = false;
        for character in characters.by_ref() {
            match character.to_digit(16) {
                Some(digit) => {
                    value = value * 16 + digit;
                    seen = true;
                }
                None => {
                    if seen {
                        push_code_point(value, out);
                    }
                    value = 0;
                    seen = false;
                    if character == '}' {
                        break;
                    }
                }
            }
        }
        if seen {
            push_code_point(value, out);
        }
        return;
    }
    let mut value = 0u32;
    for _ in 0..4 {
        match characters.peek().and_then(|c| c.to_digit(16)) {
            Some(digit) => {
                value = value * 16 + digit;
                characters.next();
            }
            None => break,
        }
    }
    push_code_point(value, out);
}

fn push_code_point(value: u32, out: &mut String) {
    if let Some(character) = char::from_u32(value) {
        out.push(character);
    }
}

/// `escape_string`: `string.inspect` without its quotes, and with the escaped double quotes put
/// back the way they were written.
pub(super) fn escape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\x0c' => out.push_str("\\f"),
            '\x0b' => out.push_str("\\v"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x1b' => out.push_str("\\e"),
            // `inspect` escapes a `#` only where it would open an interpolation.
            '#' if matches!(characters.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
            character if character.is_control() => {
                out.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => out.push(character),
        }
    }
    out
}

/// `double_quotes_required?`: the escaped form holds a single quote, or a backslash standing alone.
fn double_quotes_required(escaped: &str) -> bool {
    if escaped.contains('\'') {
        return true;
    }
    let characters: Vec<char> = escaped.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] != '\\' {
            index += 1;
            continue;
        }
        // Count the run of backslashes: an even-length run followed by one more is the lone
        // backslash the pattern looks for, unless a `\` or `"` follows it.
        let start = index;
        while index < characters.len() && characters[index] == '\\' {
            index += 1;
        }
        let run = index - start;
        if run % 2 == 1 && !matches!(characters.get(index), Some('\\' | '"')) {
            return true;
        }
    }
    false
}

pub(super) fn needs_escaping(value: &str) -> bool {
    double_quotes_required(&escape_string(value))
}

/// `to_string_literal`: the shortest literal that spells this value back.
pub(super) fn to_string_literal(value: &str) -> String {
    if needs_escaping(value) {
        return format!("\"{}\"", inspect_body(value));
    }
    format!("'{}'", value.replace('\\', "\\\\").replace("\\\"", "\""))
}

/// `string.inspect` without its quotes, which is `escape_string` with the double quotes escaped.
fn inspect_body(value: &str) -> String {
    escape_string(value).replace('"', "\\\"")
}

/// `to_symbol_literal`: a name the parser reads back as this symbol.
pub(super) fn to_symbol_literal(value: &str) -> String {
    if symbol_without_quote(value) {
        return format!(":{value}");
    }
    format!(":{}", to_string_literal(value))
}

/// The global variables `symbol_without_quote?` accepts by name.
const SPECIAL_GLOBALS: &[&str] = &[
    "$!", "$\"", "$$", "$&", "$'", "$*", "$+", "$,", "$/", "$;", "$:", "$.", "$<", "$=", "$>",
    "$?", "$@", "$\\", "$_", "$`", "$~", "$0", "$-0", "$-F", "$-I", "$-K", "$-W", "$-a", "$-d",
    "$-i", "$-l", "$-p", "$-v", "$-w",
];

/// The operators a class may redefine, which are all legal bare symbol names.
const REDEFINABLE_OPERATORS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "[]", "[]=", "`", "!", "!=", "!~",
];

fn symbol_without_quote(value: &str) -> bool {
    let bare = |name: &str| {
        let mut characters = name.chars();
        characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
    };
    let method_name = |name: &str| {
        let trimmed = name.strip_suffix(['!', '?']).unwrap_or(name);
        !trimmed.is_empty() && bare(trimmed)
    };
    let variable = |name: &str| match name.strip_prefix("@@").or_else(|| name.strip_prefix('@')) {
        Some(rest) => bare(rest),
        None => false,
    };
    let global = |name: &str| match name.strip_prefix('$') {
        Some(rest) => {
            let numbered = rest.starts_with(|c: char| c.is_ascii_digit() && c != '0')
                && rest.chars().all(|c| c.is_ascii_digit());
            numbered || bare(rest)
        }
        None => false,
    };

    method_name(value)
        || variable(value)
        || global(value)
        || SPECIAL_GLOBALS.contains(&value)
        || REDEFINABLE_OPERATORS.contains(&value)
}

/// `trim_string_interpolation_escape_character`: an escaped `#{}` written back the way it came.
pub(super) fn trim_interpolation_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < value.len() {
        if bytes[index] == b'\\'
            && value[index..].starts_with("\\#{")
            && let Some(close) = value[index..].find('}')
        {
            out.push_str(&value[index + 1..index + close + 1]);
            index += close + 1;
            continue;
        }
        let next = value[index..]
            .char_indices()
            .nth(1)
            .map_or(value.len(), |(offset, _)| index + offset);
        out.push_str(&value[index..next]);
        index = next;
    }
    out
}

/// The value of one string or symbol node, and whether it is a literal at all.
pub(super) fn node_value(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let text = context.source.node_text(node);
    match node.kind() {
        "simple_symbol" => Some(text.trim_start_matches(':').to_owned()),
        "string" | "delimited_symbol" | "subshell" => {
            let begin = node.child(0)?;
            let close = node.child(node.child_count().saturating_sub(1) as u32)?;
            if begin.id() == close.id() {
                return None;
            }
            let opener = context.source.node_text(begin);
            let body = &context.source.text()[begin.end_byte()..close.start_byte()];
            let quoting = match opener {
                "'" | "%q(" => Quoting::Single,
                _ if opener.starts_with("%q") => Quoting::Single,
                _ if opener.starts_with(":'") => Quoting::Single,
                _ => Quoting::Double,
            };
            let closing = context.source.node_text(close).chars().next()?;
            Some(decode(
                body,
                quoting,
                &[opener.chars().next_back()?, closing],
            ))
        }
        "bare_string" | "bare_symbol" => {
            let array = node.parent()?;
            let opener = context.source.node_text(array.child(0)?);
            let closing = context
                .source
                .node_text(array.child(array.child_count().saturating_sub(1) as u32)?)
                .chars()
                .next()?;
            let uppercase = opener
                .chars()
                .nth(1)
                .is_some_and(|kind| kind == 'W' || kind == 'I');
            let quoting = match uppercase {
                true => Quoting::Double,
                false => Quoting::Word,
            };
            Some(decode(
                text,
                quoting,
                &[opener.chars().next_back()?, closing],
            ))
        }
        _ => None,
    }
}
