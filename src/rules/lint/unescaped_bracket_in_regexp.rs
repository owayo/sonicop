use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal::string_value;
use crate::rules::send_node::{arguments, is_string, top_level_constant};

use super::regexp_source;
use super::regexp_tree::Tree;

const MSG: &str = "Regular expression has `]` without escape.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("regex") {
        let Some(pattern) = regexp_source::parse(node, context) else {
            continue;
        };
        // The offsets the tree reports are into the pattern, which is where it begins.
        report(&pattern.tree, offenses, context, &|index| {
            let range = pattern.range(index..index + 1);
            (range.start < range.end).then_some(range)
        });
    }
    for node in context.nodes_of("call") {
        let Some(text) = constructed_pattern(node, context) else {
            continue;
        };
        let Some(tree) = super::regexp_tree::parse(&string_value(text, context), false) else {
            continue;
        };
        // `range_at_index` counts from just after the opening quote -- through the *value's*
        // offsets, so `Regexp.new("a\\d]b")` points one character short of its own `]`. That is
        // upstream's arithmetic, and reproducing it is what keeps the columns equal.
        let Some(content) = text.child(0).map(|open| open.end_byte()) else {
            continue;
        };
        report(&tree, offenses, context, &|index| {
            character_at(content, index, context)
        });
    }
}

/// `detect_offenses_in_tree`, with where a character of the pattern sits left to the caller.
fn report(
    tree: &Tree,
    offenses: &mut Vec<Offense>,
    context: &RuleContext<'_>,
    locate: &dyn Fn(usize) -> Option<Range<usize>>,
) {
    let mut skip_class_closer = false;
    for index in tree.expressions() {
        let expression = &tree.nodes[index];
        // `[^]` and `[]` are an empty set, and the `]` the scan stopped at is reported after it as
        // a literal of its own. Ruby reads that one as closing the class, so it is let through.
        if expression.kind == "set" && expression.children.is_empty() {
            skip_class_closer = true;
            continue;
        }
        if expression.kind != "literal" {
            continue;
        }
        for position in unescaped_brackets(&expression.text) {
            if skip_class_closer {
                skip_class_closer = false;
                continue;
            }
            // Ruby does not warn about a `]` opening the pattern, which cannot be read as closing
            // anything.
            if expression.ts + position == 0 {
                continue;
            }
            let Some(range) = locate(expression.ts + position) else {
                continue;
            };
            offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: "\\]".to_owned(),
                safe: true,
            }));
        }
    }
}

/// `text.scan(/(?<!\\)\]/)`, in characters.
fn unescaped_brackets(text: &str) -> Vec<usize> {
    let characters: Vec<char> = text.chars().collect();
    (0..characters.len())
        .filter(|&index| characters[index] == ']')
        .filter(|&index| index == 0 || characters[index - 1] != '\\')
        .collect()
}

/// `regexp_constructor`: the string a `Regexp.new` or `Regexp.compile` in this call is built from.
///
/// Upstream searches the whole subtree, so a constructor nested in another call is found twice and
/// deduplicated by range. Reaching it from its own `on_send` finds it once, and the guard against
/// interpolation ends up asking the same question of the same node either way.
fn constructed_pattern<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let method = context.source.node_text(node.field("method")?);
    if !matches!(method, "new" | "compile") {
        return None;
    }
    if !top_level_constant(node.field("receiver")?, "Regexp", context) {
        return None;
    }
    if holds_interpolation(node, context) {
        return None;
    }
    let first = arguments(node).first()?.first();
    // A `?a` is a `str` upstream as well, but it has no quote to count from, and a heredoc makes
    // upstream itself raise. Only what the pattern can be read out of is looked at.
    (first.kind_str() == "string" && is_string(first, context)).then_some(first)
}

/// `node.each_descendant(:dstr).any?`: anything upstream's parser builds a `dstr` out of.
fn holds_interpolation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        let interpolates = match current.kind_str() {
            "string" => !is_string(current, context),
            "chained_string" | "heredoc_beginning" => true,
            _ => false,
        };
        if interpolates && current.id() != node.id() {
            return true;
        }
        crate::rules::push_named_children(current, &mut stack);
    }
    false
}

/// The byte range of the character standing `offset` characters after `start`.
fn character_at(start: usize, offset: usize, context: &RuleContext<'_>) -> Option<Range<usize>> {
    let (index, character) = context.source.text()[start..].char_indices().nth(offset)?;
    Some(start + index..start + index + character.len_utf8())
}
