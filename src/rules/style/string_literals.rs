use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const SINGLE_QUOTES_MESSAGE: &str =
    "Prefer single-quoted strings when you don't need string interpolation or special symbols.";
const DOUBLE_QUOTES_MESSAGE: &str = "Prefer double-quoted strings unless you need single quotes to avoid extra backslashes for escaping.";

/// Literal kinds whose interpolation makes the parts around it a `dstr`, `dsym` or `regexp` in
/// RuboCop's AST. A string inside one of those is left to `Style/StringLiteralsInInterpolation`.
/// Backticks are deliberately absent: an `xstr` is none of those three, so RuboCop *does* check
/// the strings interpolated into a command literal.
const INTERPOLATION_OWNERS: &[&str] = &[
    "string",
    "bare_string",
    "delimited_symbol",
    "bare_symbol",
    "heredoc_body",
    "regex",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "single_quotes".to_owned());
    let single_quotes = style != "double_quotes";
    let message = if single_quotes {
        SINGLE_QUOTES_MESSAGE
    } else {
        DOUBLE_QUOTES_MESSAGE
    };
    for node in context.nodes_of("string") {
        if skipped(node, context) {
            continue;
        }
        let source = context.source.node_text(node);
        if !wrong_quotes(source, single_quotes) {
            continue;
        }
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: corrected_literal(source, single_quotes),
                    safe: true,
                }),
        );
    }
}

/// Whether the cop never sees this literal at all.
///
/// RuboCop hangs the check off `on_str`, so anything the parser turns into something other than a
/// plain `str` node -- or into a `str` without quotes of its own -- is out of reach. tree-sitter
/// has one `string` node for all of them, so the distinctions have to be rebuilt here.
fn skipped(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let source = context.source.node_text(node);
    // An unterminated literal in a file that does not parse has no closing delimiter to swap.
    if source.len() < 2 {
        return true;
    }
    // `"a#{b}"`, `"#@a"` and `"#$a"` are all `dstr` nodes upstream, and `on_dstr` only reports
    // under `ConsistentQuotesInMultiline`, which is off by default.
    if has_interpolation(node) {
        return true;
    }
    // A literal whose *value* spans lines is a `dstr` whose per-line `str` children carry no
    // quotes, so `on_str` bails on the missing `loc.begin`. A backslash-continued line break stays
    // a single `str`, but it also forces double quotes, so skipping every raw newline agrees.
    if source.contains('\n') {
        return true;
    }
    inside_interpolation(node) || quoted_label_key(node, context)
}

fn has_interpolation(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
}

/// RuboCop's `StringHelp#inside_interpolation?`: from the innermost interpolation outwards, is the
/// literal owned by a `dstr`, `dsym` or `regexp`? The walk continues past the first interpolation
/// because a command literal nested in a string -- `"#{`cmd #{'x'}`}"` -- is still inside one.
fn inside_interpolation(node: Node<'_>) -> bool {
    let mut current = node;
    let mut interpolated = false;
    while let Some(parent) = current.parent() {
        if parent.kind() == "interpolation" {
            interpolated = true;
        } else if interpolated && INTERPOLATION_OWNERS.contains(&parent.kind()) {
            return true;
        }
        current = parent;
    }
    false
}

/// A quoted hash key such as `'a': 1` is a symbol rather than a string, so re-quoting it would
/// change what it means.
fn quoted_label_key(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == "pair"
        && parent
            .child_by_field_name("key")
            .is_some_and(|key| key.byte_range() == node.byte_range())
        && context.source.text().as_bytes().get(node.end_byte()) == Some(&b':')
}

/// RuboCop's `StringLiteralsHelp#wrong_quotes?`. The test runs over the literal's *source*, quotes
/// included, not over its value: what matters is whether swapping the delimiters would leave the
/// same text. `%`- and `?`-literals have no delimiter to swap.
fn wrong_quotes(source: &str, single_quotes: bool) -> bool {
    if source.starts_with('%') || source.starts_with('?') {
        return false;
    }
    if single_quotes {
        !double_quotes_required(source)
    } else {
        !single_quotes_required(source)
    }
}

/// RuboCop's `Util#double_quotes_required?`, spelled upstream as `/'|(?<!\\)\\{2}*\\(?![\\"])/`.
///
/// Double quotes are needed for a single quote, and for a backslash that escapes anything but a
/// double quote. The regex phrases the latter as "a run of backslashes of odd length not followed
/// by `"`": an even run is a sequence of escaped backslashes, which a single-quoted literal writes
/// exactly the same way, and `\"` becomes a bare `"` there.
fn double_quotes_required(source: &str) -> bool {
    if source.contains('\'') {
        return true;
    }
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < bytes.len() && bytes[index] == b'\\' {
            index += 1;
        }
        // The run is maximal, so the byte after it can only fail the upstream `(?![\\"])`
        // lookahead by being a double quote.
        if (index - run_start) % 2 == 1 && bytes.get(index) != Some(&b'"') {
            return true;
        }
    }
    false
}

