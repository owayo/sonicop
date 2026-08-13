use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const SINGLE_QUOTES_MESSAGE: &str =
    "Prefer single-quoted strings when you don't need string interpolation or special symbols.";
const DOUBLE_QUOTES_MESSAGE: &str = "Prefer double-quoted strings unless you need single quotes to avoid extra backslashes for escaping.";
const INCONSISTENT_MESSAGE: &str = "Inconsistent quote style.";

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
    let ignored = if context
        .setting("ConsistentQuotesInMultiline")
        .unwrap_or(false)
    {
        check_dstr(context, single_quotes, message, offenses)
    } else {
        Vec::new()
    };
    for node in context.nodes_of("string") {
        if skipped(node, context) || part_of_ignored_node(node, &ignored) {
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
    // A literal the parser cuts into more than one line is a `dstr` whose per-line `str` children
    // carry no quotes of their own, so `on_str` bails on the missing `loc.begin`.
    if is_dstr(source) {
        return true;
    }
    inside_interpolation(node) || quoted_label_key(node, context)
}

/// RuboCop's `on_dstr`, which only reports under `ConsistentQuotesInMultiline`: a literal the
/// parser split into several `str` parts is judged as a whole, and its parts are then skipped.
///
/// Two shapes reach it. Adjacent literals -- `'a' 'b'`, or the same written with a `\` line
/// continuation -- become a `dstr` whose children keep their own quotes, which tree-sitter spells
/// as a `chained_string`. A single literal whose body spans lines becomes a `dstr` whose children
/// have no quotes at all, which tree-sitter still spells as one `string`.
///
/// Returns the ranges whose `str` descendants `on_str` must then leave alone.
fn check_dstr(
    context: &RuleContext<'_>,
    single_quotes: bool,
    message: &str,
    offenses: &mut Vec<Offense>,
) -> Vec<Range<usize>> {
    let mut ignored = Vec::new();
    for node in context.nodes_of("chained_string") {
        let mut children = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            // `all_string_literals?`: a chained literal only ever holds `str` and `dstr` parts, so
            // anything else can only come from error recovery, where upstream bails out.
            if child.kind_str() != "string" {
                children.clear();
                break;
            }
            let source = context.source.node_text(child);
            // Ruby only opens a percent literal where a value may begin, so the `%` after a
            // complete literal is the modulo operator and `"a" %q(b)` is a `send`, not a `dstr`.
            // tree-sitter chains the two anyway, and the first literal has to stay a plain `str`.
            if !children.is_empty() && source.starts_with('%') {
                children.clear();
                break;
            }
            children.push(Part {
                quote: opening_delimiter(source),
                source,
                // `accept_child_double_quotes?` lets a part off when it is itself a `dstr`, which
                // is what an interpolated or multi-line part parses as.
                dstr: has_interpolation(child) || is_dstr(source),
            });
        }
        if children.is_empty() {
            continue;
        }
        report_dstr(context, node, &children, single_quotes, message, offenses);
        ignored.push(node.byte_range());
    }
    for node in context.nodes_of("string") {
        let source = context.source.node_text(node);
        // An interpolation makes one of the `dstr`'s children a `begin` node, and upstream's
        // `all_string_literals?` then rejects the whole literal without ignoring it.
        if has_interpolation(node) || !is_dstr(source) {
            continue;
        }
        // The parts of such a literal carry no quotes, so upstream reads the one quote style off
        // the parent instead.
        let quote = opening_delimiter(source);
        let children: Vec<Part<'_>> = body_parts(source)
            .into_iter()
            .map(|source| Part {
                quote,
                source,
                dstr: false,
            })
            .collect();
        report_dstr(context, node, &children, single_quotes, message, offenses);
        ignored.push(node.byte_range());
    }
    ignored
}

/// One `str` or `dstr` child of a `dstr`, reduced to what upstream asks of it.
struct Part<'a> {
    /// `loc.begin.source`: the quote the part opens with, or the parent's when it has none.
    quote: &'a str,
    source: &'a str,
    dstr: bool,
}

