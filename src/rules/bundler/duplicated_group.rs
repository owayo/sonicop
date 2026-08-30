use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, first_line_range, send_range};

use super::support::declarations;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

/// `SOURCE_BLOCK_NAMES`. A group declared under a different source, git remote, platform or path is
/// a different group even when it goes by the same name.
const SOURCE_BLOCK_NAMES: &[&str] = &["source", "git", "platforms", "path"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut groups: Vec<(String, Vec<Node<'_>>)> = Vec::new();
    for node in declarations(context, "group") {
        let key = format!("{}{}", source_key(node, context), attributes(node, context));
        match groups.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, group)) => group.push(node),
            None => groups.push((key, vec![node])),
        }
    }

    for (_, group) in groups.iter().filter(|(_, group)| group.len() > 1) {
        let first_line = context.source.line_column(group[0].start_byte()).0;
        for node in &group[1..] {
            let name = arguments(*node)
                .iter()
                .map(|argument| context.source.slice(argument.range()).to_owned())
                .collect::<Vec<_>>()
                .join(", ");
            offenses.push(context.offense(
                format!("Gem group `{name}` already defined on line {first_line} of the Gemfile."),
                first_line_range(send_range(*node, context), context),
            ));
        }
    }
}

/// `find_source_key`: the innermost `source`/`git`/`platforms`/`path` block the group is written
/// in, named by that block's method and its first argument.
fn source_key(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let mut child = node;
    while let Some(parent) = child.parent_of(context) {
        if matches!(child.kind_str(), "do_block" | "block")
            && parent.kind_str() == "call"
            && let Some(method) = parent.field("method")
        {
            let method = context.source.node_text(method);
            if SOURCE_BLOCK_NAMES.contains(&method) {
                let argument = arguments(parent)
                    .first()
                    .map(|argument| context.source.slice(argument.range()).to_owned())
                    .unwrap_or_default();
                return format!("{method}{argument}");
            }
        }
        child = parent;
    }
    String::new()
}

/// `group_attributes`, joined the way upstream builds its grouping key: sorted, and with nothing
/// between them.
fn attributes(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let mut attributes: Vec<String> = arguments(node)
        .iter()
        .map(|argument| {
            let parts = argument.parts();
            // A hash argument is keyed by its pairs rather than by its own source, so that
            // `group :a, foo: 1` and `group :a, foo: 1` agree however they were spaced.
            if parts.len() > 1 || parts[0].kind_str() == "hash" {
                let mut pairs: Vec<String> = match parts[0].kind_str() == "hash" {
                    true => named_children_of(parts[0], context),
                    false => parts.to_vec(),
                }
                .into_iter()
                .map(|pair| context.source.node_text(pair).to_owned())
                .collect();
                pairs.sort();
                return pairs.join(", ");
            }
            literal_value(parts[0], context)
        })
        .collect();
    attributes.sort();
    attributes.concat()
}

/// `argument.respond_to?(:value) ? argument.value.to_s : argument.source`: a basic literal is
/// keyed by what it holds, so `group :test` and `group 'test'` are the same group.
fn literal_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    use crate::rules::send_node::{is_string, string_text, symbol_name};

    if let Some(name) = symbol_name(node, context) {
        return name.to_owned();
    }
    if is_string(node, context) {
        return string_text(node, context).to_owned();
    }
    if matches!(node.kind_str(), "integer" | "float" | "rational" | "complex") {
        return context.source.node_text(node).replace('_', "");
    }
    context.source.node_text(node).to_owned()
}
