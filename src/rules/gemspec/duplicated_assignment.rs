use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{first_line_range, is_plain_send, literal_key, named_children};

use super::support::{enclosing_specification, first_specification_variable};
use crate::rules::node_ext::NodeExt;

/// The names a specification can be reached by besides the block parameter it was opened with:
/// upstream writes them into the pattern itself, so they match whether or not the file opens a
/// specification at all.
const NUMBERED_PARAMETERS: &[&str] = &["_1", "it"];

/// The specification block an assignment was made in, and the target it named. Two assignments
/// are duplicates of one another only when both agree.
type Key = (Option<usize>, String);

/// One assignment, with everything the offense needs once the duplicates are known.
struct Assignment<'tree> {
    node: Node<'tree>,
    /// What the message calls the assignment: `name=` or `metadata["key"]=`.
    name: String,
    key: Key,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let variable = first_specification_variable(context);
    let mut assignments: Vec<Assignment<'_>> = Vec::new();
    for node in context.nodes_of("assignment") {
        let Some(left) = node.field("left") else {
            continue;
        };
        // `spec.a, spec.b = 1, 2` assigns through one node per target.
        let targets = match left.kind_str() {
            "left_assignment_list" => named_children(left),
            _ => vec![left],
        };
        for target in targets {
            if let Some(assignment) = assignment(node, target, variable, context) {
                assignments.push(assignment);
            }
        }
    }

    for group in duplicates(&assignments) {
        let first_line = context.source.line_column(group[0].node.start_byte()).0;
        for assignment in &group[1..] {
            offenses.push(context.offense(
                format!(
                    "`{}` method calls already given on line {first_line} of the gemspec.",
                    assignment.name
                ),
                first_line_range(assignment.node.byte_range(), context),
            ));
        }
    }
}

/// The assignment `target` makes, when it is one the cop groups.
///
/// Upstream looks for `(send (lvar {spec :_1 :it}) _ ...)` filtered by `assignment_method?`, and
/// separately for `(send (send (lvar {...}) _) :[]= literal? _)`. Both reach here as the left-hand
/// side of an assignment, since `spec.name = 'x'` is written as one node rather than as a call to
/// `name=`.
fn assignment<'tree>(
    node: Node<'tree>,
    target: Node<'tree>,
    variable: Option<&str>,
    context: &RuleContext<'_>,
) -> Option<Assignment<'tree>> {
    match target.kind_str() {
        "call" => {
            let receiver = target.field("receiver")?;
            if !is_specification(receiver, variable, context) || !is_plain_send(target, context) {
                return None;
            }
            let method = context
                .source
                .node_text(target.field("method")?);
            Some(Assignment {
                node: assigned_node(node, target),
                name: format!("{method}="),
                key: (
                    enclosing_specification(target, context),
                    format!("{method}="),
                ),
            })
        }
        "element_reference" => {
            let object = target.field("object")?;
            if object.kind_str() != "call" || !is_plain_send(object, context) {
                return None;
            }
            let receiver = object.field("receiver")?;
            if !is_specification(receiver, variable, context) {
                return None;
            }
            let method = context
                .source
                .node_text(object.field("method")?);
            let indices = named_children(target);
            let [index] = indices.get(1..)? else {
                return None;
            };
            let index = *index;
            if !is_literal(index) {
                return None;
            }
            Some(Assignment {
                node: assigned_node(node, target),
                name: format!("{method}[{}]=", context.source.node_text(index)),
                key: (
                    enclosing_specification(target, context),
                    format!("{method}[{}]", literal_key(index, context)),
                ),
            })
        }
        _ => None,
    }
}

/// The node the offense is reported against: the whole assignment, or the target alone when the
/// assignment names more than one.
fn assigned_node<'tree>(node: Node<'tree>, target: Node<'tree>) -> Node<'tree> {
    match node.field("left") == Some(target) {
        true => node,
        false => target,
    }
}

fn is_specification(receiver: Node<'_>, variable: Option<&str>, context: &RuleContext<'_>) -> bool {
    if receiver.kind_str() != "identifier" {
        return false;
    }
    let name = context.source.node_text(receiver);
    Some(name) == variable || NUMBERED_PARAMETERS.contains(&name)
}

/// Groups of assignments that name the same target inside the same specification, in the order
/// their first member was written.
fn duplicates<'a, 'tree>(assignments: &'a [Assignment<'tree>]) -> Vec<Vec<&'a Assignment<'tree>>> {
    let mut groups: Vec<(Key, Vec<&'a Assignment<'tree>>)> = Vec::new();
    for assignment in assignments {
        match groups.iter_mut().find(|(key, _)| *key == assignment.key) {
            Some((_, group)) => group.push(assignment),
            None => groups.push((assignment.key.clone(), vec![assignment])),
        }
    }
    groups
        .into_iter()
        .map(|(_, group)| group)
        .filter(|group| group.len() > 1)
        .collect()
}

/// `node.literal?`: what a `literal?` in a node pattern accepts.
fn is_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "string"
            | "chained_string"
            | "bare_string"
            | "character"
            | "simple_symbol"
            | "delimited_symbol"
            | "integer"
            | "float"
            | "rational"
            | "complex"
            | "true"
            | "false"
            | "nil"
            | "array"
            | "string_array"
            | "symbol_array"
            | "hash"
            | "range"
            | "regex"
            | "subshell"
    )
}