/// RuboCop's `detect_quote_styles` followed by `check_multiline_quote_style`. A `dstr` offense
/// carries no correction: `StringLiteralCorrector` returns early for a `dstr`, which leaves the
/// corrector empty and the offense uncorrectable.
fn report_dstr(
    context: &RuleContext<'_>,
    node: Node<'_>,
    children: &[Part<'_>],
    single_quotes: bool,
    message: &str,
    offenses: &mut Vec<Offense>,
) {
    let quote = children[0].quote;
    if children.iter().any(|child| child.quote != quote) {
        offenses.push(context.offense(INCONSISTENT_MESSAGE, node.byte_range()));
        return;
    }
    let offends = if quote == "'" && !single_quotes {
        children
            .iter()
            .all(|child| wrong_quotes(child.source, single_quotes))
    } else if quote == "\"" && single_quotes {
        !children
            .iter()
            .any(|child| child.dstr || double_quotes_required(child.source))
    } else {
        false
    };
    if offends {
        offenses.push(context.offense(message, node.byte_range()));
    }
}

/// RuboCop's `IgnoredNode#part_of_ignored_node?`, which compares offsets rather than identity.
fn part_of_ignored_node(node: Node<'_>, ignored: &[Range<usize>]) -> bool {
    ignored
        .iter()
        .any(|range| range.start <= node.start_byte() && range.end >= node.end_byte())
}

/// The literal's opening delimiter, spelled the way `loc.begin.source` spells it: `"` or `'` for a
/// quoted literal, and the whole introducer -- `%q(`, `%Q{`, `%(` -- for a percent literal.
fn opening_delimiter(source: &str) -> &str {
    let Some(rest) = source.strip_prefix('%') else {
        return &source[..source.len().min(1)];
    };
    let mut end = 1;
    let mut characters = rest.chars();
    if let Some(first) = characters.next() {
        end += first.len_utf8();
        if first.is_ascii_alphabetic() {
            end += characters.next().map_or(0, char::len_utf8);
        }
    }
    &source[..end.min(source.len())]
}

/// The literal's body split the way the lexer splits it: one part per line, each keeping the line
/// break that ended it. A literal cut into more than one part is a `dstr` upstream.
///
/// A backslash escapes the character after it, a line break included, but only in the
/// double-quoted family: `'a\` + newline + `b'` holds a literal backslash and really does span two
/// lines, while `"a\` + newline + `b"` is one line continued.
fn for_each_body_part<'a>(source: &'a str, mut visit: impl FnMut(&'a str)) {
    let delimiter = opening_delimiter(source);
    let closing = source.chars().next_back().map_or(0, char::len_utf8);
    let Some(end) = source
        .len()
        .checked_sub(closing)
        .filter(|end| *end >= delimiter.len())
    else {
        return;
    };
    let body = &source[delimiter.len()..end];
    let escapes_newline = delimiter != "'" && !delimiter.starts_with("%q");
    let mut start = 0;
    let mut characters = body.char_indices();
    while let Some((index, character)) = characters.next() {
        match character {
            '\\' if escapes_newline => {
                characters.next();
            }
            '\n' => {
                visit(&body[start..=index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < body.len() {
        visit(&body[start..]);
    }
}

fn body_parts(source: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    for_each_body_part(source, |part| parts.push(part));
    parts
}

/// Whether the literal is a `dstr` upstream. Every literal in the file reaches this, so the common
/// case -- a body that holds no line break at all, and so cannot be cut into more than one part --
/// is answered without a scan.
pub(super) fn is_dstr(source: &str) -> bool {
    if !source.contains('\n') {
        return false;
    }
    let mut parts = 0usize;
    for_each_body_part(source, |_| parts += 1);
    parts > 1
}

pub(super) fn has_interpolation(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind_str() == "interpolation")
}

/// RuboCop's `StringHelp#inside_interpolation?`: from the innermost interpolation outwards, is the
/// literal owned by a `dstr`, `dsym` or `regexp`? The walk continues past the first interpolation
/// because a command literal nested in a string -- `"#{`cmd #{'x'}`}"` -- is still inside one.
pub(super) fn inside_interpolation(node: Node<'_>) -> bool {
    let mut current = node;
    let mut interpolated = false;
    while let Some(parent) = current.parent() {
        if parent.kind_str() == "interpolation" {
            interpolated = true;
        } else if interpolated && INTERPOLATION_OWNERS.contains(&parent.kind_str()) {
            return true;
        }
        current = parent;
    }
    false
}

/// A quoted hash key such as `'a': 1` is a symbol rather than a string, so re-quoting it would
/// change what it means.
pub(super) fn quoted_label_key(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind_str() == "pair"
        && parent
            .field("key")
            .is_some_and(|key| key.byte_range() == node.byte_range())
        && context.source.text().as_bytes().get(node.end_byte()) == Some(&b':')
}

/// RuboCop's `StringLiteralsHelp#wrong_quotes?`. The test runs over the literal's *source*, quotes
/// included, not over its value: what matters is whether swapping the delimiters would leave the
/// same text. `%`- and `?`-literals have no delimiter to swap.
pub(super) fn wrong_quotes(source: &str, single_quotes: bool) -> bool {
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
pub(super) fn corrected_literal(source: &str, single_quotes: bool) -> String {
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
    use super::{
        body_parts, double_quotes_required, is_dstr, opening_delimiter, single_quotes_required,
        to_string_literal,
    };

    #[test]
    fn the_opening_delimiter_covers_a_whole_percent_introducer() {
        assert_eq!(opening_delimiter(r#""a""#), "\"");
        assert_eq!(opening_delimiter("'a'"), "'");
        assert_eq!(opening_delimiter("%q(a)"), "%q(");
        assert_eq!(opening_delimiter("%Q{a}"), "%Q{");
        assert_eq!(opening_delimiter("%(a)"), "%(");
    }

    /// The lexer emits one part per line, so a break that only ends the literal leaves one part
    /// and the node stays a `str`. Verified against `ruby-parse`.
    #[test]
    fn a_literal_is_cut_into_one_part_per_line() {
        assert_eq!(body_parts("\"a\nb\""), vec!["a\n", "b"]);
        assert_eq!(body_parts("\"a\n\""), vec!["a\n"]);
        assert_eq!(body_parts("\"a\n\n\""), vec!["a\n", "\n"]);
        assert_eq!(body_parts("\"a\nb\nc\""), vec!["a\n", "b\n", "c"]);
        assert_eq!(body_parts("\"\""), Vec::<&str>::new());
        assert!(!is_dstr("\"a\n\""));
        assert!(is_dstr("\"a\nb\""));
    }

    /// A backslash swallows the line break in the double-quoted family only: `'a\` + newline is a
    /// literal backslash followed by a real break.
    #[test]
    fn a_backslash_only_continues_a_double_quoted_line() {
        assert_eq!(body_parts("\"a\\\nb\""), vec!["a\\\nb"]);
        assert!(!is_dstr("\"a\\\nb\""));
        assert_eq!(body_parts("'a\\\nb'"), vec!["a\\\n", "b"]);
        assert!(is_dstr("'a\\\nb'"));
        // An escaped backslash leaves the break unescaped.
        assert!(is_dstr("\"a\\\\\nb\""));
    }

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
