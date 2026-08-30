//! `Style/SingleLineDoEndBlock`: a `do ... end` block written on one line.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::heredoc_body;
use crate::rules::send_node::named_children_of;
use crate::rules::send_node::all_children_of;

const MSG: &str = "Prefer multiline `do`...`end` block.";

/// The kinds `safe_to_split?` refuses to fold, which either need their own lines or carry one.
const UNSAFE_TO_SPLIT: &[&str] = &[
    "if",
    "unless",
    "case",
    "case_match",
    "begin",
    "method",
    "singleton_method",
    "rescue",
    "ensure",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for block in context.nodes_of("do_block") {
        let Some(node) = block.parent() else {
            continue;
        };
        // `node.multiline?`: upstream's block node starts at the receiver, which is where the
        // node the grammar wrote the block inside starts too.
        if node.start_position().row != node.end_position().row {
            continue;
        }
        if single_line_blocks_preferred(context) && suitable_as_single_line(context, node) {
            continue;
        }
        let Some(end) = end_keyword(block) else {
            continue;
        };
        let Some(opening) = do_line(context, block) else {
            continue;
        };
        let mut edits = vec![Edit {
            start: opening,
            end: opening,
            replacement: "\n".to_owned(),
            safe: true,
        }];
        match trailing_heredoc(context, block) {
            // The `end` cannot stay where it is: the heredoc's body follows the line it is on.
            Some(terminator) => {
                edits.push(Edit {
                    start: end.start_byte(),
                    end: end.end_byte(),
                    replacement: String::new(),
                    safe: true,
                });
                edits.push(Edit {
                    start: terminator,
                    end: terminator,
                    replacement: "\nend".to_owned(),
                    safe: true,
                });
            }
            None => edits.push(Edit {
                start: end.start_byte(),
                end: end.start_byte(),
                replacement: "\n".to_owned(),
                safe: true,
            }),
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `do_line`: where the line break goes, which is after the parameters when the block declares
/// any and after the `do` when it does not.
fn do_line(context: &RuleContext<'_>, block: Node<'_>) -> Option<usize> {
    if let Some(parameters) = block.field("parameters")
        && !super::nodes::children_in(parameters, context).is_empty()
    {
        return Some(parameters.end_byte());
    }
    let _ = context;
    let _cursor = block.walk();
    all_children_of(block, context)
        .into_iter()
        .find(|child| child.kind_str() == "do")
        .map(|token| token.end_byte())
}

/// `node.loc.end`.
fn end_keyword<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = block.walk();
    block.children(&mut cursor)
        .find(|child| child.kind_str() == "end")
}

/// `trailing_heredoc`: where the last heredoc opened inside the block is terminated.
fn trailing_heredoc(context: &RuleContext<'_>, block: Node<'_>) -> Option<usize> {
    let mut stack = vec![block];
    let mut last = None;
    while let Some(node) = stack.pop() {
        if node.kind_str() == "heredoc_beginning"
            && let Some(body) = heredoc_body(node, context)
        {
            let _cursor = body.walk();
            if let Some(terminator) = named_children_of(body, context)
                .into_iter()
                .find(|child| child.kind_str() == "heredoc_end")
            {
                last = Some(last.map_or(terminator.end_byte(), |end: usize| {
                    end.max(terminator.end_byte())
                }));
            }
        }
        crate::rules::push_named_children_in(node, context, &mut stack);
    }
    last
}

/// `single_line_blocks_preferred?`: `Layout/RedundantLineBreak` is switched on and asked to look
/// inside blocks, which is when a one-line block is what that cop wanted.
fn single_line_blocks_preferred(context: &RuleContext<'_>) -> bool {
    context.cop_enabled("Layout/RedundantLineBreak")
        && context
            .setting_of::<bool>("Layout/RedundantLineBreak", "InspectBlocks")
            .unwrap_or(false)
}

/// `CheckSingleLineSuitability#suitable_as_single_line?`.
fn suitable_as_single_line(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    !too_long(context, node) && !comment_within(context, node) && safe_to_split(context, node)
}

/// `too_long?`: what the block would measure once folded onto one line.
fn too_long(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(max) = context.setting_of::<usize>("Layout/LineLength", "Max") else {
        return false;
    };
    let first = context.source.line_column(node.start_byte()).0;
    let last = context.source.line_column(node.end_byte()).0;
    let lines: Vec<&str> = (first..=last)
        .map(|line| context.source.line(line).trim_end_matches(['\r', '\n']))
        .collect();
    to_single_line(&lines.join("\n")).chars().count() > max
}

/// `to_single_line`.
fn to_single_line(source: &str) -> String {
    let mut folded = source.to_owned();
    for (pattern, replacement) in [
        // Double quote, backslash, and then single quote.
        (r#"" *\\\n\s*'"#, r#"" + '"#),
        // Single quote, backslash, and then double quote.
        (r#"' *\\\n\s*""#, r#"' + ""#),
    ] {
        if let Some(regex) = crate::rules::regex_cache::compiled(pattern) {
            folded = regex.replace_all(&folded, replacement).into_owned();
        }
    }
    // Double or single quote, backslash, then the same quote again.
    if let Some(regex) = crate::rules::regex_cache::compiled(r#"(["']) *\\\n\s*(["'])"#) {
        folded = regex
            .replace_all(&folded, |captures: &regex::Captures<'_>| {
                match captures[1] == captures[2] {
                    true => String::new(),
                    false => captures[0].to_owned(),
                }
            })
            .into_owned();
    }
    // Extra space within method chaining, which includes `&.`.
    if let Some(regex) = crate::rules::regex_cache::compiled(r"\n\s*(&?\.\w)") {
        folded = regex.replace_all(&folded, "$1").into_owned();
    }
    // Any other line break, with or without backslash.
    match crate::rules::regex_cache::compiled(r"\s*\\?\n\s*") {
        Some(regex) => regex.replace_all(&folded, " ").into_owned(),
        None => folded,
    }
}

/// `comment_within?`.
fn comment_within(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let first = context.source.line_column(node.start_byte()).0;
    let last = context.source.line_column(node.end_byte()).0;
    context.comment_ranges().iter().any(|comment| {
        let line = context.source.line_column(comment.start).0;
        (first..=last).contains(&line)
    })
}

/// `safe_to_split?`.
fn safe_to_split(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let mut stack: Vec<Node<'_>> = Vec::new();
    crate::rules::push_named_children_in(node, context, &mut stack);
    while let Some(child) = stack.pop() {
        if UNSAFE_TO_SPLIT.contains(&child.kind_str()) {
            return false;
        }
        if child.kind_str() == "heredoc_beginning" {
            return false;
        }
        // A literal that carries a line break of its own cannot be folded away.
        if matches!(
            child.kind_str(),
            "string"
                | "chained_string"
                | "parenthesized_statements"
                | "simple_symbol"
                | "delimited_symbol"
        ) && context.source.node_text(child).contains('\n')
        {
            return false;
        }
        crate::rules::push_named_children_in(child, context, &mut stack);
    }
    true
}
