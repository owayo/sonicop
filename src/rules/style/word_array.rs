use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::literal::{Decoded, node_value, to_string_literal, trim_interpolation_escape};
use super::nodes;
use super::percent_array::{
    Bracketed, Element, allowed_bracket_array, bracketed_replacement, elements,
    percent_array_offense, percent_replacement, percent_values,
};
use crate::rules::node_ext::NodeExt;

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
    // Upstream caches this for the same reason: without it every row of a matrix re-reads the whole
    // matrix, which turns a table of literals into quadratic work.
    let mut matrix = HashMap::new();

    for node in context.nodes_of("array") {
        let items = nodes::children(node);
        if items.is_empty() || !items.iter().all(|item| is_word(context, *item)) {
            continue;
        }
        let values = values(context, &items);
        if complex_content(&values, word)
            || within_matrix_of_complex_content(context, node, word, &mut matrix)
        {
            continue;
        }
        let array = Bracketed { node, items };
        if allowed_bracket_array(context, &array) || style != "percent" {
            continue;
        }
        let contents: Vec<String> = values
            .iter()
            .map(|value| value.as_ref().map_or_else(String::new, |v| v.value.clone()))
            .collect();
        let replacement = percent_replacement(context, &array, 'w', &contents);
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
        let decoded = percent_values(context, node, &items);
        // An interpolated word is a `dstr`, whose `str_content` is nil and which upstream therefore
        // never measures.
        let values: Vec<Option<Decoded>> = items
            .iter()
            .zip(&decoded)
            .map(|(item, value)| match item.interpolated {
                true => None,
                false => Some(Decoded {
                    value: value.value.clone(),
                    valid: value.valid,
                }),
            })
            .collect();
        // `invalid_percent_array_contents?` drops the word test: only a blank forces brackets.
        let brackets_required = complex_content(&values, None);
        if style != "brackets" && !brackets_required {
            continue;
        }
        let text = context.source.text();
        let written: Vec<String> = items
            .iter()
            .zip(&decoded)
            .map(|(item, value)| word_source(&text[item.range.clone()], item, &value.value))
            .collect();
        let bracketed = bracketed_replacement(context, node, &items, &written);
        offenses.push(percent_array_offense(context, node, ARRAY_MSG, bracketed));
    }
}

/// `bracketed_array_of?(:str, node)`: a plain string, so neither interpolated nor split across
/// lines, both of which upstream's parser turns into a `dstr`. A `?a` character literal is a `str`
/// there too.
fn is_word(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "character" => true,
        "string" => !interpolated(node) && !context.source.node_text(node).contains('\n'),
        _ => false,
    }
}

fn interpolated(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind_str() == "interpolation")
}

/// The values upstream measures, with a `None` where `str_content` is nil -- anything that is not a
/// plain string, which `complex_content?` skips rather than judges.
fn values(context: &RuleContext<'_>, items: &[Node<'_>]) -> Vec<Option<Decoded>> {
    items
        .iter()
        .map(|item| match is_word(context, *item) {
            true => node_value(context, *item),
            false => None,
        })
        .collect()
}

/// `complex_content?`: a word that does not look like one, holds a blank, or is not text at all.
fn complex_content(values: &[Option<Decoded>], word: Option<&Regex>) -> bool {
    values.iter().flatten().any(|decoded| {
        !decoded.valid
            || word.is_some_and(|pattern| !pattern.is_match(&decoded.value))
            || decoded.value.contains(' ')
    })
}

/// `within_matrix_of_complex_content?`: a row of a table whose other rows hold phrases keeps its
/// brackets, so that the table stays readable as a whole.
fn within_matrix_of_complex_content(
    context: &RuleContext<'_>,
    node: Node<'_>,
    word: Option<&Regex>,
    cache: &mut HashMap<usize, bool>,
) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    if parent.kind_str() != "array" {
        return false;
    }
    if let Some(known) = cache.get(&parent.id()) {
        return *known;
    }
    let rows = nodes::children(parent);
    let matrix = rows.iter().all(|row| row.kind_str() == "array")
        && rows
            .iter()
            .any(|row| complex_content(&values(context, &nodes::children(*row)), word));
    cache.insert(parent.id(), matrix);
    matrix
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
fn word_regex(context: &RuleContext<'_>) -> Option<&'static Regex> {
    let Some(configured) = context.setting::<String>("WordRegex") else {
        return Some(&DEFAULT_WORD);
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
    crate::rules::regex_cache::compiled(&translated)
}
