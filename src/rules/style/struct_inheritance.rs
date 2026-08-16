use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;

const MSG: &str = "Don't extend an instance initialized by `Struct.new`. \
                   Use a block to customize the struct.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("class") {
        let Some(superclass) = node.field("superclass") else {
            continue;
        };
        let Some(parent) = super::nodes::children(superclass).into_iter().next() else {
            continue;
        };
        if !is_struct_constructor(context, parent) {
            continue;
        }
        let (Some(keyword), Some(operator)) = (node.child(0), superclass.child(0)) else {
            continue;
        };
        let mut edits = vec![
            Edit {
                start: keyword.start_byte(),
                end: spaces_after(context, keyword.end_byte()),
                replacement: String::new(),
                safe: true,
            },
            Edit {
                start: operator.start_byte(),
                end: operator.end_byte(),
                replacement: "=".to_owned(),
                safe: true,
            },
        ];
        correct_parent(context, node, parent, &mut edits);
        offenses.push(
            context
                .offense(MSG, parent.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `{(send (const {nil? cbase} :Struct) :new ...) (block (send ...) ...)}`. A block is spelled as
/// part of the call it hangs off here, so both readings are the same node.
fn is_struct_constructor(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    if node
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "new")
    {
        return false;
    }
    node.field("receiver")
        .is_some_and(|receiver| match receiver.kind_str() {
            "constant" => context.source.node_text(receiver) == "Struct",
            // `::Struct`, which is the `cbase` half of the pattern.
            "scope_resolution" => {
                receiver.field("scope").is_none()
                    && receiver
                        .field("name")
                        .is_some_and(|name| context.source.node_text(name) == "Struct")
            }
            _ => false,
        })
}

/// `correct_parent`: where the block the struct is customized in has to be opened.
fn correct_parent(
    context: &RuleContext<'_>,
    class_node: Node<'_>,
    parent: Node<'_>,
    edits: &mut Vec<Edit>,
) {
    if let Some(block) = parent.field("block") {
        // The call already carries a block: its `end` closes the class instead.
        let Some(end) = block.child(block.child_count().saturating_sub(1) as u32) else {
            return;
        };
        edits.push(Edit {
            start: spaces_before(context, end.start_byte()),
            end: spaces_after(context, end.end_byte()),
            replacement: String::new(),
            safe: true,
        });
        return;
    }
    // `class_node.body.nil?`: a body holding nothing but comments is no body upstream.
    if class_node
        .field("body")
        .is_none_or(|body| super::nodes::children(body).is_empty())
    {
        edits.push(empty_body_removal(context, class_node, parent));
        return;
    }
    if let Some(arguments) = unparenthesized_arguments(context, parent) {
        let Some(selector) = parent.field("method") else {
            return;
        };
        edits.push(Edit {
            start: selector.end_byte(),
            end: parent.end_byte(),
            replacement: format!("({arguments}) do"),
            safe: true,
        });
        return;
    }
    edits.push(Edit {
        start: parent.end_byte(),
        end: parent.end_byte(),
        replacement: " do".to_owned(),
        safe: true,
    });
}

/// `range_for_empty_class_body`: with no body, the class's own `end` is what has to go.
fn empty_body_removal(context: &RuleContext<'_>, class_node: Node<'_>, parent: Node<'_>) -> Edit {
    if class_node.start_position().row == class_node.end_position().row {
        return Edit {
            start: parent.end_byte(),
            end: class_node.end_byte(),
            replacement: String::new(),
            safe: true,
        };
    }
    let line = class_node.end_position().row + 1;
    let start = context.source.line_start(line);
    let end = context
        .source
        .line_range(line)
        .end
        .min(context.source.len());
    Edit {
        start,
        end,
        replacement: String::new(),
        safe: true,
    }
}

/// `unparenthesized_struct_new?`: the arguments, written as the replacement will spell them.
fn unparenthesized_arguments(context: &RuleContext<'_>, parent: Node<'_>) -> Option<String> {
    let arguments = parent.field("arguments")?;
    if arguments.child(0).is_some_and(|open| open.kind_str() == "(") {
        return None;
    }
    let joined = super::nodes::children(arguments)
        .into_iter()
        .map(|argument| context.source.node_text(argument))
        .collect::<Vec<_>>()
        .join(", ");
    (!joined.is_empty()).then_some(joined)
}

/// `range_with_surrounding_space(side: :right, newlines: false)`.
fn spaces_after(context: &RuleContext<'_>, offset: usize) -> usize {
    support::final_pos(context.source.text(), offset, true, false, false, false)
}

/// `range_with_surrounding_space(side: :left, newlines: false)`.
fn spaces_before(context: &RuleContext<'_>, offset: usize) -> usize {
    support::final_pos(context.source.text(), offset, false, false, false, false)
}
