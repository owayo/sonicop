use std::collections::HashSet;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::Interpolations;
use crate::source::is_protected;
use crate::rules::send_node::named_children_iter;

/// Node kinds whose named children are a sequence of statements.
///
/// RuboCop reaches the same set through the `begin` node its parser builds for any body holding
/// more than one expression. Restricting the scan to these kinds is what keeps `def foo; bar(1, 2);
/// end` quiet: an argument list also has two children ending on the line, but it is not a place
/// where a semicolon could be separating statements.
const STATEMENT_SEQUENCE_KINDS: &[&str] = &[
    "program",
    "body_statement",
    "block_body",
    "then",
    "else",
    "ensure",
    "begin",
    "do",
    "parenthesized_statements",
    "begin_block",
    "end_block",
    // `"#{a; b}"` puts the interpolated statements in a `begin` node of their own upstream.
    "interpolation",
];

/// Children of a statement sequence that are not statements of it.
///
/// The exception-handling clauses are separate nodes upstream -- the sequence RuboCop sees ends
/// where the `rescue` begins -- and an empty statement is nothing at all, so `if x then ; 1; end`
/// has a one-expression body rather than a two-expression one.
const NON_STATEMENT_KINDS: &[&str] = &["comment", "empty_statement", "rescue", "else", "ensure"];

/// Node kinds that spell a semicolon as part of a single token: `$;`, `?;` and `:";"`. RuboCop
/// walks a token stream rather than the text, so none of these is a semicolon to it.
const SEMICOLON_BEARING_TOKENS: &[&str] = &["global_variable", "character", "delimited_symbol"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    if !text.contains(';') {
        return;
    }
    let allow_as_expression_separator: bool = context
        .setting("AllowAsExpressionSeparator")
        .unwrap_or(false);

    let semicolons = semicolon_offsets(context);
    let mut terminators: Vec<usize> = Vec::new();

    // RuboCop reports one semicolon per line here: the one that terminates the line or opens it.
    // A semicolon in the middle of a line is not an offense on its own -- `def foo; bar; end` is
    // left alone -- because that shape is a single expression, not two.
    for line_number in 1..=context.source.line_count() {
        if let Some(offset) = line_terminator_or_opener(context, line_number, &semicolons) {
            terminators.push(offset);
        }
    }

    // The second pass is the one that makes `foo; bar` an offense: a line holding the end of more
    // than one statement really is separating expressions, and then *every* semicolon on it counts.
    // Upstream runs this pass second and drops a range it has already reported, so a semicolon that
    // both ends its line and separates expressions keeps the first pass's correction.
    let mut separators: Vec<usize> = Vec::new();
    if !allow_as_expression_separator {
        let separator_lines = expression_separator_lines(context);
        let already: HashSet<usize> = terminators.iter().copied().collect();
        separators.extend(
            semicolons
                .iter()
                .filter(|(line, offset)| {
                    separator_lines.contains(line) && !already.contains(offset)
                })
                .map(|(_, offset)| *offset),
        );
    }

    let mut reported: Vec<(usize, bool)> = terminators
        .into_iter()
        .map(|offset| (offset, false))
        .chain(separators.into_iter().map(|offset| (offset, true)))
        .collect();
    reported.sort_unstable();

    let heredoc_openers = heredoc_openers(context);
    for (offset, after_expression) in reported {
        let offense = context.offense(
            "Do not use semicolons to terminate expressions.",
            offset..offset + 1,
        );
        // A semicolon that terminates or opens a line can simply be dropped. One that separates
        // two expressions becomes the line break that should have been there -- unless a heredoc
        // opened earlier on the line, because then the rest of the line would fall into its body.
        let replacement = if after_expression {
            if heredoc_opened_before(context, &heredoc_openers, offset) {
                offenses.push(offense);
                continue;
            }
            "\n"
        } else {
            ""
        };
        let removal = Edit {
            start: offset,
            end: offset + 1,
            replacement: replacement.to_owned(),
            safe: true,
        };
        // `corrector.wrap(node, '(', ')') if node`: an endless range whose `;` goes away would
        // reach on to the next line and swallow it (`42..;\n42...;` becomes one range). Upstream
        // puts parentheses around the range first.
        // `token_before_semicolon&.type == :tLABEL`: `m key:;` has to keep reading as an argument
        // list once the semicolon goes, so the hash gains parentheses and the space in front of it
        // is removed. Without it `m key:` is a label standing on its own.
        if let Some((space, arguments)) = value_omission_before(context, offset) {
            offenses.push(offense.corrected_by_all([
                Edit {
                    start: space.start,
                    end: space.end,
                    replacement: String::new(),
                    safe: true,
                },
                insert(arguments.start, "("),
                insert(arguments.end, ")"),
                removal,
            ]));
            continue;
        }
        if let Some(range) = endless_range_before(context, offset) {
            offenses.push(offense.corrected_by_all([
                insert(range.start, "("),
                insert(range.end, ")"),
                removal,
            ]));
            continue;
        }
        offenses.push(offense.corrected_by(removal));
    }
}

