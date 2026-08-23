use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal;

const MSG_SINGLE: &str = "Prefer single-quoted symbols when you don't need string interpolation \
                          or special symbols.";
const MSG_DOUBLE: &str = "Prefer double-quoted symbols unless you need single quotes to \
                          avoid extra backslashes for escaping.";

/// A quoted symbol whose quotes do not match the configured style.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let single = wants_single_quotes(context);
    for node in context.nodes_of_any(&["delimited_symbol", "string"]) {
        let Some(hash_key) = symbol_kind(node, context) else {
            continue;
        };
        let source = context.source.node_text(node);
        // `quoted?`: `/\A:?(['"]).*?\1\z/m`.
        let Some(quote) = quoted(source) else {
            continue;
        };
        // `wrong_quotes?` drops the leading `:` for a symbol written as one.
        let checked = if hash_key { source } else { &source[1..] };
        if !wrong_quotes(checked, single) && !invalid_double_quotes(source, single) {
            continue;
        }
        let _ = quote;
        // `autocorrect`: the inner text is requoted, and a symbol keeps its colon.
        let inner_start = if hash_key { 1 } else { 2 };
        let Some(inner) = source
            .get(inner_start..source.len().saturating_sub(1))
            .map(|inner| correct_quotes(inner, single))
        else {
            continue;
        };
        let replacement = if hash_key { inner } else { format!(":{inner}") };
        offenses.push(
            context
                .offense(
                    if single { MSG_SINGLE } else { MSG_DOUBLE },
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// Whether the node is a symbol at all, and whether it is the `'key':` of a hash.
///
/// `'key': 1` is a `sym` upstream whose source has no colon in it; the grammar leaves it a `string`
/// in the key position instead.
fn symbol_kind(node: Node<'_>, context: &RuleContext<'_>) -> Option<bool> {
    // An interpolating symbol is a `dsym` upstream and never reaches `on_sym`.
    if crate::rules::send_node::has_interpolation(node) {
        return None;
    }
    if node.kind_str() == "delimited_symbol" {
        // A literal running over more than one line is a `dsym` too: the parser gives each line a
        // `str` of its own and hangs them under one, so `on_sym` never sees it.
        if context.source.node_text(node).contains('\n') {
            return None;
        }
        // An empty `:""` is a `dsym` upstream as well -- but an empty hash key is a plain `sym`.
        return (node.named_child_count() > 0).then_some(false);
    }
    let parent = node.parent()?;
    if parent.kind_str() != "pair" || parent.field("key")?.id() != node.id() {
        return None;
    }
    // `node.parent.colon?`: the `'key': 1` spelling rather than `'key' => 1`.
    let text = &context.source.text()[node.end_byte()..parent.end_byte()];
    text.trim_start().starts_with(':').then_some(true)
}

/// `quoted?`: the source is wrapped in a matching pair of quotes, after an optional colon.
fn quoted(source: &str) -> Option<char> {
    let body = source.strip_prefix(':').unwrap_or(source);
    let mut characters = body.chars();
    let opening = characters.next()?;
    if opening != '\'' && opening != '"' {
        return None;
    }
    (body.len() >= 2 && body.ends_with(opening)).then_some(opening)
}

/// `wrong_quotes?`.
fn wrong_quotes(source: &str, single: bool) -> bool {
    if source.starts_with('%') || source.starts_with('?') {
        return false;
    }
    if single {
        return !double_quotes_required(source);
    }
    // `!/" | \\[^'\\] | \#[@{$]/x`.
    !(source.contains('"') || has_plain_escape(source) || has_interpolation_marker(source))
}

/// `invalid_double_quotes?`, which only the double-quoted style asks.
fn invalid_double_quotes(source: &str, single: bool) -> bool {
    if single {
        return false;
    }
    // `!/" | (?<!\\)\\[aAbcdefkMnprsStuUxzZ0-7] | \#[@{$]/x`.
    !(source.contains('"') || has_known_escape(source) || has_interpolation_marker(source))
}

/// `double_quotes_required?`: a `'` in the text, or a lone backslash that is not escaping a quote.
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
        let start = index;
        while index < bytes.len() && bytes[index] == b'\\' {
            index += 1;
        }
        if (index - start) % 2 == 1 && bytes.get(index) != Some(&b'"') {
            return true;
        }
    }
    false
}

/// `\\[^'\\]`: a backslash escaping anything but a quote or another backslash.
fn has_plain_escape(source: &str) -> bool {
    let bytes = source.as_bytes();
    (0..bytes.len()).any(|index| {
        bytes[index] == b'\\'
            && bytes
                .get(index + 1)
                .is_some_and(|next| !matches!(next, b'\'' | b'\\'))
    })
}

/// `(?<!\\)\\[aAbcdefkMnprsStuUxzZ0-7]`: one of the escapes a double-quoted string gives a meaning.
fn has_known_escape(source: &str) -> bool {
    const KNOWN: &[u8] = b"aAbcdefkMnprsStuUxzZ01234567";
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] == b'\\' {
            index += 1;
        }
        // Only the last backslash of an odd run is unescaped itself.
        if (index - start) % 2 == 1 && bytes.get(index).is_some_and(|next| KNOWN.contains(next)) {
            return true;
        }
    }
    false
}

/// `\#[@{$]`.
fn has_interpolation_marker(source: &str) -> bool {
    let bytes = source.as_bytes();
    (0..bytes.len()).any(|index| {
        bytes[index] == b'#' && matches!(bytes.get(index + 1), Some(b'@' | b'{' | b'$'))
    })
}

/// `correct_quotes`.
fn correct_quotes(inner: &str, single: bool) -> String {
    let correction = if single {
        to_string_literal(inner)
    } else {
        ruby_literal::inspect_string(&inner.replace("\\'", "'"))
    };
    correction.replace("\\\\", "\\").replace("\\\"", "\"")
}

/// `Util#to_string_literal`.
fn to_string_literal(value: &str) -> String {
    if needs_escaping(value) {
        return ruby_literal::inspect_string(value);
    }
    format!("'{}'", value.replace('\\', "\\\\").replace("\\\"", "\""))
}

/// `needs_escaping?`.
fn needs_escaping(value: &str) -> bool {
    double_quotes_required(&escape_string(value))
}

/// `escape_string`: `string.inspect[1..-2]` with `\"` turned back into `"`.
fn escape_string(value: &str) -> String {
    let inspected = ruby_literal::inspect_string(value);
    inspected[1..inspected.len() - 1].replace("\\\"", "\"")
}

/// `same_as_string_literals` follows `Style/StringLiterals`, and falls back to single quotes when
/// that cop is switched off.
fn wants_single_quotes(context: &RuleContext<'_>) -> bool {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "same_as_string_literals".to_owned());
    if style != "same_as_string_literals" {
        return style == "single_quotes";
    }
    if !context.cop_enabled("Style/StringLiterals") {
        return true;
    }
    context
        .setting_of::<String>("Style/StringLiterals", "EnforcedStyle")
        .is_none_or(|inner| inner == "single_quotes")
}
