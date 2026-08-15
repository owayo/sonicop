use std::collections::HashSet;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::regexp_source;
use super::regexp_tree::{Expression, Tree};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("regex") {
        let Some(pattern) = regexp_source::parse(node, context) else {
            continue;
        };
        // What an interpolation expands to is unknown, so whether the quantifier around it repeats
        // anything is unknown too.
        if !pattern.interpolations.is_empty() {
            continue;
        }
        let tree = &pattern.tree;
        // A group reached from inside another one was already paired with everything under it.
        let mut seen: HashSet<usize> = HashSet::new();
        for index in tree.expressions() {
            if seen.contains(&index) || !redundant_group(tree, index) {
                continue;
            }
            let Some(outer) = mergeable(&tree.nodes[index]) else {
                continue;
            };
            for &inner in tree.subtree(index).iter().skip(1) {
                seen.insert(inner);
                // The walk stops at the first thing a quantifier could not have been lifted
                // through, since nothing below it repeats what the group repeats.
                if !redundantly_quantifiable(tree, inner) {
                    break;
                }
                let Some(replacement) = mergeable(&tree.nodes[inner]) else {
                    continue;
                };
                report(
                    context,
                    offenses,
                    &pattern,
                    index,
                    inner,
                    outer,
                    replacement,
                );
            }
        }
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    pattern: &regexp_source::Pattern,
    group: usize,
    child: usize,
    outer: &str,
    inner: &str,
) {
    let (Some(group_quantifier), Some(child_quantifier)) = (
        pattern.tree.nodes[group].quantifier.as_ref(),
        pattern.tree.nodes[child].quantifier.as_ref(),
    ) else {
        return;
    };
    // `(?:a+)+` is `(?:a+)`, while every mixed pair collapses to `*`.
    let merged = match outer == inner {
        true => outer,
        false => "*",
    };
    let outer_range = pattern.range(group_quantifier.ts..group_quantifier.te);
    let inner_range = pattern.range(child_quantifier.ts..child_quantifier.te);
    let message = format!(
        "Replace redundant quantifiers `{}` and `{}` with a single `{merged}`.",
        child_quantifier.text, group_quantifier.text
    );
    offenses.push(
        context
            .offense(message, inner_range.start..outer_range.end)
            .corrected_by_all([
                Edit {
                    start: outer_range.start,
                    end: outer_range.end,
                    replacement: String::new(),
                    safe: true,
                },
                Edit {
                    start: inner_range.start,
                    end: inner_range.end,
                    // `{1,}` and `+` mean the same thing, and the merge writes the short one.
                    replacement: merged.to_owned(),
                    safe: true,
                },
            ]),
    );
}

/// `redundant_group?`: `(?:…)` around a single expression, which its quantifier therefore repeats
/// no more than that expression's own quantifier does.
fn redundant_group(tree: &Tree, index: usize) -> bool {
    let node = &tree.nodes[index];
    node.kind == "group"
        && node.token == "passive"
        && node
            .children
            .iter()
            .filter(|&&child| tree.nodes[child].kind != "free_space")
            .count()
            == 1
}

/// `redundantly_quantifiable?`.
fn redundantly_quantifiable(tree: &Tree, index: usize) -> bool {
    let node = &tree.nodes[index];
    redundant_group(tree, index)
        || (node.kind == "set" && node.token == "character")
        || node.terminal()
}

/// `mergeable_quantifier`: the one-character spelling of a greedy quantifier, if it has one.
///
/// A lazy or possessive quantifier is left alone -- merging those changes what the pattern
/// matches, and Ruby does not warn about them either.
fn mergeable(node: &Expression) -> Option<&'static str> {
    let quantifier = node.quantifier.as_ref()?;
    if !quantifier.greedy {
        return None;
    }
    match (quantifier.min, quantifier.max) {
        (0, -1) => Some("*"),
        (0, 1) => Some("?"),
        (1, -1) => Some("+"),
        _ => None,
    }
}