fn insert(at: usize, text: &str) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text.to_owned(),
        safe: true,
    }
}

/// `token_before_semicolon&.regexp_dots?`: the range that ends right where the semicolon stands,
/// written without an end.
///
/// Only an endless one matters. `1..2;` keeps its meaning without the semicolon, but `1..;` reads
/// on into whatever follows.
fn endless_range_before(
    context: &RuleContext<'_>,
    offset: usize,
) -> Option<std::ops::Range<usize>> {
    context
        .nodes_of("range")
        .find(|node| node.end_byte() == offset && node.field("end").is_none())
        .map(|node| node.byte_range())
}

/// Every semicolon that is code rather than text, as `(line, byte offset)`.
fn semicolon_offsets(context: &RuleContext<'_>) -> Vec<(usize, usize)> {
    let ranges = context.protected_ranges();
    let tokens: Vec<std::ops::Range<usize>> = context
        .nodes_of_any(SEMICOLON_BEARING_TOKENS)
        .map(|node| node.byte_range())
        .collect();
    let interpolations = Interpolations::new(context);
    let text = context.source.text();
    text.bytes()
        .enumerate()
        .filter(|(offset, byte)| {
            *byte == b';'
                && (!is_protected(*offset, ranges) || interpolations.holds_code(*offset))
                && !tokens.iter().any(|token| token.contains(offset))
        })
        .map(|(offset, _)| (context.source.line_column(offset).0, offset))
        .collect()
}

/// The semicolon RuboCop reports for `line_number`, if any.
///
/// Upstream inspects the line's token list and accepts a single position: the last token, the
/// first token, or a semicolon hugging a closing or opening brace. Comments are not tokens, so a
/// trailing comment does not stop a line from ending in a semicolon.
fn line_terminator_or_opener(
    context: &RuleContext<'_>,
    line_number: usize,
    semicolons: &[(usize, usize)],
) -> Option<usize> {
    let on_line: Vec<usize> = semicolons
        .iter()
        .filter(|(line, _)| *line == line_number)
        .map(|(_, offset)| *offset)
        .collect();
    if on_line.is_empty() {
        return None;
    }

    let code = code_range(context, line_number)?;
    let text = context.source.text();
    let last = on_line[on_line.len() - 1];
    let first = on_line[0];

    // A trailing comment is a token of its own to RuboCop, so it takes the last position away
    // from the semicolon and the line stops counting as one that ends in one.
    if last + 1 == code.end && !comment_follows(context, last) {
        return Some(last);
    }
    if first == code.start {
        return Some(first);
    }
    // `foo { ; }` and `"#{ ; }"`: the semicolon sits against the brace that opens or closes the
    // block, which upstream treats the same as sitting at the edge of the line.
    let after_last = text[last + 1..code.end].trim();
    if after_last == "}" {
        return Some(last);
    }
    // `exist_semicolon_before_right_string_interpolation_brace?`: the `}` closing an interpolation
    // is a `tSTRING_DEND`, and the string it sits in goes on past it -- so the line does not end
    // there, but the semicolon still terminates the last expression of the interpolation.
    if closes_an_interpolation(context, last) {
        return Some(last);
    }
    let before_first = text[code.start..first].trim_end();
    if before_first.ends_with('{') {
        return Some(first);
    }
    None
}

