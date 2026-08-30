use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG_MISSING: &str = "Use def with parentheses when there are parameters.";
const MSG_PRESENT: &str = "Use def without parentheses.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "require_parentheses".to_owned());
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.field("parameters") else {
            continue;
        };
        let parenthesized = context.source.node_text(parameters).starts_with('(');
        let require_parentheses = style == "require_parentheses"
            || (style == "require_no_parentheses_except_multiline"
                && multiline(context, parameters));
        if require_parentheses {
            // `node.arguments?`: an empty pair of parentheses declares no parameter at all, so
            // there is nothing to wrap.
            if !parenthesized && !super::nodes::children_in(parameters, context).is_empty() {
                offenses.push(missing(context, parameters));
            }
            continue;
        }
        // `forced_parentheses?`: an endless definition and an anonymous parameter both need the
        // parentheses whatever the style asks for.
        if parenthesized && !forced(node, parameters) {
            offenses.push(unwanted(context, parameters));
        }
    }
}

/// `add_parentheses` for an `args` node: the space before the parameters becomes the opening
/// parenthesis, and the closing one is written after them.
fn missing(context: &RuleContext<'_>, parameters: Node<'_>) -> Offense {
    let range = parameters.byte_range();
    let leading = super::ranges::extended_left(context.source.text(), range.start, true);
    context
        .offense(MSG_MISSING, range.clone())
        .corrected_by_all([
            Edit {
                start: leading,
                end: range.start,
                replacement: "(".to_owned(),
                safe: true,
            },
            Edit {
                start: range.end,
                end: range.end,
                replacement: ")".to_owned(),
                safe: true,
            },
        ])
}

/// `correct_arguments`: the opening parenthesis becomes the space that separated the name from the
/// parameters, and the closing one goes away.
fn unwanted(context: &RuleContext<'_>, parameters: Node<'_>) -> Offense {
    let range = parameters.byte_range();
    let text = context.source.text();
    let open = range.start..range.start + 1;
    let close = text[..range.end]
        .char_indices()
        .next_back()
        .map_or(range.end..range.end, |(offset, character)| {
            offset..offset + character.len_utf8()
        });
    context.offense(MSG_PRESENT, range).corrected_by_all([
        Edit {
            start: open.start,
            end: open.end,
            replacement: " ".to_owned(),
            safe: true,
        },
        Edit {
            start: close.start,
            end: close.end,
            replacement: String::new(),
            safe: true,
        },
    ])
}

/// `node.endless? || anonymous_arguments?(node)`: either forces the parentheses to stay whatever
/// the style asks for.
fn forced(node: Node<'_>, parameters: Node<'_>) -> bool {
    // `endless?` is `!loc.end`: a definition written with `=` has no closing keyword.
    let mut cursor = node.walk();
    if !node.children(&mut cursor)
        .any(|child| child.kind_str() == "end")
    {
        return true;
    }
    let written = super::nodes::children(parameters);
    let anonymous = |parameter: &Node<'_>| match parameter.kind_str() {
        // `(...)`: forwarding takes the parentheses with it.
        "forward_parameter" => true,
        // `*` and `**` written without a name are anonymous.
        "splat_parameter" | "hash_splat_parameter" => {
            parameter.field("name").is_none()
        }
        _ => false,
    };
    written.iter().any(anonymous)
        // An anonymous block parameter only counts where it is the last one written.
        || written.last().is_some_and(|last| {
            last.kind_str() == "block_parameter" && last.field("name").is_none()
        })
}

fn multiline(context: &RuleContext<'_>, parameters: Node<'_>) -> bool {
    let range = parameters.byte_range();
    context.source.line_column(range.start).0 != context.source.line_column(range.end).0
}
