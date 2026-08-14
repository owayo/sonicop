//! `Layout/SpaceBeforeBrackets`: a blank between a receiver and the `[` that indexes it.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

const MSG: &str = "Remove the space before the opening brackets.";

/// Receiver kinds that are values whatever they are called, so `x [1]` indexes them.
const ALWAYS_A_VALUE: &[&str] = &["instance_variable", "global_variable", "class_variable"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["element_reference", "call"]) {
        let Some(range) = gap(node, context) else {
            continue;
        };
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// The `[` a subscript opens with.
fn opening_bracket<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && child.kind_str() == "[")
}

/// The span between the end of the receiver and the `[`, when there is one.
fn gap(node: Node<'_>, context: &RuleContext<'_>) -> Option<Range<usize>> {
    let (receiver_end, bracket_start) = match node.kind_str() {
        "element_reference" => {
            let object = node.field("object")?;
            let bracket = opening_bracket(node)?;
            (object.end_byte(), bracket.start_byte())
        }
        // `a [1]` is indexing to Ruby whenever `a` names a value, but the grammar reads the blank
        // as separating a call from an array argument. Upstream's parser has already decided, so
        // the shape has to be put back: a receiverless call of one array literal whose "name" is a
        // local variable or a variable of another kind.
        "call" => {
            if node.field("receiver").is_some() || node.field("block").is_some() {
                return None;
            }
            let name = node.field("method")?;
            let value = match name.kind_str() {
                "identifier" => names_a_local_variable(name, context),
                kind => ALWAYS_A_VALUE.contains(&kind),
            };
            if !value {
                return None;
            }
            let list = arguments(node);
            let [only] = list.as_slice() else {
                return None;
            };
            let array = only.first();
            if array.kind_str() != "array" {
                return None;
            }
            (name.end_byte(), array.start_byte())
        }
        _ => return None,
    };
    (receiver_end < bracket_start).then_some(receiver_end..bracket_start)
}

/// Whether the name is a local variable at this point, which is what makes Ruby read `a [1]` as
/// indexing rather than as a call taking an array.
///
/// `LocalVariables::is_lvar` cannot answer it: the grammar wrote the name as the *selector* of a
/// call, and `VariableForce` records reads, not selectors. What decides it is the same thing the
/// parser uses -- whether the enclosing scope has already assigned the name here.
fn names_a_local_variable(name: Node<'_>, context: &RuleContext<'_>) -> bool {
    let text = context.source.node_text(name);
    let position = name.start_byte();
    let analysis = context.variable_analysis();
    analysis.variables.iter().any(|variable| {
        variable.name == text
            && variable.declaration.start_byte() < position
            && analysis
                .scopes
                .get(variable.scope)
                .is_some_and(|scope| {
                    scope.node.start_byte() <= position && position <= scope.node.end_byte()
                })
    })
}
