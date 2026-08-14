//! `Layout/EmptyLinesAfterModuleInclusion`: a blank line after the `include`s a class opens with.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

const MSG: &str = "Add an empty line after module inclusion.";

/// `MODULE_INCLUSION_METHODS`.
const INCLUSION_METHODS: &[&str] = &["include", "extend", "prepend"];

/// What the grammar parks in a statement list that upstream's `begin` has no child for.
const NOT_A_STATEMENT: &[&str] = &["comment", "heredoc_body"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if !is_inclusion(node, context) {
            continue;
        }
        // `node.parent&.type?(:send, :any_block, :array)`: an inclusion handed to something else
        // is not the statement this cop is about.
        if node.field("block").is_some() || parent_takes_it_as_a_value(node, context) {
            continue;
        }
        let last_line = node.end_position().row + 1;
        if next_line_empty_or_enable_directive(context, last_line) {
            continue;
        }
        // `require_empty_line?`: nothing to separate from, or the next statement is another
        // inclusion, and the blank is not wanted.
        let Some(next) = next_statement(node) else {
            continue;
        };
        if is_allowed_next(next, context) {
            continue;
        }
        // `range_by_whole_lines`: the insertion goes after the line the call ends on, or after the
        // `rubocop:enable` comment written under it.
        let mut insert_at = context.source.line_range(last_line).end.saturating_sub(1);
        if let Some(comment) = enable_directive_at(context, last_line + 1) {
            insert_at = comment.end;
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrections_anchored_at(insert_at..insert_at)
                .corrected_by(Edit {
                    start: insert_at,
                    end: insert_at,
                    replacement: "\n".to_owned(),
                    safe: true,
                }),
        );
    }
}

/// `(send nil? {:include :extend :prepend} ...)` with at least one argument.
fn is_inclusion(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.field("receiver").is_some() {
        return false;
    }
    node.field("method")
        .is_some_and(|name| INCLUSION_METHODS.contains(&context.source.node_text(name)))
        && !arguments(node).is_empty()
}

/// Whether what encloses the call reads it as a value rather than as a statement.
fn parent_takes_it_as_a_value(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = context.parent(node) else {
        return false;
    };
    let parent = match parent.kind_str() {
        "argument_list" => match context.parent(parent) {
            Some(outer) => outer,
            None => return false,
        },
        _ => parent,
    };
    matches!(
        parent.kind_str(),
        "call" | "element_reference" | "array" | "block" | "do_block"
    )
}

/// `next_line_empty_or_enable_directive_comment?`.
fn next_line_empty_or_enable_directive(context: &RuleContext<'_>, last_line: usize) -> bool {
    if line_empty(context, last_line + 1) {
        return true;
    }
    enable_directive_at(context, last_line + 1).is_some() && line_empty(context, last_line + 2)
}

/// `line_empty?`, which reads a line past the end of the file as empty.
fn line_empty(context: &RuleContext<'_>, line: usize) -> bool {
    context.source.line(line).trim().is_empty()
}

/// `enable_directive_comment?`: a `# rubocop:enable ...` written on that line.
fn enable_directive_at(context: &RuleContext<'_>, line: usize) -> Option<std::ops::Range<usize>> {
    context.comment_ranges().iter().find_map(|comment| {
        if context.source.line_column(comment.start).0 != line {
            return None;
        }
        let text = context.source.slice(comment.clone());
        let header = crate::directives::directive_header(text)?;
        (header.mode == "enable").then(|| comment.clone())
    })
}

/// `next_line_node`: the statement written after this one, when there is one.
///
/// `return if node.parent.if_type?` guards the modifier forms: there the sibling upstream hands
/// out is the *condition*, not a statement, so a conditional inclusion has nothing after it.
fn next_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let container = node.parent()?;
    if matches!(
        container.kind_str(),
        "if" | "unless" | "elsif" | "conditional" | "if_modifier" | "unless_modifier"
    ) {
        return None;
    }
    let mut cursor = container.walk();
    let statements: Vec<Node<'tree>> = container
        .named_children(&mut cursor)
        .filter(|child| !NOT_A_STATEMENT.contains(&child.kind_str()))
        .collect();
    let position = statements
        .iter()
        .position(|statement| statement.id() == node.id())?;
    statements.get(position + 1).copied()
}

/// `allowed_method?`: another module inclusion, which the blank is not wanted between.
fn is_allowed_next(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    // `node.body if node.modifier_form?`.
    let node = match node.kind_str() {
        "if_modifier" | "unless_modifier" | "while_modifier" | "until_modifier" => {
            match node.field("body") {
                Some(body) => body,
                None => return false,
            }
        }
        _ => node,
    };
    // Upstream asks only for the name here: the receiver and the arguments are not looked at.
    node.kind_str() == "call"
        && node.field("method").is_some_and(|name| {
            INCLUSION_METHODS.contains(&context.source.node_text(name))
        })
}
