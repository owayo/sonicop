//! Defaulted parameters, unfolded from the shape tree-sitter sometimes gives them.
//!
//! A run of defaulted parameters is occasionally parsed as a single `optional_parameter` whose
//! value is a chain of multiple assignments: `def tag(name = nil, options = nil)` comes out as
//! `name = (nil, options = nil)`. Each `left_assignment_list` in the chain holds the previous
//! parameter's default followed by the name the fold swallowed, so unwinding it recovers the pairs
//! the source actually spells out. `Metrics/ParameterLists` counts the same fold.

use std::collections::VecDeque;

use tree_sitter::Node;

/// One defaulted parameter: the name it declares and the expression it defaults to.
pub(super) struct Defaulted<'tree> {
    pub(super) name: Node<'tree>,
    pub(super) value: Node<'tree>,
}

/// A `keyword_parameter`, which is only a defaulted parameter when a default was written: `k:`
/// with nothing after it is a required keyword argument.
pub(super) fn keyword<'tree>(parameter: Node<'tree>) -> Option<Defaulted<'tree>> {
    Some(Defaulted {
        name: parameter.child_by_field_name("name")?,
        value: parameter.child_by_field_name("value")?,
    })
}

/// The parameters one `optional_parameter` node really stands for.
pub(super) fn defaulted<'tree>(parameter: Node<'tree>) -> Vec<Defaulted<'tree>> {
    let (Some(name), Some(value)) = (
        parameter.child_by_field_name("name"),
        parameter.child_by_field_name("value"),
    ) else {
        return Vec::new();
    };
    let mut pending: VecDeque<Node<'tree>> = VecDeque::from([name]);
    let mut parameters: Vec<Defaulted<'tree>> = Vec::new();
    let mut current = value;
    loop {
        let folded = (current.kind() == "assignment")
            .then(|| current.child_by_field_name("left"))
            .flatten()
            .filter(|left| left.kind() == "left_assignment_list");
        let Some(left) = folded else {
            if let Some(name) = pending.pop_front() {
                parameters.push(Defaulted {
                    name,
                    value: current,
                });
            }
            return parameters;
        };
        let mut cursor = left.walk();
        let targets: Vec<Node<'tree>> = left.named_children(&mut cursor).collect();
        let Some((default, names)) = targets.split_first() else {
            return parameters;
        };
        if let Some(name) = pending.pop_front() {
            parameters.push(Defaulted {
                name,
                value: *default,
            });
        }
        pending.extend(names.iter().copied());
        let Some(right) = current.child_by_field_name("right") else {
            return parameters;
        };
        current = right;
    }
}
