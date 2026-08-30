use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{has_interpolation, string_text, symbol_name};
use crate::rules::send_node::named_children_of;

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
        // `foo(a: 1, b: 2)` contains one `hash` argument upstream, but tree-sitter leaves its
        // pairs directly under the argument list. `on_hash` therefore has a second structural
        // spelling here even though no braces were written.
        for list in context.nodes_of("argument_list") {
            let keys: Vec<Node<'_>> = named_children_of(list, context)
                .into_iter()
                .filter(|child| child.kind_str() == "pair")
                .filter_map(|pair| pair.field("key"))
                .filter(|key| literal_symbol_value(*key, context).is_some())
                .collect();
            correct_inconsistent_keys(context, offenses, keys, &mut ignored);
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
    // **`on_send` is not `on_csend`.** The cop declares only the first, so `"x"&.to_sym` goes
    // unchecked -- the grammar spells both as a `call`.
    if node
        .field("operator")
        .is_some_and(|dot| context.source.node_text(dot) == "&.")
    {
        return;
    }
    let Some(correction) = conversion_correction(receiver, context) else {
        return;
    };
    report(context, offenses, node, &correction, &correction, false);
}

fn conversion_correction(receiver: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match receiver.kind_str() {
        // Literals written next to each other are one `dstr` upstream, and it has no delimiters of
        // its own -- so the correction is built from the value, which keeps each interpolation as it
        // was written and strips the quotes around every part.
        "chained_string" => {
            let mut out = String::from(":\"");
            for part in named_children_of(receiver, context) {
                for inner in named_children_of(part, context) {
                    out.push_str(context.source.node_text(inner));
                }
            }
            out.push('"');
            Some(out)
        }
        // **`:"#{x}"` is a `dsym`, which `symbol_conversion_correction` answers `nil` for.** Only
        // `str`, `sym` and a non-heredoc `dstr` reach a correction there, so an interpolated
        // symbol is left alone -- while an interpolated *string* is corrected.
        "delimited_symbol" if has_interpolation(receiver) => None,
        "string" if has_interpolation(receiver) => {
            // `dstr_correction`: a literal already written with `"` keeps its body verbatim. A
            // `%Q{…}` takes the other arm, which reads the same body back off the parts -- and
            // whose opening delimiter is **three characters**, not one.
            let text = context.source.node_text(receiver);
            let open = receiver
                .child(0)
                .map_or(1, |node| context.source.node_text(node).len());
            // The closing delimiter is one character for every spelling the parts leave behind.
            let close = 1;
            text.get(open..text.len().checked_sub(close)?)
                .map(|body| format!(":\"{body}\""))
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
    // **A quoted symbol spanning lines is a `dsym`, not a `sym`.** The parser cuts the contents
    // after every line break and wraps the pieces, so `:'a\nb'` is a `dsym` of two `str`s that
    // `on_sym` never sees. A break that closes the contents leaves one piece and stays a `sym`,
    // which is why `:"two\n"` is still this cop's business.
    if node.kind_str() == "delimited_symbol" && crate::rules::support::quoted_spans_lines(source) {
        return;
    }
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
    let keys: Vec<Node<'_>> = named_children_of(hash, context)
        .into_iter()
        .filter(|child| child.kind_str() == "pair")
        .filter_map(|pair| pair.field("key"))
        .filter(|key| literal_symbol_value(*key, context).is_some())
        .collect();
    correct_inconsistent_keys(context, offenses, keys, ignored);
}

fn correct_inconsistent_keys(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    keys: Vec<Node<'_>>,
    ignored: &mut Vec<usize>,
) {
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
            (colon && !has_interpolation(node))
                .then(|| crate::rules::ruby_literal::string_value(node, context))
        }
        // A quoted symbol resolves its escapes the way a string does. The value is what
        // `Symbol#inspect` writes back out, so keeping `\"` as written would double the backslash
        // and make every escaped quote look like a difference.
        "delimited_symbol" if !has_interpolation(node) => {
            Some(crate::rules::ruby_literal::string_value(node, context))
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

/// `Symbol#inspect`, which is where the decision to quote is made.
///
/// The shared reading of `rb_enc_symname_type` is the one to use: the operators, the `?`, `!` and
/// `=` a method name may close with, and the one-character globals such as `$'` all stand bare, and
/// quoting any of them is exactly what this cop reports.
fn symbol_inspect(name: &str) -> String {
    crate::rules::ruby_literal::inspect_symbol(name)
}
