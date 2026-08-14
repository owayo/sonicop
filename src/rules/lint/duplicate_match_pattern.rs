use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::ruby_version::RubyVersion;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < RubyVersion::new(2, 7) {
        return;
    }
    for case in context.nodes_of("case_match") {
        let mut cursor = case.walk();
        let mut seen: HashSet<String> = HashSet::new();
        for branch in case.named_children(&mut cursor) {
            if branch.kind_str() != "in_clause" {
                continue;
            }
            let Some(pattern) = branch.field("pattern") else {
                continue;
            };
            if seen.insert(pattern_identity(branch, pattern, context)) {
                continue;
            }
            offenses.push(context.offense(
                "Duplicate `in` pattern detected.",
                pattern.byte_range(),
            ));
        }
    }
}

/// `pattern_identity`: what makes two branches the same test.
///
/// A hash pattern and an alternation are order-independent, so their parts are compared sorted;
/// everything else is compared by source. A guard is part of the test either way.
fn pattern_identity(branch: Node<'_>, pattern: Node<'_>, context: &RuleContext<'_>) -> String {
    let mut identity = match pattern.kind_str() {
        "hash_pattern" | "alternative_pattern" => format!("{:?}", sorted_parts(pattern, context)),
        _ => context.source.node_text(pattern).to_owned(),
    };
    if let Some(guard) = branch.field("guard") {
        identity.push_str(context.source.node_text(guard));
    }
    identity
}

/// `pattern.children.map(&:source).sort`.
///
/// Upstream builds an alternation of three from an alternation of two, so its children are the run
/// up to the last alternative and that last one -- not the flat list the grammar keeps.
fn sorted_parts(pattern: Node<'_>, context: &RuleContext<'_>) -> Vec<String> {
    let mut cursor = pattern.walk();
    let parts: Vec<Node<'_>> = pattern
        .named_children(&mut cursor)
        .filter(|child| child.kind_str() != "comment")
        .collect();
    let mut sources: Vec<String> = if pattern.kind_str() == "alternative_pattern" && parts.len() > 2
    {
        let (last, leading) = parts.split_last().expect("checked to hold three parts");
        let nested = leading[0].start_byte()
            ..leading
                .last()
                .expect("a run of leading alternatives is never empty")
                .end_byte();
        vec![
            context.source.slice(nested).to_owned(),
            context.source.node_text(*last).to_owned(),
        ]
    } else {
        parts
            .iter()
            .map(|part| context.source.node_text(*part).to_owned())
            .collect()
    };
    sources.sort();
    sources
}
