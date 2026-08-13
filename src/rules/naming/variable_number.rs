use std::collections::HashSet;
use std::ops::Range;

use regex::Regex;
use tree_sitter::Node;

use super::support::{
    PARAMETER_LISTS, ParameterKind, bound_parameters, class_emitter_method,
    quoted_content, ruby_regex,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "normalcase".to_owned());
    let allowed: Vec<String> = context.setting("AllowedIdentifiers").unwrap_or_default();
    let patterns: Vec<&'static Regex> = context
        .setting::<Vec<serde_yaml_ng::Value>>("AllowedPatterns")
        .unwrap_or_default()
        .iter()
        .filter_map(ruby_regex)
        .collect();
    let check_method_names: bool = context.setting("CheckMethodNames").unwrap_or(true);
    let check_symbols: bool = context.setting("CheckSymbols").unwrap_or(true);

    let mut found: Vec<(Range<usize>, String, &'static str, Option<Node<'_>>)> = Vec::new();
    let mut parameter_names = HashSet::new();
    for list in context.nodes_of_any(PARAMETER_LISTS) {
        for (name, kind) in bound_parameters(list) {
            parameter_names.insert(name.start_byte());
            // Only a required parameter is an `arg` node; every other form has its own type and
            // no handler here.
            if kind == ParameterKind::Arg {
                found.push((
                    name.byte_range(),
                    context.source.node_text(name).to_owned(),
                    "variable",
                    None,
                ));
            }
        }
    }

    let variables = context.variable_roles();
    for node in context.nodes_of_any(&[
        "identifier",
        "instance_variable",
        "class_variable",
        "global_variable",
    ]) {
        if parameter_names.contains(&node.start_byte()) || !variables.is_definition(node) {
            continue;
        }
        found.push((
            node.byte_range(),
            context.source.node_text(node).to_owned(),
            "variable",
            None,
        ));
    }

    if check_method_names {
        for node in context.nodes_of_any(&["method", "singleton_method"]) {
            if let Some(name) = node.field("name") {
                found.push((
                    name.byte_range(),
                    context.source.node_text(name).to_owned(),
                    "method name",
                    Some(node),
                ));
            }
        }
    }

    if check_symbols {
        collect_symbols(context, &mut found);
    }

    found.sort_by_key(|(range, ..)| (range.start, range.end));
    for (range, name, identifier_type, definition) in found {
        // `allowed_identifier?` compares the name with its sigils removed, so `@x86_64` is as
        // allowed as `x86_64`.
        let bare: String = name.chars().filter(|c| *c != '@' && *c != '$').collect();
        if allowed.contains(&bare) {
            continue;
        }
        if valid_number(&name, &style)
            || patterns.iter().any(|pattern| pattern.is_match(&name))
            || definition.is_some_and(|node| class_emitter_method(node, &name, context.source))
        {
            continue;
        }
        offenses
            .push(context.offense(format!("Use {style} for {identifier_type} numbers."), range));
    }
}

/// `ConfigurableNumbering::FORMATS`, spelled out. Ruby writes `\d` and `\D` there, which stay
/// ASCII whatever the source encoding is.
fn valid_number(name: &str, style: &str) -> bool {
    let length = name.chars().count();
    let digits = name.chars().rev().take_while(char::is_ascii_digit).count();
    if length == 0 {
        return false;
    }
    // A name that does not end in a digit satisfies `\D\z`, and one that is nothing but digits
    // satisfies `\A\d+\z`; every style accepts both.
    if digits == 0 || digits == length {
        return true;
    }
    let before = name.chars().nth(length - digits - 1);
    // `\A_\d+\z`: the implicit block parameters `_1`, `_2` and the rest.
    let implicit = digits + 1 == length && before == Some('_');
    match style {
        "snake_case" => before == Some('_'),
        "non_integer" => implicit,
        // normalcase
        _ => implicit || before.is_some_and(|character| character != '_'),
    }
}

/// Every `sym` node in the file, with the value the parser reads off it.
fn collect_symbols<'tree>(
    context: &RuleContext<'tree>,
    found: &mut Vec<(Range<usize>, String, &'static str, Option<Node<'tree>>)>,
) {
    let mut push = |range: Range<usize>, value: String| {
        // A quoted empty hash key parses as an empty symbol, which names nothing.
        if !value.is_empty() {
            found.push((range, value, "symbol", None));
        }
    };
    for node in context.nodes_of_any(&[
        "simple_symbol",
        "delimited_symbol",
        "bare_symbol",
        "hash_key_symbol",
        "string",
        "alias",
        "undef",
    ]) {
        match node.kind_str() {
            "simple_symbol" => push(
                node.byte_range(),
                context
                    .source
                    .node_text(node)
                    .trim_start_matches(':')
                    .to_owned(),
            ),
            "delimited_symbol" | "bare_symbol" => {
                if let Some(value) = quoted_content(node, context.source) {
                    push(node.byte_range(), value);
                }
            }
            // A label is a symbol, except in a pattern match where `{key:}` binds a variable and
            // builds no symbol at all.
            "hash_key_symbol" => {
                let bare_pattern = node.parent().is_some_and(|parent| {
                    parent.kind_str() == "keyword_pattern"
                        && parent.field("value").is_none()
                });
                if !bare_pattern {
                    push(node.byte_range(), context.source.node_text(node).to_owned());
                }
            }
            // A quoted label -- `{ "key": 1 }` -- is a symbol too, while the same string before a
            // `=>` is not.
            "string" => {
                if is_quoted_label(node)
                    && let Some(value) = quoted_content(node, context.source)
                {
                    push(node.byte_range(), value);
                }
            }
            // `alias foo bar` and `undef foo` name methods with symbols the parser invents; only
            // an alias between global variables does not.
            _ => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if matches!(
                        child.kind_str(),
                        "identifier" | "constant" | "operator" | "setter"
                    ) {
                        push(
                            child.byte_range(),
                            context.source.node_text(child).to_owned(),
                        );
                    }
                }
            }
        }
    }
}

/// Whether a string stands where a symbol key would, which is decided by the `:` that follows it
/// rather than by the string itself.
fn is_quoted_label(node: Node<'_>) -> bool {
    let Some(parent) = node.parent().filter(|parent| parent.kind_str() == "pair") else {
        return false;
    };
    if parent.field("key") != Some(node) {
        return false;
    }
    let mut cursor = parent.walk();
    parent
        .children(&mut cursor)
        .any(|child| child.kind_str() == ":")
}
