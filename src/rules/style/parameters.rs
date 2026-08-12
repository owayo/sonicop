//! A definition's parameter list, as upstream's parser holds it.
//!
//! The grammar reads `def m(a = nil, b = false)` as one optional parameter whose default is the
//! multiple assignment `nil, b = false`, because `nil` is spelled the same as an assignment target
//! there. Upstream's parser has two `optarg`s, so a cop that walks the list has to put the
//! parameters back together before it can answer anything about them.

use std::ops::Range;

use tree_sitter::Node;

/// One parameter, which for the misread run above has no node of its own.
pub(super) struct Parameter<'tree> {
    /// The node kind upstream's parser would have produced.
    pub(super) kind: &'static str,
    pub(super) name: Option<Node<'tree>>,
    pub(super) value: Option<Node<'tree>>,
    pub(super) range: Range<usize>,
}

/// The parameters of a `method_parameters`, `parameters`, `block_parameters` or
/// `lambda_parameters` node, in source order.
pub(super) fn parameters<'tree>(list: Node<'tree>) -> Vec<Parameter<'tree>> {
    let mut found = Vec::new();
    for child in super::nodes::children(list) {
        if child.kind() == "optional_parameter"
            && let Some(expanded) = split_misread_defaults(child)
        {
            found.extend(expanded);
            continue;
        }
        found.push(Parameter {
            kind: child.kind(),
            name: child.child_by_field_name("name"),
            value: child.child_by_field_name("value"),
            range: child.byte_range(),
        });
    }
    found
}

/// Splits the run of optional parameters the grammar folded into one, or `None` when the parameter
/// really does default to an assignment.
fn split_misread_defaults<'tree>(parameter: Node<'tree>) -> Option<Vec<Parameter<'tree>>> {
    let name = parameter.child_by_field_name("name")?;
    let value = parameter.child_by_field_name("value")?;
    if value.kind() != "assignment" {
        return None;
    }
    // The misreading always spells the left side as a list of two: the default that belongs to the
    // parameter before the comma, and the name of the one after it. `def m(x = y = 1)` assigns for
    // real and has a single target, which is what tells the two apart.
    let mut written = Vec::new();
    let mut current = value;
    loop {
        let left = current.child_by_field_name("left")?;
        if left.kind() != "left_assignment_list" {
            return None;
        }
        let targets = super::nodes::children(left);
        if targets.len() != 2 {
            return None;
        }
        written.extend(targets);
        let right = current.child_by_field_name("right")?;
        if right.kind() == "assignment"
            && right
                .child_by_field_name("left")
                .is_some_and(|inner| inner.kind() == "left_assignment_list")
        {
            current = right;
            continue;
        }
        written.push(right);
        break;
    }

    let mut found = vec![Parameter {
        kind: "optional_parameter",
        name: Some(name),
        value: Some(written[0]),
        range: name.start_byte()..written[0].end_byte(),
    }];
    for pair in written[1..].chunks(2) {
        let [name, value] = pair else {
            return None;
        };
        found.push(Parameter {
            kind: "optional_parameter",
            name: Some(*name),
            value: Some(*value),
            range: name.start_byte()..value.end_byte(),
        });
    }
    Some(found)
}
