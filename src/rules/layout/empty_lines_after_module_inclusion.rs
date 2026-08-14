use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::directives::directive_header;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, named_children, send_range};

const MSG: &str = "Add an empty line after module inclusion.";

/// `MODULE_INCLUSION_METHODS`, which is also `RESTRICT_ON_SEND`.
const METHODS: &[&str] = &["include", "extend", "prepend"];

/// The node kinds the grammar adds for statement and argument lists, which upstream's parser has no
/// node for other than its `begin`.
const CONTAINERS: &[&str] = &[
    "program",
    "body_statement",
    "then",
    "else",
    "do",
    "block_body",
    "parenthesized_statements",
];

/// A conditional written after its body, whose `modifier_form?` upstream reads through.
const MODIFIERS: &[&str] = &[
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if node.field("receiver").is_some() || !is_plain_send(node, context) {
            continue;
        }
        if node
            .field("method")
            .is_none_or(|method| !METHODS.contains(&context.source.node_text(method)))
            || arguments(node).is_empty()
        {
            continue;
        }
        let Some(parent) = upstream_parent(node, context) else {
            continue;
        };
        // `node.parent&.type?(:send, :any_block, :array)`: an inclusion handed to another call or
        // written as an element of an array is not the statement this cop is about.
        if matches!(parent.kind_str(), "call" | "array") {
            continue;
        }
        let last_line = context.source.line_column(send_range(node, context).end).0;
        if next_line_empty_or_enable_directive(last_line, context) {
            continue;
        }
        // `next_line_node`: a conditional the inclusion was written under has no next statement to
        // look at. An `elsif` is one more `if` upstream, which is what makes the branch of an
        // `elsif` chain exempt as much as the branch of the `if` that opened it.
        if matches!(parent.kind_str(), "if" | "unless" | "elsif" | "conditional")
            || is_modifier(parent)
        {
            continue;
        }
        let Some(next) = next_statement(node, parent) else {
            continue;
        };
        if is_module_inclusion(next, context) {
            continue;
        }
        // `autocorrect`: the blank line goes after the inclusion, or after the `rubocop:enable`
        // comment written directly below it.
        let mut anchor = whole_lines(send_range(node, context), context);
        let below = context.source.line_column(anchor.end).0 + 1;
        if let Some(comment) = enable_directive_at(below, context) {
            anchor = comment;
        }
        offenses.push(
            context
                .offense(MSG, send_range(node, context))
                .corrections_anchored_at(anchor.clone())
                .corrected_by(Edit {
                    start: anchor.end,
                    end: anchor.end,
                    replacement: "\n".to_owned(),
                    safe: true,
                }),
        );
    }
}

/// `node.parent` as upstream's parser builds it: the wrappers the grammar adds for statement and
/// argument lists have no counterpart there, and a list of more than one statement is a `begin`.
fn upstream_parent<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut current = node;
    loop {
        let parent = current.parent_of(context)?;
        if parent.kind_str() == "argument_list" {
            current = parent;
            continue;
        }
        // A container holding one statement is that statement upstream; holding more, it is a
        // `begin`, which this cop only ever asks the siblings of.
        if CONTAINERS.contains(&parent.kind_str()) {
            if statements(parent).len() > 1 {
                return Some(parent);
            }
            current = parent;
            continue;
        }
        return Some(parent);
    }
}

/// The statements a container holds, which are its children less the comments.
fn statements<'tree>(container: Node<'tree>) -> Vec<Node<'tree>> {
    named_children(container)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect()
}

/// `node.right_sibling`: the next child of the parent, which for a statement list is the statement
/// written after this one.
fn next_statement<'tree>(node: Node<'tree>, parent: Node<'tree>) -> Option<Node<'tree>> {
    let siblings = statements(parent);
    let position = siblings
        .iter()
        .position(|sibling| sibling.byte_range().contains(&node.start_byte()))?;
    siblings.get(position + 1).copied()
}

/// `allowed_method?`: whether the next statement is another module inclusion, which needs no blank
/// line between the two.
fn is_module_inclusion(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let node = match is_modifier(node) {
        true => match node.field("body") {
            Some(body) => body,
            None => return false,
        },
        false => node,
    };
    node.kind_str() == "call"
        && is_plain_send(node, context)
        && node
            .field("method")
            .is_some_and(|method| METHODS.contains(&context.source.node_text(method)))
}

fn is_modifier(node: Node<'_>) -> bool {
    MODIFIERS.contains(&node.kind_str())
}

/// `next_line_empty_or_enable_directive_comment?`, where `line` is the inclusion's last line: the
/// line below it is blank, or holds a `rubocop:enable` comment with a blank line under that.
fn next_line_empty_or_enable_directive(line: usize, context: &RuleContext<'_>) -> bool {
    if is_blank(line + 1, context) {
        return true;
    }
    enable_directive_at(line + 1, context).is_some() && is_blank(line + 2, context)
}

/// `line_empty?`, which counts a line past the end of the file as empty.
fn is_blank(line: usize, context: &RuleContext<'_>) -> bool {
    line > context.source.line_count() || context.source.line(line).trim().is_empty()
}

/// The span of the `rubocop:enable` comment written on `line`, when there is one.
fn enable_directive_at(line: usize, context: &RuleContext<'_>) -> Option<Range<usize>> {
    let comment = context
        .comment_ranges()
        .iter()
        .find(|range| context.source.line_column(range.start).0 == line)?;
    let text = context.source.slice(comment.clone());
    let header = directive_header(text)?;
    // `DirectiveComment#enabled?` is `mode == 'enable'`, so a `todo` or a comment carrying no
    // directive at all is not one.
    (header.mode == "enable").then(|| comment.clone())
}

/// `range_by_whole_lines(range)`: the lines the node sits on, without the break that closes them.
fn whole_lines(range: Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    let text = context.source.text();
    let start = text[..range.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |offset| range.end + offset);
    start..end
}
