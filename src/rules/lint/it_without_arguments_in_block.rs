use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::ruby_version::RubyVersion;

use super::locals::LocalVariables;

/// `maximum_target_ruby_version 3.3`: from 3.4 on `it` *is* the first block parameter, so there is
/// nothing left to warn about.
const MAXIMUM_VERSION: RubyVersion = RubyVersion::new(3, 3);

const MSG: &str = "`it` calls without arguments will refer to the first block param in Ruby 3.4; \
                   use `it()` or `self.it`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() > MAXIMUM_VERSION {
        return;
    }
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("identifier") {
        if !is_deprecated_it(node, context, &locals) {
            continue;
        }
        // `each_ancestor(:block).first`: the innermost block, which is the one whose first
        // parameter the name would become.
        let Some(block) = enclosing_block(node, context) else {
            continue;
        };
        // `empty_and_without_delimiters?`: not even a `| |`.
        if block.field("parameters").is_some() {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// `deprecated_it_method?`: a bare `it`, with no receiver, no arguments, no parentheses and no
/// block of its own.
///
/// All four of those turn the name into the `method` of a call node here, so the one shape the cop
/// wants is the identifier that stands on its own -- and a name the parser has seen assigned is an
/// `lvar`, which `on_send` never reaches either.
fn is_deprecated_it(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    if context.source.node_text(node) != "it" || locals.is_lvar(node) {
        return false;
    }
    node.parent_of(context).is_none_or(|parent| match parent.kind_str() {
        "call" => parent
            .field("method")
            .is_none_or(|method| method.id() != node.id()),
        // A name being written to is an `lvasgn` upstream, and `it += 1` reads the variable the
        // assignment declares rather than calling a method.
        "assignment" | "operator_assignment" => parent
            .field("left")
            .is_none_or(|left| left.id() != node.id()),
        _ => true,
    })
}

fn enclosing_block<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "block" | "do_block") {
            return Some(ancestor);
        }
        current = ancestor.parent_of(context);
    }
    None
}
