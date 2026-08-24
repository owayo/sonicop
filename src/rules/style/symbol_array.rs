use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

use super::literal::{node_value, to_string_literal, to_symbol_literal, trim_interpolation_escape};
use super::nodes;
use super::percent_array::{
    Bracketed, Element, allowed_bracket_array, bracketed_replacement, elements,
    percent_array_offense, percent_replacement, percent_values,
};
use crate::rules::node_ext::NodeExt;

const PERCENT_MSG: &str = "Use `%i` or `%I` for an array of symbols.";
const ARRAY_MSG: &str = "Use %<prefer>s for an array of symbols.";

/// The characters `complex_content?` refuses to leave inside a percent literal.
const DELIMITERS: [char; 4] = ['[', ']', '(', ')'];

/// `minimum_target_ruby_version 2.0`: `%i[…]` arrived in 2.0.
const MINIMUM: RubyVersion = RubyVersion::new(2, 0);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "percent".to_owned());

    for node in context.nodes_of("array") {
        let items = nodes::children(node);
        if items.is_empty() || !items.iter().all(|item| is_symbol(context, *item)) {
            continue;
        }
        let Some(values) = values(context, &items) else {
            continue;
        };
        let sources: Vec<&str> = items
            .iter()
            .map(|item| context.source.node_text(*item))
            .collect();
        if complex_content(&sources, &values, &vec![false; items.len()]) {
            continue;
        }
        let array = Bracketed { node, items };
        if allowed_bracket_array(context, &array) || style != "percent" {
            continue;
        }
        let replacement = percent_replacement(context, &array, 'i', &values);
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

    for node in context.nodes_of("symbol_array") {
        let items = elements(context, node);
        let values: Vec<String> = percent_values(context, node, &items)
            .into_iter()
            .map(|decoded| decoded.value)
            .collect();
        let text = context.source.text();
        let sources: Vec<&str> = items.iter().map(|item| &text[item.range.clone()]).collect();
        let interpolations: Vec<bool> = items.iter().map(|item| item.interpolated).collect();
        let brackets_required = complex_content(&sources, &values, &interpolations);
        if style != "brackets" && !brackets_required {
            continue;
        }
        let written: Vec<String> = items
            .iter()
            .zip(&values)
            .map(|(item, value)| symbol_source(&text[item.range.clone()], item, value))
            .collect();
        let bracketed = bracketed_replacement(context, node, &items, &written);
        offenses.push(percent_array_offense(context, node, ARRAY_MSG, bracketed));
    }
}

/// `bracketed_array_of?(:sym, node)`: an element is a symbol unless an interpolation makes it a
/// `dsym`.
fn is_symbol(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    matches!(node.kind_str(), "simple_symbol" | "delimited_symbol")
        && !interpolated(node)
        && !context.source.node_text(node).contains('\n')
}

fn interpolated(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind_str() == "interpolation")
}

fn values(context: &RuleContext<'_>, items: &[Node<'_>]) -> Option<Vec<String>> {
    items
        .iter()
        .map(|item| node_value(context, *item).map(|decoded| decoded.value))
        .collect()
}

/// `complex_content?`: a name the percent form cannot spell, because it holds a blank or a bracket
/// that does not close.
///
/// An element whose whole source is one bracket -- which `%i[[ ]]` produces -- makes upstream give
/// up on the literal entirely rather than skip that one element.
fn complex_content(sources: &[&str], values: &[String], interpolations: &[bool]) -> bool {
    for ((source, value), interpolated) in sources.iter().zip(values).zip(interpolations) {
        if source.chars().count() == 1 && DELIMITERS.contains(&source.chars().next().unwrap_or(' '))
        {
            return false;
        }
        // A `dsym` contributes the source of its parts rather than a name it does not have.
        let content = match interpolated {
            true => *source,
            false => value.as_str(),
        };
        let stripped = strip_delimiter_pairs(content);
        if content.contains(' ')
            || DELIMITERS
                .iter()
                .any(|delimiter| stripped.contains(*delimiter))
        {
            return true;
        }
    }
    false
}

/// `content.gsub(/(\[[^\s\[\]]*\])|(\([^\s()]*\))/, '')`: a bracket pair holding nothing but a name
/// does not count against the literal.
fn strip_delimiter_pairs(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let characters: Vec<char> = content.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        let closing = match characters[index] {
            '[' => Some(']'),
            '(' => Some(')'),
            _ => None,
        };
        if let Some(closing) = closing {
            let opening = characters[index];
            let mut scan = index + 1;
            while scan < characters.len()
                && characters[scan] != closing
                && characters[scan] != opening
                && !characters[scan].is_whitespace()
            {
                scan += 1;
            }
            if scan < characters.len() && characters[scan] == closing {
                index = scan + 1;
                continue;
            }
        }
        out.push(characters[index]);
        index += 1;
    }
    out
}

/// `build_bracketed_array`'s per-element source: a `dsym` keeps its written form, a plain symbol is
/// written back from its name.
fn symbol_source(source: &str, item: &Element, value: &str) -> String {
    if item.interpolated {
        let literal = to_string_literal(source);
        return format!(":{}", trim_interpolation_escape(&literal));
    }
    to_symbol_literal(value)
}
