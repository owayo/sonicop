use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::literal::{node_value, to_string_literal, trim_interpolation_escape};
use super::nodes;
use super::percent_array::{
    Bracketed, Element, allowed_bracket_array, bracketed_replacement, elements,
    percent_array_offense, percent_replacement, percent_values,
};

const PERCENT_MSG: &str = "Use `%w` or `%W` for an array of words.";
const ARRAY_MSG: &str = "Use %<prefer>s for an array of words.";

/// `WordRegex`'s default, with Ruby's `\p{Word}` written as this engine spells the same class.
static DEFAULT_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\w|\w-\w|\n|\t)+$").unwrap());

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "percent".to_owned());
    let word = word_regex(context);

    for node in context.nodes_of("array") {
        let items = nodes::children(node);
        if items.is_empty() || !items.iter().all(|item| is_word(context, *item)) {
            continue;
        }
        let Some(values) = values(context, &items) else {
            continue;
        };
        if complex_content(&values, word.as_ref())
            || within_matrix_of_complex_content(context, node, word.as_ref())
        {
            continue;
        }
        let array = Bracketed { node, items };
        if allowed_bracket_array(context, &array) || style != "percent" {
            continue;
        }
        let replacement = percent_replacement(context, &array, 'w', &values);
        offenses.push(
            context
                .offense(PERCENT_MSG, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }

    for node in context.nodes_of("string_array") {
        let items = elements(context, node);
        let values = percent_values(context, node, &items);
        // `invalid_percent_array_contents?` drops the word test: only a blank forces brackets.
        let brackets_required = complex_content(&values, None);
        if style != "brackets" && !brackets_required {
            continue;
        }
        let text = context.source.text();
        let written: Vec<String> = items
            .iter()
            .zip(&values)
            .map(|(item, value)| word_source(&text[item.range.clone()], item, value))
            .collect();
        let bracketed = bracketed_replacement(context, node, &items, &written);
        offenses.push(percent_array_offense(context, node, ARRAY_MSG, bracketed));
    }
}

/// `bracketed_array_of?(:str, node)`: a plain string, so neither interpolated nor split across
/// lines, both of which upstream's parser turns into a `dstr`.
fn is_word(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "string"
        && !interpolated(node)
        && !context.source.node_text(node).contains('\n')
}

fn interpolated(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
}

fn values(context: &RuleContext<'_>, items: &[Node<'_>]) -> Option<Vec<String>> {
    items
        .iter()
        .map(|item| node_value(context, *item))
        .collect()
}

/// `complex_content?`: a word that does not look like one, or that holds a blank.
fn complex_content(values: &[String], word: Option<&Regex>) -> bool {
    values
        .iter()
        .any(|value| word.is_some_and(|pattern| !pattern.is_match(value)) || value.contains(' '))
}

/// `within_matrix_of_complex_content?`: a row of a table whose other rows hold phrases keeps its
/// brackets, so that the table stays readable as a whole.
fn within_matrix_of_complex_content(
    context: &RuleContext<'_>,
    node: Node<'_>,
    word: Option<&Regex>,
) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "array" {
        return false;
    }
    let rows = nodes::children(parent);
    rows.iter().all(|row| row.kind() == "array")
        && rows.iter().any(|row| {
            let items = nodes::children(*row);
            values(context, &items).is_some_and(|values| complex_content(&values, word))
        })
}

/// `build_bracketed_array`'s per-element source: a `dstr` keeps its written form, a plain word is
/// written back from its value.
fn word_source(source: &str, item: &Element, value: &str) -> String {
    if item.interpolated {
        let literal = to_string_literal(source);
        return trim_interpolation_escape(&literal);
    }
    to_string_literal(value)
}

/// The configured `WordRegex`, or the bundled default when it cannot be read as a pattern.
fn word_regex(context: &RuleContext<'_>) -> Option<Regex> {
    let Some(configured) = context.setting::<String>("WordRegex") else {
        return Some(DEFAULT_WORD.clone());
    };
    let body = configured
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap_or(&configured);
    // Ruby anchors with `\A` and `\z`; this engine spells the same anchors `^` and `$` without the
    // multi-line flag, and writes `\p{Word}` as `\w`.
    let translated = body
        .replace(r"\A", "^")
        .replace(r"\z", "$")
        .replace(r"\p{Word}", r"\w");
    Regex::new(&translated).ok()
}