/// The `double_quotes` half of `wrong_quotes?`, upstream `/"|\\[^'\\]|\#[@{$]/`. Unlike the
/// single-quote test this is a plain scan rather than a run count, because a single-quoted literal
/// only gives `\` a meaning before `\` and `'`.
fn single_quotes_required(source: &str) -> bool {
    let bytes = source.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'"' => return true,
            b'\\' => {
                if matches!(bytes.get(index + 1), Some(next) if *next != b'\'' && *next != b'\\') {
                    return true;
                }
            }
            b'#' => {
                if matches!(bytes.get(index + 1), Some(b'@' | b'{' | b'$')) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// RuboCop's `StringLiteralCorrector`: the literal is rebuilt from its *value*, so escapes that
/// only existed to satisfy the old delimiter disappear.
fn corrected_literal(source: &str, single_quotes: bool) -> String {
    // Only `"` and `'` reach here -- `%`-literals never offend -- so the delimiters are one byte.
    let inner = &source[1..source.len() - 1];
    if single_quotes {
        to_string_literal(&decode_double_quoted(inner))
    } else {
        format!("\"{}\"", inspect_body(&decode_single_quoted(inner)))
    }
}

/// The value of a double-quoted literal that the cop reported. Only `\\` and `\"` can appear:
/// every other escape would have made [`double_quotes_required`] true and suppressed the offense.
fn decode_double_quoted(inner: &str) -> String {
    unescape(inner, &['\\', '"'])
}

/// The value of a single-quoted literal, where `\\` and `\'` are the only escapes Ruby recognises.
fn decode_single_quoted(inner: &str) -> String {
    unescape(inner, &['\\', '\''])
}

fn unescape(inner: &str, escapable: &[char]) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some(next) if escapable.contains(&next) => out.push(next),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// RuboCop's `Util#to_string_literal`. A value that Ruby would have to escape -- a raw tab, say --
/// cannot be written single-quoted at all, so upstream falls back to `String#inspect` and keeps
/// the double quotes.
fn to_string_literal(value: &str) -> String {
    if double_quotes_required(&escape_string(value)) {
        return format!("\"{}\"", inspect_body(value));
    }
    format!("'{}'", value.replace('\\', "\\\\").replace("\\\"", "\""))
}

/// RuboCop's `Util#escape_string`: `String#inspect` without the delimiters and with `\"` folded
/// back to `"`, so that the leftover backslashes are exactly the ones the value itself needs.
fn escape_string(value: &str) -> String {
    inspect_body(value).replace("\\\"", "\"")
}

/// The body of Ruby's `String#inspect` for a valid UTF-8 string: control characters become their
/// named escape or `\uXXXX`, `"` and `\` are escaped, and a `#` that would start an interpolation
/// is neutralised. Everything else, non-ASCII included, is printable to Ruby and stays put.
fn inspect_body(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\x0b' => out.push_str("\\v"),
            '\x0c' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\x1b' => out.push_str("\\e"),
            '#' if matches!(characters.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
            _ if character.is_ascii_control() => {
                out.push_str(&format!("\\u{:04X}", character as u32));
            }
            _ => out.push(character),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{double_quotes_required, single_quotes_required, to_string_literal};

    #[test]
    fn even_backslash_runs_do_not_require_double_quotes() {
        assert!(!double_quotes_required(r#""\\x34""#));
        assert!(!double_quotes_required(r#""\\""#));
        assert!(double_quotes_required(r#""\x34""#));
    }

    #[test]
    fn an_escaped_double_quote_does_not_require_double_quotes() {
        assert!(!double_quotes_required(r#""{\"k\":\"v\"}""#));
        assert!(!double_quotes_required(r#""\\\"x""#));
    }

    #[test]
    fn a_single_quote_requires_double_quotes() {
        assert!(double_quotes_required(r#""it's""#));
    }

    #[test]
    fn interpolation_syntax_requires_double_quotes() {
        assert!(single_quotes_required("'#{a}'"));
        assert!(single_quotes_required("'#@a'"));
        assert!(!single_quotes_required("'#a'"));
        assert!(single_quotes_required(r"'a\nb'"));
        // The scan has no run count: the second backslash of `\\b` escapes `b` as far as the
        // upstream regex is concerned, while `\\` before the closing quote escapes nothing.
        assert!(single_quotes_required(r"'a\\b'"));
        assert!(!single_quotes_required(r"'a\\'"));
    }

    #[test]
    fn corrections_rebuild_the_literal_from_its_value() {
        assert_eq!(to_string_literal(r#"{"k":"v"}"#), r#"'{"k":"v"}'"#);
        assert_eq!(to_string_literal(r"\x34"), r"'\\x34'");
        assert_eq!(to_string_literal("a\tb"), r#""a\tb""#);
    }
}
