use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const MSG: &str = "Don't unfreeze interpolated strings as they are already unfrozen.";

/// `minimum_target_ruby_version 3.0`: before that an interpolated string could still be frozen.
const MINIMUM: RubyVersion = RubyVersion::new(3, 0);

/// `on_dstr` with `{(send dstr_type? {:+@ :dup}) (send (const nil? :String) :new dstr_type?)}` on
/// the parent.
///
/// `uninterpolated_string?` and `uninterpolated_heredoc?` between them exempt every literal that
/// holds no interpolation, so what is left is exactly the literals that interpolate.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of_any(&["string", "heredoc_beginning"]) {
        if !interpolates(node, context) {
            continue;
        }
        let Some((range, replaced)) = unfreezing(node, context) else {
            continue;
        };
        offenses.push(context.offense(MSG, range).corrected_by(Edit {
            start: replaced.start,
            end: replaced.end,
            replacement: context.source.node_text(node).to_owned(),
            safe: true,
        }));
    }
}

/// Whether the literal holds a `#{...}` -- which is what upstream's `begin`, `ivar`, `cvar` and
/// `gvar` descendants stand for, since `"#@x"` is an interpolation here as well.
fn interpolates(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let literal = if node.kind_str() == "heredoc_beginning" {
        match send_node::heredoc_body(node, context) {
            Some(body) => body,
            None => return false,
        }
    } else {
        node
    };
    super::nodes::children(literal)
        .iter()
        .any(|child| child.kind_str() == "interpolation")
}

/// The reported range and the range the literal replaces, for the three ways of asking for an
/// unfrozen copy.
fn unfreezing(
    node: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let parent = node.parent()?;
    match parent.kind_str() {
        // `(send dstr_type? :+@)`: the unary plus. `-"#{x}"` is `:-@` and is not one of them.
        "unary" => {
            let operator = parent.field("operator")?;
            if context.source.node_text(operator) != "+" {
                return None;
            }
            if parent.field("operand")?.id() != node.id() {
                return None;
            }
            Some((operator.byte_range(), parent.byte_range()))
        }
        // `(send dstr_type? :dup)`: the literal is the receiver.
        "call" => {
            if parent.field("receiver")?.id() != node.id() {
                return None;
            }
            let selector = parent.field("method")?;
            if context.source.node_text(selector) != "dup" {
                return None;
            }
            Some((
                selector.byte_range(),
                send_node::send_range(parent, context),
            ))
        }
        // `(send (const nil? :String) :new dstr_type?)`: the literal is the only argument.
        "argument_list" => {
            let call = parent.parent()?;
            if call.kind_str() != "call" {
                return None;
            }
            let receiver = call.field("receiver")?;
            // `(const nil? :String)`: a plain `String`, so `::String.new` is not a match.
            if receiver.kind_str() != "constant" || context.source.node_text(receiver) != "String" {
                return None;
            }
            let selector = call.field("method")?;
            if context.source.node_text(selector) != "new" {
                return None;
            }
            match super::nodes::children(parent).as_slice() {
                [only] if only.id() == node.id() => {}
                _ => return None,
            }
            // `node.source_range.begin.join(node.loc.selector)`: `String.new`, without the argument.
            Some((
                call.start_byte()..selector.end_byte(),
                send_node::send_range(call, context),
            ))
        }
        _ => None,
    }
}