/// Whether the first thing after the semicolon is the brace that closes an interpolation.
fn closes_an_interpolation(context: &RuleContext<'_>, semicolon: usize) -> bool {
    let text = context.source.text();
    let rest = &text[semicolon + 1..];
    let brace = semicolon + 1 + (rest.len() - rest.trim_start().len());
    text[brace..].starts_with('}')
        && context
            .nodes_of("interpolation")
            .any(|node| node.end_byte() == brace + 1)
}

/// The byte range of `line_number` with comments and surrounding whitespace removed.
fn code_range(context: &RuleContext<'_>, line_number: usize) -> Option<std::ops::Range<usize>> {
    let text = context.source.text();
    let line = context.source.line_range(line_number);
    let comment_start = context
        .comment_ranges()
        .iter()
        .filter(|range| range.start >= line.start && range.start < line.end)
        .map(|range| range.start)
        .min()
        .unwrap_or(line.end);
    let slice = &text[line.start..comment_start];
    let start = line.start + (slice.len() - slice.trim_start().len());
    let end = line.start + slice.trim_end().len();
    (start < end).then_some(start..end)
}

/// Lines on which more than one statement ends, which is what makes a semicolon a separator.
fn expression_separator_lines(context: &RuleContext<'_>) -> HashSet<usize> {
    let mut lines = HashSet::new();
    for node in context.nodes_of_any(STATEMENT_SEQUENCE_KINDS) {
        let _cursor = node.walk();
        let children: Vec<_> = named_children_iter(node, context).collect();
        // `begin ... end` holds its statements directly upstream rather than wrapping them in the
        // `begin` node the cop looks for, so `begin a; b end` is not an offense. Add a `rescue` and
        // the protected body becomes such a node after all.
        if node.kind_str() == "begin"
            && !children
                .iter()
                .any(|child| matches!(child.kind_str(), "rescue" | "else" | "ensure"))
        {
            continue;
        }
        let mut ends: Vec<usize> = children
            .iter()
            .filter(|child| !NON_STATEMENT_KINDS.contains(&child.kind_str()))
            .map(|child| child.end_position().row + 1)
            .collect();
        ends.sort_unstable();
        lines.extend(
            ends.windows(2)
                .filter(|pair| pair[0] == pair[1])
                .map(|pair| pair[0]),
        );
    }
    lines
}

/// Whether a comment starts after `offset` on the same line.
fn comment_follows(context: &RuleContext<'_>, offset: usize) -> bool {
    let line = context
        .source
        .line_range(context.source.line_column(offset).0);
    context
        .comment_ranges()
        .iter()
        .any(|range| range.start > offset && range.start < line.end)
}

/// The `<<~FOO` openers of the file, as `(line, byte offset just past the opener)`.
fn heredoc_openers(context: &RuleContext<'_>) -> Vec<(usize, usize)> {
    context
        .nodes_of("heredoc_beginning")
        .map(|node| (node.start_position().row + 1, node.end_byte()))
        .collect()
}

/// Whether a heredoc opens on the semicolon's line, before the semicolon itself.
fn heredoc_opened_before(
    context: &RuleContext<'_>,
    openers: &[(usize, usize)],
    offset: usize,
) -> bool {
    if openers.is_empty() {
        return false;
    }
    let line_number = context.source.line_column(offset).0;
    openers
        .iter()
        .any(|(line, end)| *line == line_number && *end <= offset)
}

/// The space to drop and the span to bracket when the semicolon follows a value-omitted label.
///
/// `value_omission_pair_nodes` upstream; the pair's grandparent is the call whose selector the
/// space belongs to.
fn value_omission_before(
    context: &RuleContext<'_>,
    offset: usize,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let before = context.source.text()[..offset].trim_end().len();
    for pair in context.nodes_of("pair") {
        if pair.field("value").is_some() || pair.end_byte() != before {
            continue;
        }
        let list = pair.parent()?;
        if list.kind_str() != "argument_list" {
            continue;
        }
        let call = list.parent()?;
        if call.kind_str() != "call" {
            continue;
        }
        let selector = call.field("method")?;
        // A call already written with brackets needs neither edit.
        if list
            .child(0)
            .is_some_and(|first| context.source.node_text(first) == "(")
        {
            continue;
        }
        let _cursor = list.walk();
        let first = named_children_iter(list, context).next()?;
        return Some((
            selector.end_byte()..first.start_byte(),
            first.start_byte()..list.end_byte(),
        ));
    }
    None
}
