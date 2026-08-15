use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{has_interpolation, named_children, string_text, symbol_name};

/// The operators a symbol may be written bare, as `rb_enc_symname_type` accepts them.
const OPERATOR_SYMBOLS: [&str; 27] = [
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "[]", "[]=", "`", "!", "!=",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let consistent = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "consistent");
    // `on_send`: `"foo".to_sym` and `"foo".intern`.
    for node in context.nodes_of("call") {
        conversion_call(context, offenses, node);
    }
    // `on_hash` runs before the keys it holds, and the keys it marks are the ones `on_sym` then
    // leaves alone.
    let mut ignored: Vec<usize> = Vec::new();
    if consistent {
        for node in context.nodes_of("hash") {
            inconsistent_keys(context, offenses, node, &mut ignored);
        }
    }
    for node in context.nodes_of_any(&[
        "simple_symbol",
        "delimited_symbol",
        "hash_key_symbol",
        "string",
    ]) {
        if ignored.contains(&node.start_byte()) {
            continue;
        }
        symbol_literal(context, offenses, node, consistent);
    }
}

/// `on_send`: a conversion of something that is already a symbol, or of a literal that could have
/// been written as one.
fn conversion_call(context: &RuleContext<'_>, offenses: &mut Vec<Offense>, node: Node<'_>) {
    let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
        return;
    };
    if !matches!(context.source.node_text(method), "to_sym" | "intern") {
        return;
    }
    let Some(correction) = conversion_correction(receiver, context) else {
        return;
    };
    report(context, offenses, node, &correction, &correction, false);
}

fn conversion_correction(receiver: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match receiver.kind_str() {
        "string" | "delimited_symbol" if has_interpolation(receiver) => {
            // `dstr_correction`: a literal already written with `"` keeps its body verbatim.
            let text = context.source.node_text(receiver);
            text.starts_with('"')
                .then(|| format!(":\"{}\"", &text[1..text.len() - 1]))
        }
        "string" => Some(symbol_inspect(string_text(receiver, context))),
        "simple_symbol" | "delimited_symbol" | "hash_key_symbol" => {
            Some(symbol_inspect(symbol_name(receiver, context)?))
        }
        _ => None,
    }
}

/// `on_sym`.
fn symbol_literal(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    consistent: bool,
) {
    let Some(value) = literal_symbol_value(node, context) else {
        return;
    };
    let source = context.source.node_text(node);
    let inspected = symbol_inspect(&value);
    if properly_quoted(source, &inspected, consistent) {
        return;
    }
    // `in_alias?` and `in_percent_literal_array?`: neither spelling can carry quotes.
    let parent = node.parent_of(context);
    if parent.is_some_and(|parent| matches!(parent.kind_str(), "alias" | "symbol_array")) {
        return;
    }
    match hash_key_pair(node, context) {
        Some((_, colon)) => correct_hash_key(context, offenses, node, &value, colon, consistent),
        None => report(context, offenses, node, &inspected, &inspected, false),
    }
}

/// `correct_hash_key`.
fn correct_hash_key(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    value: &str,
    colon: bool,
    consistent: bool,
) {
    if !value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric() || first == '_')
    {
        return;
    }
    let inspected = symbol_inspect(value);
    let correction = match colon {
        true => inspected.trim_start_matches(':').to_owned(),
        false => inspected,
    };
    let source = context.source.node_text(node);
    if properly_quoted(source, &correction, consistent) {
        return;
    }
    let message = match colon {
        true => format!("{correction}:"),
        false => correction.clone(),
    };
    report(context, offenses, node, &correction, &message, false);
}

/// `on_hash` in the `consistent` style: once one key needs quotes, they all wear them.
fn inconsistent_keys(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    hash: Node<'_>,
    ignored: &mut Vec<usize>,
) {
    let keys: Vec<Node<'_>> = named_children(hash)
        .into_iter()
        .filter(|child| child.kind_str() == "pair")
        .filter_map(|pair| pair.field("key"))
        .filter(|key| literal_symbol_value(*key, context).is_some())
        .collect();
    let needs_quotes = |key: &Node<'_>| {
        literal_symbol_value(*key, context).is_some_and(|value| {
            let inspected = symbol_inspect(&value);
            inspected.starts_with(":\"") || inspected.ends_with('=')
        })
    };
    if !keys.iter().any(needs_quotes) {
        return;
    }
    for key in keys {
        ignored.push(key.start_byte());
        if needs_quotes(&key) {
            continue;
        }
        let Some(value) = literal_symbol_value(key, context) else {
            continue;
        };
        let correction = format!("\"{value}\"");
        if properly_quoted(context.source.node_text(key), &correction, true) {
            continue;
        }
        let message = format!("{correction}:");
        report(context, offenses, key, &correction, &message, true);
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    correction: &str,
    shown: &str,
    consistency: bool,
) {
    let message = match consistency {
        true => format!("Symbol hash key should be quoted for consistency; use `{shown}` instead."),
        false => format!("Unnecessary symbol conversion; use `{shown}` instead."),
    };
    let range = node.byte_range();
    offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: correction.to_owned(),
        safe: true,
    }));
}

/// `properly_quoted?`.
fn properly_quoted(source: &str, value: &str, consistent: bool) -> bool {
    if !consistent && (!source.contains(['\'', '"']) || value.ends_with('=')) {
        return true;
    }
    source == value || source.replace('"', "\\\"").replace('\'', "\"") == value
}

/// The name a `sym` node carries, for the four spellings the grammar gives one.
///
/// `"name": value` keys the pair by a symbol upstream while `"name" => value` keys it by a string,
/// so a `string` counts only where the separator is a colon.
fn literal_symbol_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match node.kind_str() {
        "string" => {
            let (_, colon) = hash_key_pair(node, context)?;
            (colon && !has_interpolation(node)).then(|| string_text(node, context).to_owned())
        }
        _ => symbol_name(node, context).map(str::to_owned),
    }
}

/// The pair this node is the key of, and whether that pair was written with a colon.
fn hash_key_pair<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<(Node<'tree>, bool)> {
    let parent = node.parent_of(context)?;
    if parent.kind_str() != "pair" || parent.field("key")?.id() != node.id() {
        return None;
    }
    let colon = parent
        .child(1)
        .is_some_and(|separator| context.source.node_text(separator) == ":")
        || node.kind_str() == "hash_key_symbol";
    Some((parent, colon))
}

/// `Symbol#inspect`.
fn symbol_inspect(name: &str) -> String {
    if is_bare_symbol(name) {
        return format!(":{name}");
    }
    let mut quoted = String::with_capacity(name.len() + 4);
    quoted.push_str(":\"");
    for character in name.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            '\u{1b}' => quoted.push_str("\\e"),
            '\0' => quoted.push_str("\\0"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// `rb_enc_symname_type`: whether the name can stand after a colon with no quotes.
fn is_bare_symbol(name: &str) -> bool {
    if OPERATOR_SYMBOLS.contains(&name) {
        return true;
    }
    let (prefixed, rest) = if let Some(rest) = name.strip_prefix("@@") {
        (true, rest)
    } else if let Some(rest) = name.strip_prefix('@').or_else(|| name.strip_prefix('$')) {
        (true, rest)
    } else {
        (false, name)
    };
    // Only a plain name may end in `?`, `!` or `=`.
    let core = match prefixed {
        true => rest,
        false => rest.strip_suffix(['?', '!', '=']).unwrap_or(rest),
    };
    let mut characters = core.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_' || !first.is_ascii())
        && characters.all(|character| {
            character.is_alphanumeric() || character == '_' || !character.is_ascii()
        })
}
