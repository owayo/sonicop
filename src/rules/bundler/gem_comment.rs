use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::{push_named_children_in};
use crate::rules::send_node::{arguments, is_string, pair_key_symbol, send_range, string_text};

use super::support::gem_declarations;
use crate::rules::send_node::named_children_iter;

const MSG: &str = "Missing gem description comment.";

/// `RESTRICTIVE_VERSION_PATTERN`.
static RESTRICTIVE_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?-u:\s)*(?:<|~>|(?-u:\d)|=)").expect("the restriction pattern compiles"));

/// `MAGIC_COMMENT_RE`, which `Comment::Associator` steps over before it starts associating.
static MAGIC_COMMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^#(?-u:\s)*(-\*-|)(?-u:\s)*(frozen_string_literal|warn_indent|warn_past_scope):.*$")
        .expect("the magic comment pattern compiles")
});

/// `Buffer::ENCODING_RE`.
static ENCODING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[[:space:]#](en)?coding(?-u:\s)*[:=](?-u:\s)*((utf8-mac)|([A-Za-z0-9_-]+?)(-unix|-dos|-mac)|([A-Za-z0-9_-]+))",
    )
    .expect("the encoding comment pattern compiles")
});

/// One comment `Comment::Associator` will hand out, and whether it stands on a line of its own.
///
/// A comment sharing a line with code is associated with the statement that line holds, so a gem
/// declared on the following line is never described by it.
struct Comment {
    line: usize,
    start: usize,
    standalone: bool,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedGems").unwrap_or_default();
    let only_for: Vec<String> = context.setting("OnlyFor").unwrap_or_default();
    let comments = associable_comments(context);
    for (node, name) in gem_declarations(context) {
        if allowed.iter().any(|gem| gem == string_text(name, context)) {
            continue;
        }
        if is_commented(node, &comments, context) {
            continue;
        }
        if !only_for.is_empty() && !checked_options_present(node, &only_for, context) {
            continue;
        }
        offenses.push(context.offense(MSG, send_range(node, context)));
    }
}

/// Every comment but the shebang and the two magic comments `advance_through_directives` steps over
/// before the association starts.
fn associable_comments(context: &RuleContext<'_>) -> Vec<Comment> {
    let comments: Vec<Comment> = context
        .comment_ranges()
        .iter()
        .map(|range| {
            let line = context.source.line_column(range.start).0;
            let before = context
                .source
                .slice(context.source.line_start(line)..range.start);
            Comment {
                line,
                start: range.start,
                standalone: before.trim().is_empty(),
            }
        })
        .collect();
    let text = |index: usize| {
        context
            .comment_ranges()
            .get(index)
            .map_or("", |range| context.source.slice(range.clone()))
    };
    // A shebang, then at most one magic comment, then at most one encoding comment, each only while
    // it is the next comment in the file.
    let mut skipped = 0;
    if text(skipped).starts_with("#!") {
        skipped += 1;
    }
    if MAGIC_COMMENT.is_match(text(skipped)) {
        skipped += 1;
    }
    if ENCODING.is_match(text(skipped)) {
        skipped += 1;
    }
    comments.into_iter().skip(skipped).collect()
}

/// `commented_any_descendant?`: whether a comment the declaration or one of its descendants was
/// associated with sits on that node's own line or the line above it.
///
/// Upstream reaches this through `ProcessedSource#ast_with_comments`, which hands every comment to
/// exactly one node: the first one visited that begins after it, or the innermost one whose last
/// line the comment shares. For the shapes a Gemfile is written in, what that decides is which of
/// two neighbouring declarations the comment between them belongs to -- and that is settled by
/// whether the comment stands on a line of its own or trails the earlier one.
fn is_commented(node: Node<'_>, comments: &[Comment], context: &RuleContext<'_>) -> bool {
    let range = send_range(node, context);
    let lines = subtree_lines(node, context);
    let last_line = context.source.line_column(range.end).0;
    comments.iter().any(|comment| {
        let leading = comment.start < range.start;
        if leading && (!comment.standalone || claimed_by_an_enclosing_node(node, comment, context)) {
            return false;
        }
        comment.line <= last_line
            && (lines.contains(&comment.line) || lines.contains(&(comment.line + 1)))
    })
}

