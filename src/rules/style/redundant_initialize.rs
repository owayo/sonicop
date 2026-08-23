use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Remove unnecessary `initialize` method.";
const MSG_EMPTY: &str = "Remove unnecessary empty `initialize` method.";

/// `forwards?`: the parameters that pass everything on, which makes the definition worth keeping.
/// A block parameter is not one of them.
const FORWARDING: [&str; 3] = [
    "splat_parameter",
    "hash_splat_parameter",
    "forward_parameter",
];

/// An `initialize` that does nothing its parent's does not.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_comments = context.setting::<bool>("AllowComments").unwrap_or(false);
    for node in context.nodes_of("method") {
        if node
            .field("name")
            .is_none_or(|name| context.source.node_text(name) != "initialize")
        {
            continue;
        }
        let parameters = node
            .field("parameters")
            .map(|list| super::parameters::parameters(list))
            .unwrap_or_default();
        // `forwards?`.
        if parameters
            .iter()
            .any(|parameter| FORWARDING.contains(&parameter.kind))
        {
            continue;
        }
        if allow_comments && holds_comments(node, context) {
            continue;
        }
        let statements = node
            .field("body")
            .map(|body| {
                super::nodes::children(body)
                    .into_iter()
                    .filter(|child| child.kind_str() != "comment")
                    .collect::<Vec<Node<'_>>>()
            })
            .unwrap_or_default();
        let message = match statements.as_slice() {
            // An empty body is only redundant when nothing was declared either.
            [] if parameters.is_empty() => MSG_EMPTY,
            [] => continue,
            // `node.body.begin_type?`: two statements are not just a forward.
            [only] if forwards_to_super(*only, &parameters, context) => MSG,
            _ => continue,
        };
        let range = whole_lines(node.byte_range(), context);
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `initialize_forwards?` together with `same_args?`: the body is a `super` that hands on exactly
/// the parameters, in order. A bare `super` always does.
fn forwards_to_super(
    body: Node<'_>,
    parameters: &[super::parameters::Parameter<'_>],
    context: &RuleContext<'_>,
) -> bool {
    // `$arg*`: every parameter has to be a plain one.
    if parameters
        .iter()
        .any(|parameter| parameter.kind != "identifier")
    {
        return false;
    }
    // `zsuper`: the bare keyword, which forwards whatever was declared.
    if body.kind_str() == "super" {
        return true;
    }
    if body.kind_str() != "call"
        || body
            .field("method")
            .is_none_or(|method| method.kind_str() != "super")
    {
        return false;
    }
    let arguments = body
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    // A plain parameter has no `name` field of its own -- the node *is* the name -- so the range
    // the helper reports is what gets compared.
    arguments.len() == parameters.len()
        && arguments.iter().zip(parameters).all(|(argument, parameter)| {
            context.source.node_text(*argument) == &context.source.text()[parameter.range.clone()]
        })
}

/// `contains_comments?`: the comments between the definition's first line and the line
/// `find_end_line` picks, which is where the *next* statement starts rather than where the
/// definition ends.
fn holds_comments(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let start = context.source.line_column(node.start_byte()).0;
    let end = find_end_line(node, context);
    context.comment_ranges().iter().any(|range| {
        let line = context.source.line_column(range.start).0;
        line >= start && line < end
    })
}

/// `find_end_line` for a definition.
fn find_end_line(node: Node<'_>, context: &RuleContext<'_>) -> usize {
    let fallback = context.source.line_column(node.end_byte()).0;
    // `node.parent` is nil at the top level upstream, and the `|| node.loc.end.line` fallback is
    // what answers there. The grammar hands back `program` instead, which would otherwise put the
    // end line at 1 and hide every comment inside the definition.
    let Some(parent) = node.parent().filter(|parent| parent.kind_str() != "program") else {
        return fallback;
    };
    let siblings: Vec<Node<'_>> = super::nodes::children(parent)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    let position = siblings.iter().position(|child| child.id() == node.id());
    if let Some(next) = position.and_then(|index| siblings.get(index + 1)) {
        return context.source.line_column(next.start_byte()).0;
    }
    // With no sibling after it, upstream looks at the parent: a `begin` has no `end` and answers
    // with its own first line, while a class or module answers with its `end`.
    if siblings.len() > 1 || parent.kind_str() == "program" {
        return siblings
            .first()
            .map_or(fallback, |first| context.source.line_column(first.start_byte()).0);
    }
    parent
        .parent()
        .map_or(fallback, |owner| context.source.line_column(owner.end_byte()).0)
}

/// `range_by_whole_lines(range, include_final_newline: true)`.
fn whole_lines(
    range: std::ops::Range<usize>,
    context: &RuleContext<'_>,
) -> std::ops::Range<usize> {
    let text = context.source.text();
    let start = text[..range.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |offset| range.end + offset + 1);
    start..end
}
