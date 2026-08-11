//! The alphabetical-order rule `Bundler/OrderedGems` and `Gemspec/OrderedDependencies` share.
//!
//! Upstream keeps it in the `OrderedGemNode` mixin and the `OrderedGemCorrector`; the two cops
//! differ only in which declarations they collect, what they call them in the message, and whether
//! a pair of neighbours has to name the same method to be compared.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::send_range;

/// One declaration to be ordered: the call that makes it and the gem name it names.
pub(crate) struct Declaration<'tree> {
    pub node: Node<'tree>,
    pub name: String,
}

/// Reports every neighbouring pair that is out of order, in the order the declarations were found.
///
/// `comparable` is the extra condition a department puts on a pair beyond being adjacent and out of
/// order: `Gemspec/OrderedDependencies` compares only declarations made through the same method,
/// while `Bundler/OrderedGems` compares every neighbouring pair.
pub(crate) fn check(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    declarations: &[Declaration<'_>],
    message: &dyn Fn(&str, &str) -> String,
    comparable: &dyn Fn(Node<'_>, Node<'_>) -> bool,
) {
    // `ConsiderPunctuation` is `false` and `TreatCommentsAsGroupSeparators` is `true` by default,
    // and both cops carry the same two parameters.
    let consider_punctuation: bool = context.setting("ConsiderPunctuation").unwrap_or(false);
    let comments_separate: bool = context
        .setting("TreatCommentsAsGroupSeparators")
        .unwrap_or(true);

    for pair in declarations.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        if !consecutive_lines(previous.node, current.node, comments_separate, context) {
            continue;
        }
        if !out_of_order(&current.name, &previous.name, consider_punctuation) {
            continue;
        }
        if !comparable(previous.node, current.node) {
            continue;
        }
        // Upstream names the *current* gem as the one that should come first, and the *previous*
        // one as what it should come before -- the two are swapped against the loop's own naming.
        offenses.push(
            context
                .offense(
                    message(&current.name, &previous.name),
                    send_range(current.node, context),
                )
                .corrected_by(swap(
                    previous.node,
                    current.node,
                    comments_separate,
                    context,
                )),
        );
    }
}

/// Mirrors `case_insensitive_out_of_order?`: `-` and `_` carry no weight unless the configuration
/// says they do, and case never does.
fn out_of_order(current: &str, previous: &str, consider_punctuation: bool) -> bool {
    canonical_name(current, consider_punctuation) < canonical_name(previous, consider_punctuation)
}

fn canonical_name(name: &str, consider_punctuation: bool) -> String {
    let name = match consider_punctuation {
        true => name.to_owned(),
        false => name.replace(['-', '_'], ""),
    };
    name.to_lowercase()
}

/// Whether `current` is declared on the line directly below `previous`. When comments do not
/// separate groups, the declaration starts at the comment written above it instead.
fn consecutive_lines(
    previous: Node<'_>,
    current: Node<'_>,
    comments_separate: bool,
    context: &RuleContext<'_>,
) -> bool {
    let first_line = line_of(
        declaration_start(current, comments_separate, context),
        context,
    );
    let previous_last_line = line_of(send_range(previous, context).end, context);
    previous_last_line + 1 == first_line
}

/// Where the declaration begins for the purpose of grouping: the call itself, or the first of the
/// comment lines directly above it when comments are not group separators.
fn declaration_start(node: Node<'_>, comments_separate: bool, context: &RuleContext<'_>) -> usize {
    if comments_separate {
        return node.start_byte();
    }
    let mut line = line_of(node.start_byte(), context);
    while line > 1 && is_comment_line(line - 1, context) {
        line -= 1;
    }
    context.source.line_start(line)
}

/// Whether the line holds nothing but a comment. A trailing comment belongs to the code on its
/// line, and upstream never associates one with the declaration below it.
fn is_comment_line(line: usize, context: &RuleContext<'_>) -> bool {
    let range = context.source.line_range(line);
    let trimmed = context.source.slice(range.clone()).trim_start();
    trimmed.starts_with('#')
        && context
            .comment_ranges()
            .iter()
            .any(|comment| comment.start >= range.start && comment.start < range.end)
}

/// Swaps the two declarations, each taken as the whole lines it occupies.
///
/// Upstream writes this as two edits -- insert the current declaration before the previous one and
/// delete it where it stood -- but the two ranges are always adjacent, since a pair is only
/// reported when the declarations are on neighbouring lines. One replacement over both therefore
/// produces the same bytes while staying a single edit, which is what the correction pass expects.
fn swap(
    previous: Node<'_>,
    current: Node<'_>,
    comments_separate: bool,
    context: &RuleContext<'_>,
) -> Edit {
    let previous_range = whole_lines(previous, comments_separate, context);
    let current_range = whole_lines(current, comments_separate, context);
    let previous_source = context.source.slice(previous_range.clone());
    let current_source = context.source.slice(current_range.clone());
    // A file whose last line has no newline leaves the moved declaration without one, so upstream
    // puts it back when it inserts the text above.
    let separator = match current_source.ends_with('\n') {
        true => "",
        false => "\n",
    };
    Edit {
        start: previous_range.start,
        end: current_range.end,
        replacement: format!("{current_source}{separator}{previous_source}"),
        safe: true,
    }
}

/// The whole lines a declaration occupies, from the start of its first line through the newline
/// ending its last.
fn whole_lines(node: Node<'_>, comments_separate: bool, context: &RuleContext<'_>) -> Range<usize> {
    let start = declaration_start(node, comments_separate, context);
    let first_line = line_of(start, context);
    let last_line = line_of(send_range(node, context).end, context);
    context.source.line_start(first_line)..context.source.line_range(last_line).end
}

fn line_of(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}