/// Whether a comment written before the declaration belongs to something the declaration sits inside
/// rather than to the declaration itself.
///
/// `process_leading_comments` hands the comment to the *first* node the walk reaches that begins
/// after it, and the walk reaches an enclosing node before the one it encloses. A `gem` declaration
/// written as `gem 'x' if cond` is therefore described by nothing: the comment above it belongs to
/// the `if`, which begins at the same place. Only upstream's `begin` is passed over, so the wrappers
/// the grammar adds for statement and argument lists -- which no node upstream corresponds to -- are
/// skipped here as well.
fn claimed_by_an_enclosing_node(
    node: Node<'_>,
    comment: &Comment,
    context: &RuleContext<'_>,
) -> bool {
    let end = comment_end(comment, context);
    let mut current = node;
    while let Some(parent) = current.parent_of(context) {
        if !SYNTHETIC_KINDS.contains(&parent.kind_str()) && parent.start_byte() >= end {
            return true;
        }
        current = parent;
    }
    false
}

/// Where the comment ends, which is where the line it opens does.
fn comment_end(comment: &Comment, context: &RuleContext<'_>) -> usize {
    context
        .comment_ranges()
        .iter()
        .find(|range| range.start == comment.start)
        .map_or(comment.start, |range| range.end)
}

/// The node kinds the grammar adds that upstream's parser has no node for, plus its `begin`, which
/// `process_leading_comments` passes over.
const SYNTHETIC_KINDS: &[&str] = &[
    "program",
    "body_statement",
    "then",
    "else",
    "do",
    "block_body",
    "parenthesized_statements",
    "argument_list",
    "block",
    "do_block",
];

/// The lines the declaration's own nodes begin on, which are the lines a comment can describe.
fn subtree_lines(node: Node<'_>, context: &RuleContext<'_>) -> HashSet<usize> {
    let mut lines = HashSet::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "comment" {
            continue;
        }
        lines.insert(context.source.line_column(current.start_byte()).0);
        push_named_children_in(current, context, &mut stack);
    }
    lines
}

/// `checked_options_present?`: whether the declaration carries one of the things `OnlyFor` asks a
/// comment for.
fn checked_options_present(node: Node<'_>, only_for: &[String], context: &RuleContext<'_>) -> bool {
    let arguments = arguments(node);
    let rest = arguments.get(1..).unwrap_or_default();
    // `version_specified_gem?`: the second argument is a plain string.
    let versioned = rest
        .first()
        .is_some_and(|argument| is_string(argument.first(), context));
    if versioned && only_for.iter().any(|option| option == "version_specifiers") {
        return true;
    }
    // `restrictive_version_specified_gem?`: one of the arguments after the name pins the version
    // rather than merely allowing it.
    if versioned
        && only_for
            .iter()
            .any(|option| option == "restrictive_version_specifiers")
        && rest.iter().any(|argument| {
            is_string(argument.first(), context)
                && RESTRICTIVE_VERSION.is_match(string_text(argument.first(), context))
        })
    {
        return true;
    }
    // `contains_checked_options?`: one of the keys of the trailing options hash is named.
    gem_options(node, context)
        .iter()
        .any(|key| only_for.iter().any(|option| option == key))
}

/// `gem_options`: the string and symbol keys of the declaration's last argument, when that argument
/// is a hash.
fn gem_options<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Vec<&'a str> {
    let arguments = arguments(node);
    let Some(last) = arguments.last() else {
        return Vec::new();
    };
    let pairs: Vec<Node<'_>> = match last.first().kind_str() {
        "hash" if last.parts().len() == 1 => {
            let _cursor = last.first().walk();
            named_children_iter(last.first(), context).collect()
        }
        _ => last.parts().to_vec(),
    };
    pairs
        .iter()
        .filter(|pair| pair.kind_str() == "pair")
        .filter_map(|pair| {
            pair_key_symbol(*pair, context).or_else(|| {
                let key = pair.field("key")?;
                is_string(key, context).then(|| string_text(key, context))
            })
        })
        .collect()
}
