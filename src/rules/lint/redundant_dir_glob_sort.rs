use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;
use crate::ruby_version::RubyVersion;

/// `minimum_target_ruby_version 3.0`: `Dir.glob` returns a sorted list from 3.0 on.
const MINIMUM_VERSION: RubyVersion = RubyVersion::new(3, 0);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM_VERSION {
        return;
    }
    for node in context.nodes_of("call") {
        let (Some(selector), Some(receiver)) = (node.field("method"), node.field("receiver"))
        else {
            continue;
        };
        if context.source.node_text(selector) != "sort" || !is_dir_glob(receiver, context) {
            continue;
        }
        // `sort_with_comparator?`: a block or a `&:x` makes the ordering the author's own.
        if node.field("block").is_some() {
            continue;
        }
        let sort_arguments = arguments(node);
        if sort_arguments
            .last()
            .is_some_and(|last| last.first().kind_str() == "block_argument")
        {
            continue;
        }
        // `multiple_argument?`: `Dir.glob(a, b)` and `Dir.glob(*a)` are not the single-pattern
        // call the guarantee is about.
        let glob_arguments = glob_arguments(receiver);
        if glob_arguments.len() >= 2
            || glob_arguments
                .first()
                .is_some_and(|first| first.kind_str() == "splat_argument")
        {
            continue;
        }
        let Some(dot) = node.field("operator") else {
            continue;
        };
        let range = selector.byte_range();
        offenses.push(
            context
                .offense("Remove redundant `sort`.", range.clone())
                .corrected_by_all([
                    Edit {
                        start: range.start,
                        end: range.end,
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: dot.start_byte(),
                        end: dot.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// `dir_glob?`: the receiver is `Dir.glob(...)` or `Dir[...]`. `short_name` is the last part of
/// the constant, so a `Dir` reached through any namespace counts.
fn is_dir_glob(receiver: Node<'_>, context: &RuleContext<'_>) -> bool {
    let inner = match receiver.kind_str() {
        "call" => {
            let method = receiver.field("method");
            if !method
                .is_some_and(|method| matches!(context.source.node_text(method), "glob" | "[]"))
            {
                return false;
            }
            receiver.field("receiver")
        }
        // `Dir[...]` is a `send` of `:[]` upstream, which the grammar spells as an index.
        "element_reference" => receiver.field("object"),
        _ => return false,
    };
    inner.is_some_and(|inner| short_name(inner, context) == Some("Dir"))
}

/// The arguments the glob was given, whichever of the two spellings it was written in.
fn glob_arguments<'tree>(receiver: Node<'tree>) -> Vec<Node<'tree>> {
    if receiver.kind_str() == "element_reference" {
        let mut cursor = receiver.walk();
        return receiver
            .named_children(&mut cursor)
            .filter(|child| {
                child.kind_str() != "comment"
                    && receiver
                        .field("object")
                        .is_none_or(|object| object.id() != child.id())
            })
            .collect();
    }
    arguments(receiver)
        .into_iter()
        .map(|argument| argument.first())
        .collect()
}

/// `ConstNode#short_name`: the last part of the constant's path.
fn short_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" => node.field("name").map(|name| context.source.node_text(name)),
        _ => None,
    }
}
