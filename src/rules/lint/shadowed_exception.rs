use std::cmp::Ordering;
use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::named_children;

use super::exception_hierarchy::{compare, is_exception, is_system_call_error, resolve};
use super::rescue_clause;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not shadow rescued Exceptions.";

/// One `resbody`, as the classes its exception list resolves to. A clause that lists nothing
/// rescues `StandardError`, and a name no constant answers to is `nil`.
type Group = Vec<Option<usize>>;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // Upstream's `rescue` node holds every clause of one body; the clauses that share a parent are
    // the ones that share it.
    let mut clauses: HashMap<usize, Vec<Node<'_>>> = HashMap::new();
    let mut order: Vec<usize> = Vec::new();
    for node in context.nodes_of("rescue") {
        let Some(parent) = node.parent_of(context) else {
            continue;
        };
        clauses
            .entry(parent.id())
            .or_insert_with(|| {
                order.push(parent.id());
                Vec::new()
            })
            .push(node);
    }
    for parent in order {
        let rescues = &clauses[&parent];
        let groups: Vec<Group> = rescues
            .iter()
            .map(|clause| evaluate_exceptions(*clause, context))
            .collect();
        let multiple_levels = groups.iter().any(contains_multiple_levels);
        if !multiple_levels && sorted(&groups) {
            continue;
        }
        let Some(shadowing) = find_shadowing_rescue(&groups, rescues) else {
            continue;
        };
        let body = rescue_clause::body(shadowing);
        offenses.push(context.offense(
            MSG,
            shadowing.start_byte()..rescue_clause::end(shadowing, &body),
        ));
    }
}

/// `evaluate_exceptions`.
fn evaluate_exceptions(clause: Node<'_>, context: &RuleContext<'_>) -> Group {
    let Some(list) = clause.field("exceptions") else {
        return vec![resolve("StandardError")];
    };
    let listed: Vec<Node<'_>> = named_children(list);
    if listed.is_empty() {
        return vec![resolve("StandardError")];
    }
    listed
        .into_iter()
        .map(|exception| resolve(context.source.node_text(exception)))
        .collect()
}

/// `contains_multiple_levels_of_exceptions?`.
fn contains_multiple_levels(group: &Group) -> bool {
    if group.len() > 1 && group.iter().any(|entry| entry.is_some_and(is_exception)) {
        return true;
    }
    for (index, left) in group.iter().enumerate() {
        for right in &group[index + 1..] {
            if compare_exceptions(*left, *right) {
                return true;
            }
        }
    }
    false
}

/// `compare_exceptions`: two `Errno` classes never shadow one another, and an unresolved name
/// never shadows anything.
fn compare_exceptions(left: Option<usize>, right: Option<usize>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    if is_system_call_error(left) && is_system_call_error(right) {
        // `exception.const_get(:Errno) != other.const_get(:Errno) && exception <=> other`: two
        // different `Errno` classes are unrelated, and the same one compares equal to itself.
        return false;
    }
    compare(left, right).is_some()
}

/// `sorted?`.
fn sorted(groups: &[Group]) -> bool {
    groups.windows(2).all(|pair| {
        let (left, right) = (&pair[0], &pair[1]);
        if left.iter().any(|entry| entry.is_some_and(is_exception)) {
            return false;
        }
        if right.iter().any(|entry| entry.is_some_and(is_exception))
            || left.iter().all(Option::is_none)
            || right.iter().all(Option::is_none)
        {
            return true;
        }
        array_compare(left, right).unwrap_or(Ordering::Equal) <= Ordering::Equal
    })
}

/// `Array#<=>`: element by element, then by length, and nothing at all once a pair is unrelated.
fn array_compare(left: &Group, right: &Group) -> Option<Ordering> {
    for (left, right) in left.iter().zip(right.iter()) {
        match (left, right) {
            (None, None) => {}
            (Some(left), Some(right)) => match compare(*left, *right)? {
                Ordering::Equal => {}
                ordering => return Some(ordering),
            },
            _ => return None,
        }
    }
    Some(left.len().cmp(&right.len()))
}

/// `find_shadowing_rescue`.
fn find_shadowing_rescue<'tree>(groups: &[Group], rescues: &[Node<'tree>]) -> Option<Node<'tree>> {
    for (group, clause) in groups.iter().zip(rescues.iter()) {
        if contains_multiple_levels(group) {
            return Some(*clause);
        }
    }
    for (index, pair) in groups.windows(2).enumerate() {
        if !sorted(pair) {
            return rescues.get(index).copied();
        }
    }
    None
}
