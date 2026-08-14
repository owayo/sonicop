use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 3.1`: the level argument to `File.dirname` landed in 3.1, so before
/// that the replacement does not exist and the cop has nothing to suggest.
const MINIMUM: RubyVersion = RubyVersion::new(3, 1);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    for node in context.nodes_of("call") {
        // `RESTRICT_ON_SEND = %i[dirname]` is the whole entry condition upstream: the receiver of
        // the *outer* call is never checked, only the name. That is why
        // `Foo::File.dirname(File.dirname(path))` is reported too -- what has to be `File.dirname`
        // is the argument, not the call being replaced.
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "dirname" {
            continue;
        }
        // Only the outermost call is reported: an inner one sits under a call of the same shape,
        // whose offense already covers it.
        if enclosing_call(node).is_some_and(|parent| is_file_dirname(parent, context)) {
            continue;
        }
        // `path_with_dir_level`: peeling one `File.dirname` off the argument goes a level deeper,
        // and the argument that is no longer one is the path.
        let mut level = 1_usize;
        let mut current = node;
        let path = loop {
            let Some(argument) = first_argument(current) else {
                break None;
            };
            if is_file_dirname(argument, context) {
                level += 1;
                current = argument;
            } else {
                break Some(argument);
            }
        };
        let Some(path) = path else {
            continue;
        };
        // A single `File.dirname(path)` is what the code should say already.
        if level < 2 {
            continue;
        }
        // `offense_range`: the selector through the end of the call, so whatever receiver was
        // written stays and the replacement reads as `File.dirname(path, 2)`.
        let range = selector.start_byte()..node.end_byte();
        let replacement = format!("dirname({}, {level})", context.source.node_text(path));
        offenses.push(
            context
                .offense(format!("Use `{replacement}` instead."), range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `(send (const {cbase nil?} :File) :dirname ...)`.
fn is_file_dirname(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    let (Some(receiver), Some(method)) = (node.field("receiver"), node.field("method")) else {
        return false;
    };
    super::nodes::is_top_level_constant(receiver, "File", context)
        && context.source.node_text(method) == "dirname"
}

/// The call this one is an argument of.
///
/// Upstream's `node.parent` is the enclosing `send`, because its AST hangs arguments straight off
/// the call. Here an `argument_list` sits in between, so one more step is needed to land on the
/// same node.
fn enclosing_call<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    if parent.kind_str() == "argument_list" {
        parent.parent()
    } else {
        Some(parent)
    }
}

/// `node.first_argument`.
fn first_argument<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("arguments")
        .and_then(|arguments| super::nodes::children(arguments).into_iter().next())
}
