use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{
    first_line_range, literal_key, named_children, send_range, string_text,
};

use super::support::gem_declarations;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut groups: Vec<(String, Vec<(Node<'_>, Node<'_>)>)> = Vec::new();
    for (node, name) in gem_declarations(context) {
        let key = literal_key(name, context);
        match groups.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, group)) => group.push((node, name)),
            None => groups.push((key, vec![(node, name)])),
        }
    }

    for (_, group) in groups.iter().filter(|(_, group)| group.len() > 1) {
        let nodes: Vec<Node<'_>> = group.iter().map(|(node, _)| *node).collect();
        if conditional(&nodes) {
            continue;
        }
        let first_line = context.source.line_column(nodes[0].start_byte()).0;
        for (node, name) in &group[1..] {
            offenses.push(context.offense(
                format!(
                    "Gem `{}` requirements already given on line {first_line} of the Gemfile.",
                    string_text(*name, context)
                ),
                first_line_range(send_range(*node, context), context),
            ));
        }
    }
}

/// `conditional_declaration?`: a gem declared once per branch of the same conditional is declared
/// once, so the branches have to hold every one of the declarations for the group to be excused.
fn conditional(nodes: &[Node<'_>]) -> bool {
    let Some(parent) = statement_parent(nodes[0]) else {
        return false;
    };
    let root = match parent.kind_str() {
        "if" | "elsif" | "unless" | "if_modifier" | "unless_modifier" | "conditional" => parent,
        // `parent.parent` upstream: a `when` is a part of the `case` that owns every branch.
        "when" => match parent.parent() {
            Some(case) => case,
            None => return false,
        },
        _ => return false,
    };
    let branches = branches(root);
    nodes.iter().all(|node| {
        node.parent()
            .is_some_and(|parent| branches.iter().any(|branch| branch.id() == parent.id()))
    })
}

/// The first ancestor that is not a statement sequence, which is what
/// `each_ancestor.find { |ancestor| !ancestor.begin_type? }` reaches for.
///
/// Upstream's branches hold either a single statement or a `begin` of several; tree-sitter always
/// writes the `then` or `else` around them, so exactly those two stand in for the `begin`.
fn statement_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut parent = node.parent()?;
    while matches!(parent.kind_str(), "then" | "else") {
        parent = parent.parent()?;
    }
    Some(parent)
}

/// The nodes a conditional's branches are written inside, in the sense `within_conditional?` asks
/// about: a declaration belongs to a branch when it is the branch itself or a statement of it,
/// which here is the same as being written directly inside one of these.
fn branches<'tree>(root: Node<'tree>) -> Vec<Node<'tree>> {
    match root.kind_str() {
        // The body of a modifier form and both arms of a ternary hang straight off the node.
        "if_modifier" | "unless_modifier" | "conditional" => vec![root],
        "if" | "elsif" | "unless" => {
            let mut branches = Vec::new();
            branches.extend(root.field("consequence"));
            // `branches` flattens an `elsif` chain, so every arm of the whole chain counts as a
            // branch of the conditional it started.
            match root.field("alternative") {
                Some(alternative) if alternative.kind_str() == "elsif" => {
                    branches.extend(self::branches(alternative));
                }
                Some(alternative) => branches.push(alternative),
                None => {}
            }
            branches
        }
        "case" => named_children(root)
            .into_iter()
            .filter_map(|child| match child.kind_str() {
                "when" => child.field("body"),
                "else" => Some(child),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
