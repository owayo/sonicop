//! `Layout/DefEndAlignment`.

use std::collections::HashSet;

use tree_sitter::Node;

use super::support::{character_column, end_keyword, end_keyword_alignment};
use crate::diagnostic::Offense;
use crate::rules::{RuleContext, push_named_children};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // A definition passed to `private` lines its `end` up with the modifier by default.
    let align_with_def = context
        .setting::<String>("EnforcedStyleAlignWith")
        .as_deref()
        == Some("def");
    let mut ignored: HashSet<usize> = HashSet::new();
    let mut stack = vec![context.root_node()];
    while let Some(node) = stack.pop() {
        match node.kind_str() {
            "method" | "singleton_method" => {
                if !ignored.contains(&node.id())
                    && let Some(offense) = check_definition(
                        context,
                        node,
                        node.child(0)
                            .map_or_else(|| node.byte_range(), |keyword| keyword.byte_range()),
                        character_column(context, node.start_byte()),
                    )
                {
                    offenses.push(offense);
                }
            }
            // `private def foo`: the modifier and the definition are one line, and the `end` is
            // measured against whichever of the two the style names.
            "call" => {
                let Some(definition) = def_modifier(node) else {
                    continue;
                };
                let Some(keyword) = definition.child(0) else {
                    continue;
                };
                let (base, column) = match align_with_def {
                    true => (
                        keyword.byte_range(),
                        character_column(context, definition.start_byte()),
                    ),
                    false => (
                        node.start_byte()..keyword.end_byte(),
                        character_column(context, node.start_byte()),
                    ),
                };
                if let Some(offense) = check_definition(context, definition, base, column) {
                    offenses.push(offense);
                }
                ignored.insert(definition.id());
            }
            _ => {}
        }
        push_named_children(node, &mut stack);
    }
}

fn check_definition(
    context: &RuleContext<'_>,
    definition: Node<'_>,
    base: std::ops::Range<usize>,
    column: i64,
) -> Option<Offense> {
    let end = end_keyword(definition)?;
    end_keyword_alignment(context, end.byte_range(), base, column)
}

/// `def_modifier`: the definition a chain of bare calls such as `private public def foo` wraps.
fn def_modifier<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = call;
    loop {
        if current.field("receiver").is_some() {
            return None;
        }
        let arguments = current.field("arguments")?;
        let argument = arguments.named_child(0)?;
        if matches!(argument.kind_str(), "method" | "singleton_method") {
            return Some(argument);
        }
        if argument.kind_str() != "call" {
            return None;
        }
        current = argument;
    }
}
