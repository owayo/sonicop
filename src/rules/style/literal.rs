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

/// A literal's content as the parser holds it: the value, and whether the bytes it names are text at
/// all. A `\xFF` escape puts a byte in the string that UTF-8 has no character for, which upstream
/// reads as `valid_encoding?` being false.
pub(super) struct Decoded {
    pub value: String,
    pub valid: bool,
}

/// The value of a literal body, as the parser hands it to a cop.
pub(super) fn decode(body: &str, quoting: Quoting, delimiters: &[char]) -> Decoded {
    let bytes = decode_bytes(body, quoting, delimiters);
    match String::from_utf8(bytes) {
        Ok(value) => Decoded { value, valid: true },
        Err(error) => Decoded {
            value: String::from_utf8_lossy(error.as_bytes()).into_owned(),
            valid: false,
        },
    }
}

fn decode_bytes(body: &str, quoting: Quoting, delimiters: &[char]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(body.len());
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            push_char(character, &mut out);
            continue;
        }
        let Some(next) = characters.next() else {
            out.push(b'\\');
            break;
        };
        match quoting {
            Quoting::Single => match next {
                '\\' => out.push(b'\\'),
                _ if delimiters.contains(&next) => push_char(next, &mut out),
                _ => {
                    out.push(b'\\');
                    push_char(next, &mut out);
                }
            },
            Quoting::Word => match next {
                '\\' => out.push(b'\\'),
                _ if next.is_whitespace() || delimiters.contains(&next) => {
                    push_char(next, &mut out)
                }
                _ => {
                    out.push(b'\\');
                    push_char(next, &mut out);
                }
            },
            Quoting::Double => decode_double_escape(next, &mut characters, &mut out),
        }
    }
    out
}

fn push_char(character: char, out: &mut Vec<u8>) {
    let mut buffer = [0u8; 4];
    out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
}

fn decode_double_escape(
    next: char,
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut Vec<u8>,
) {
    match next {
        'n' => out.push(b'\n'),
        't' => out.push(b'\t'),
        'r' => out.push(b'\r'),
        'f' => out.push(0x0c),
        'v' => out.push(0x0b),
        'a' => out.push(0x07),
        'b' => out.push(0x08),
        'e' => out.push(0x1b),
        's' => out.push(b' '),
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
            // `\xNN` names a byte, not a character: `"\xFF"` is not text at all.
            out.push(value as u8);
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
            out.push(value as u8);
        }
        other => push_char(other, out),
    }
}

fn decode_unicode(characters: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut Vec<u8>) {
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

fn push_code_point(value: u32, out: &mut Vec<u8>) {
    if let Some(character) = char::from_u32(value) {
        push_char(character, out);
    }
}

/// `escape_string`: `string.inspect` without its quotes, and with the escaped double quotes put
/// back the way they were written.
pub(super) fn escape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        escape_character(character, characters.peek().copied(), &mut out);
    }
    out
}

/// One character of `escape_string`, which needs to see the one after it to know whether a `#`
/// would have opened an interpolation.
fn escape_character(character: char, following: Option<char>, out: &mut String) {
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
        '#' if matches!(following, Some('{' | '$' | '@')) => out.push_str("\\#"),
        character if character.is_control() => {
            out.push_str(&format!("\\u{:04X}", character as u32));
        }
        character => out.push(character),
    }
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
pub(super) fn inspect_body(value: &str) -> String {
    escape_string(value).replace('"', "\\\"")
}

/// The value of a literal body as the bytes the parser put in the string.
///
/// A `\xFF` escape names a byte no character stands for, so a cop that writes the value back out
/// has to keep the bytes rather than the lossy text [`decode`] hands it.
pub(super) fn decode_raw(body: &str, quoting: Quoting, delimiters: &[char]) -> Vec<u8> {
    decode_bytes(body, quoting, delimiters)
}

/// `string.inspect` without its quotes, for a value that is not text throughout: `inspect` spells
/// a byte UTF-8 has no character for as `\xNN`.
///
/// `binary` is whether the source declared itself to hold bytes rather than text. Every byte above
/// ASCII is its own character there, so none of them join into the character their UTF-8 spells.
pub(super) fn inspect_bytes(bytes: &[u8], binary: bool) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        // A byte string has no code points, so `inspect` writes anything it cannot print as the
        // byte it is rather than as the `\uNNNN` a text string would name.
        if binary && !(0x20..0x7f).contains(&bytes[index]) && !NAMED_ESCAPE.contains(&bytes[index])
        {
            out.push_str(&format!("\\x{:02X}", bytes[index]));
            index += 1;
            continue;
        }
        match next_character(bytes, index).filter(|_| !binary || bytes[index] < 0x80) {
            Some((character, width)) => {
                let following = next_character(bytes, index + width).map(|(next, _)| next);
                escape_character(character, following, &mut out);
                index += width;
            }
            None => {
                out.push_str(&format!("\\x{:02X}", bytes[index]));
                index += 1;
            }
        }
    }
    out.replace('"', "\\\"")
}

/// The control characters `inspect` has a name for, which it uses in a byte string too.
const NAMED_ESCAPE: &[u8] = &[b'\n', b'\t', b'\r', 0x0c, 0x0b, 0x07, 0x08, 0x1b];

/// The character starting at `index`, and how many bytes it took, when the bytes there are one.
fn next_character(bytes: &[u8], index: usize) -> Option<(char, usize)> {
    let rest = bytes.get(index..)?;
    let width = match rest.first()? {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return None,
    };
    let character = std::str::from_utf8(rest.get(..width)?)
        .ok()?
        .chars()
        .next()?;
    Some((character, width))
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
pub(super) fn node_value(context: &RuleContext<'_>, node: Node<'_>) -> Option<Decoded> {
    let text = context.source.node_text(node);
    match node.kind() {
        "simple_symbol" => Some(Decoded {
            value: text.trim_start_matches(':').to_owned(),
            valid: true,
        }),
        // `?a` is a one-character `str` upstream, escapes and all.
        "character" => Some(decode(text.trim_start_matches('?'), Quoting::Double, &[])),
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
