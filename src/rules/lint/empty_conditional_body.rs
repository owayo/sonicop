use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::statements::statements;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::all_children_iter;

/// The three conditionals whose branch can be written empty. A modifier form always has a body.
const CONDITIONALS: &[&str] = &["if", "unless", "elsif"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(true);
    for node in context.nodes_of_any(CONDITIONALS) {
        let consequence = node.field("consequence");
        if consequence.is_some_and(|branch| !statements(branch).is_empty()) {
            continue;
        }
        // `same_line?(node.loc.begin, node.loc.end)`: `if foo then end` is written that way on
        // purpose and reported by nothing.
        if let (Some(begin), Some(end)) = (then_keyword(node, context), end_keyword(node, context))
            && context.source.line_column(begin.start_byte()).0
                == context.source.line_column(end.start_byte()).0
        {
            continue;
        }
        if allow_comments && contains_comments(node, context) {
            continue;
        }
        let keyword = context.source.node_text(
            node.child(0)
                .filter(|child| !child.is_named())
                .unwrap_or(node),
        );
        let alternative = node.field("alternative");
        let range = match alternative {
            Some(clause) => node.start_byte()..clause.start_byte(),
            // `node.source_range` ends at the last node the **parser** built, and a comment is not
            // one. An `elsif` holding nothing but a comment therefore ends at its condition; an
            // `if` still reaches its own `end`, which the grammar keeps inside the node.
            None if node.kind_str() == "elsif" => {
                let end = node
                    .field("condition")
                    .map_or_else(|| node.end_byte(), |condition| condition.end_byte());
                // The parser's range keeps the separator that closes the clause -- `elsif cond;`
                // ends after the semicolon -- and stops before a comment or a line break.
                let end = match context.source.text()[end..node.end_byte()].chars().next() {
                    Some(';') => end + 1,
                    _ => end,
                };
                node.start_byte()..end
            }
            None => node.byte_range(),
        };
        let mut offense =
            context.offense(format!("Avoid `{keyword}` branches without a body."), range);
        // `can_simplify_conditional?`: only an `else` can be flipped into the condition, since an
        // `elsif` would still need a branch of its own.
        if let Some(clause) =
            alternative.filter(|clause| clause.kind_str() == "else" && !statements(*clause).is_empty())
        {
            offense = offense.corrected_by_all(flip(node, clause, keyword, context));
        }
        offenses.push(offense);
    }
}

/// `flip_orphaned_else`: the `else` becomes the conditional itself, and the empty branch goes.
fn flip(node: Node<'_>, clause: Node<'_>, keyword: &str, context: &RuleContext<'_>) -> Vec<Edit> {
    let Some(else_keyword) = clause.child(0) else {
        return Vec::new();
    };
    let Some(condition) = node.field("condition") else {
        return Vec::new();
    };
    let inverse = match keyword {
        "if" => "unless",
        "unless" => "if",
        _ => "",
    };
    let mut edits = vec![Edit {
        start: else_keyword.start_byte(),
        end: else_keyword.end_byte(),
        replacement: format!("{inverse} {}", context.source.node_text(condition)),
        safe: true,
    }];
    // `remove_empty_branch`: an `if` whose own branch is the empty one loses everything up to the
    // `else`, while a branch reached through an `elsif` loses the rest of its line as well.
    let empty_if_branch = is_empty_if_branch(node);
    let else_branch = !statements(clause).is_empty()
        && !statements(clause)
            .first()
            .is_some_and(|first| matches!(first.kind_str(), "if" | "unless" | "elsif"));
    let range = if empty_if_branch && else_branch {
        node.start_byte()..else_keyword.start_byte()
    } else {
        deletion_range(node.start_byte()..condition.end_byte(), context)
    };
    edits.push(Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    });
    edits
}

/// `empty_if_branch?`: whether this is the branch the enclosing conditional would be left without.
fn is_empty_if_branch(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if !CONDITIONALS.contains(&parent.kind_str()) {
        return true;
    }
    let Some(branch) = parent.field("consequence") else {
        return true;
    };
    let branch = statements(branch);
    match branch.first() {
        Some(first) => {
            CONDITIONALS.contains(&first.kind_str())
                && first
                    .field("consequence")
                    .is_none_or(|body| statements(body).is_empty())
        }
        None => true,
    }
}

/// `deletion_range`: the span plus the rest of the line it ends on, including the line break.
fn deletion_range(
    range: std::ops::Range<usize>,
    context: &RuleContext<'_>,
) -> std::ops::Range<usize> {
    let text = context.source.text();
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |offset| range.end + offset + 1);
    range.start..end
}

/// `contains_comments?`: a comment written between the keyword and the branch that follows the
/// empty one, which is where a reader would explain why the branch does nothing.
fn contains_comments(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let start = context.source.line_column(node.start_byte()).0;
    let end = match node.field("alternative") {
        // `find_end_line`: the `else` or `elsif` that follows ends the span.
        Some(clause) => context.source.line_column(clause.start_byte()).0,
        // `find_end_line`'s `elsif?` branch: `node.each_ancestor(:if).find(&:if?).loc.end.line`.
        // An `elsif` with nothing after it owns the lines up to the `end` that closes the whole
        // conditional, and that `end` belongs to the `if` above it. The grammar stops the `elsif`
        // node at its own last statement, so reading its end instead leaves an empty span and the
        // comment that excuses the branch is never seen.
        None if node.kind_str() == "elsif" => enclosing_end(node, context)
            .map_or_else(
                || context.source.line_column(node.end_byte()).0,
                |keyword| context.source.line_column(keyword.start_byte()).0,
            ),
        None => context.source.line_column(node.end_byte()).0,
    };
    context.comment_ranges().iter().any(|comment| {
        let line = context.source.line_column(comment.start).0;
        line >= start && line < end
    })
}

/// The `end` closing the conditional an `elsif` sits in, which is the `if` above it. A chain of
/// `elsif` nests, so the walk passes any number of them before it reaches the `if`.
fn enclosing_end<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if ancestor.kind_str() == "if" {
            return end_keyword(ancestor, context);
        }
        current = ancestor.parent();
    }
    None
}

/// `node.loc.begin`: the `then` or `;` the branch was introduced with. The grammar puts a `then`
/// inside the branch it opens, but a `;` stays beside the condition -- and a branch holding nothing
/// is no node at all, which leaves the `;` as the only trace of it.
fn then_keyword<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    if let Some(first) = node
        .field("consequence")
        .and_then(|branch| branch.child(0))
        && matches!(context.source.node_text(first), "then" | ";")
    {
        return Some(first);
    }
    let condition = node.field("condition")?;
    let _cursor = node.walk();
    all_children_iter(node, context).find(|child| {
        child.start_byte() >= condition.end_byte()
            && matches!(context.source.node_text(*child), "then" | ";")
    })
}

/// `node.loc.end`: the `end` keyword, which an `elsif` borrows from the `if` it belongs to.
fn end_keyword<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let last = node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)?;
    (context.source.node_text(last) == "end").then_some(last)
}
