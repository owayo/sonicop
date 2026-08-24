//! Scanning and node grouping shared by more than one Layout cop.

use std::collections::VecDeque;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
pub(super) use crate::rules::support::final_pos;

/// The run of spaces and tabs ending at `offset`.
pub(super) fn whitespace_before(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut start = offset;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    start..offset
}

/// The run of spaces and tabs starting at `offset`.
pub(super) fn whitespace_after(source: &str, offset: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut end = offset;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    offset..end
}

/// The hash literals of a file, each as the run of elements upstream's parser folds into one
/// `hash` node.
///
/// A braced hash is a node of its own here as well, but a brace-less one -- `foo(a: 1, b: 2)`,
/// `[a: 1]`, `foo[a: 1]` -- is not: the grammar leaves its pairs as siblings of whatever was
/// written before them, while upstream's parser wraps the trailing run of `key: value` pairs and
/// `**splat`s into a single `hash`. A cop written against `on_hash` has to see that run as one
/// literal or it measures alignment against the wrong first pair.
pub(super) fn hash_literals<'ctx, 'tree>(
    context: &'ctx RuleContext<'tree>,
) -> Vec<Vec<Node<'tree>>> {
    let mut literals: Vec<(usize, Vec<Node<'tree>>)> = Vec::new();
    for node in context.nodes_of("hash") {
        let mut cursor = node.walk();
        let elements: Vec<Node<'tree>> = node
            .named_children(&mut cursor)
            .filter(|child| is_hash_element(*child))
            .collect();
        if !elements.is_empty() {
            literals.push((node.start_byte(), elements));
        }
    }
    for container in context.nodes_of_any(&["argument_list", "array", "element_reference"]) {
        let mut cursor = container.walk();
        // A comment written between two pairs is a node here and nothing at all upstream, so it
        // must not break the run it sits in.
        let children: Vec<Node<'tree>> = container
            .named_children(&mut cursor)
            .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
            .collect();
        let mut index = 0;
        while index < children.len() {
            if !is_hash_element(children[index]) {
                index += 1;
                continue;
            }
            let start = index;
            while index < children.len() && is_hash_element(children[index]) {
                index += 1;
            }
            literals.push((
                children[start].start_byte(),
                children[start..index].to_vec(),
            ));
        }
    }
    literals.sort_by_key(|(start, _)| *start);
    literals.into_iter().map(|(_, elements)| elements).collect()
}

fn is_hash_element(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "pair" | "hash_splat_argument")
}

/// The comments upstream's lexer produced.
///
/// The grammar starts a `comment` node at any `#` in a heredoc body that is not opening an
/// interpolation, which is literal text there and never a comment. A comment written inside an
/// interpolation is a real one and sits below the `interpolation` node rather than directly under
/// the body.
pub(super) fn comments(context: &RuleContext<'_>) -> Vec<Range<usize>> {
    context
        .nodes_of("comment")
        .filter(|node| {
            node.parent_of(context)
                .is_none_or(|parent| parent.kind_str() != "heredoc_body")
        })
        .map(|node| node.byte_range())
        .collect()
}

/// `Util.begins_its_line?`: the first non-blank character of the line is where the node starts.
pub(super) fn begins_its_line(context: &RuleContext<'_>, offset: usize) -> bool {
    let line = context.source.line_column(offset).0;
    let start = context.source.line_start(line);
    context.source.text()[start..offset]
        .chars()
        .all(char::is_whitespace)
}

/// A set of `insert_before` and `remove` corrections over one node, collapsed into the single
/// replacement `Edit` carries.
pub(super) struct Edits<'a> {
    #[allow(dead_code)]
    text: &'a str,
    /// `(start, end, replacement)` triples, in the order they were recorded.
    parts: Vec<(usize, usize, String)>,
}

impl<'a> Edits<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        Self {
            text,
            parts: Vec::new(),
        }
    }

    /// `HashAlignment#adjust`: a positive delta pads before `offset`, a negative one eats that
    /// many characters off the padding already there.
    pub(super) fn adjust(&mut self, offset: usize, delta: i64) {
        match delta.cmp(&0) {
            std::cmp::Ordering::Greater => {
                let width = usize::try_from(delta).unwrap_or(0);
                self.parts.push((offset, offset, " ".repeat(width)));
            }
            std::cmp::Ordering::Less => {
                let width = usize::try_from(-delta).unwrap_or(0);
                let mut start = offset;
                for _ in 0..width {
                    if start == 0 {
                        break;
                    }
                    start -= 1;
                    while start > 0 && !self.text.is_char_boundary(start) {
                        start -= 1;
                    }
                }
                self.parts.push((start, offset, String::new()));
            }
            std::cmp::Ordering::Equal => {}
        }
    }

    /// The recorded corrections, in source order. Two that eat into the same padding would
    /// clobber each other upstream, which leaves the offense uncorrected rather than
    /// half-corrected.
    pub(super) fn finish(mut self) -> Vec<Edit> {
        self.parts
            .retain(|(start, end, replacement)| *start != *end || !replacement.is_empty());
        self.parts.sort_by_key(|(start, end, _)| (*start, *end));
        let mut cursor = 0;
        for (start, end, _) in &self.parts {
            if *start < cursor {
                return Vec::new();
            }
            cursor = *end;
        }
        self.parts
            .into_iter()
            .map(|(start, end, replacement)| Edit {
                start,
                end,
                replacement,
                safe: true,
            })
            .collect()
    }
}

/// One argument of a call, as `SendNode#arguments` hands it over: a single node, or the run of
/// `key: value` pairs and `**splat`s the parser folds into one brace-less `hash`.
pub(super) struct GroupedArgument<'tree> {
    pub(super) parts: Vec<Node<'tree>>,
    pub(super) range: Range<usize>,
    /// Whether the argument is the brace-less hash the parser synthesized.
    pub(super) hash_run: bool,
}

/// The arguments of a call, grouped the way upstream's parser presents them. An index read is a
/// call to `[]` there, so the nodes between its brackets are its arguments.
pub(super) fn grouped_arguments<'tree>(call: Node<'tree>) -> Vec<GroupedArgument<'tree>> {
    let mut cursor = call.walk();
    let children: Vec<Node<'tree>> = if call.kind_str() == "element_reference" {
        call.named_children(&mut cursor)
            .skip(1)
            .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
            .collect()
    } else {
        let Some(list) = call
            .children(&mut cursor)
            .find(|child| child.kind_str() == "argument_list")
        else {
            return Vec::new();
        };
        let mut inner = list.walk();
        list.named_children(&mut inner)
            .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
            .collect()
    };
    let mut arguments = Vec::new();
    let mut index = 0;
    while index < children.len() {
        if is_hash_element(children[index]) {
            let start = index;
            while index < children.len() && is_hash_element(children[index]) {
                index += 1;
            }
            let parts = children[start..index].to_vec();
            let range = parts[0].start_byte()..parts[parts.len() - 1].end_byte();
            arguments.push(GroupedArgument {
                parts,
                range,
                hash_run: true,
            });
        } else {
            arguments.push(GroupedArgument {
                parts: vec![children[index]],
                range: children[index].byte_range(),
                hash_run: false,
            });
            index += 1;
        }
    }
    arguments
}

/// The `Alignment` mixin: items measured against a base column, and the offenses the ones that
/// miss it turn into.
///
/// `register_offense` consults the cop's own offense list, so one instance stands for one cop's
/// pass over one file.
pub(super) struct AlignmentPass {
    /// `@current_offenses`: the ranges this cop has already reported for the file.
    reported: Vec<Range<usize>>,
}

impl AlignmentPass {
    pub(super) fn new() -> Self {
        Self {
            reported: Vec::new(),
        }
    }

    /// `Alignment#each_bad_alignment`: the items that begin a line of their own at a column other
    /// than `base`, each with the delta that would put it right. An item sharing its line with the
    /// item before it is left to whichever cop owns line breaks.
    pub(super) fn misaligned(
        context: &RuleContext<'_>,
        items: &[Range<usize>],
        base: i64,
    ) -> Vec<(Range<usize>, i64)> {
        let mut previous_line = 0;
        let mut found = Vec::new();
        for item in items {
            let line = context.source.line_column(item.start).0;
            if line > previous_line && begins_its_line(context, item.start) {
                let delta = base - display_column(context, item.start);
                if delta != 0 {
                    found.push((item.clone(), delta));
                }
            }
            previous_line = line;
        }
        found
    }

    /// `Alignment#register_offense`: an item lying inside a span this cop is already realigning is
    /// reported without a correction of its own, since two rewrites of one area by the same cop
    /// cannot be composed. The next pass finds the offense again and corrects it then.
    ///
    /// `correct` is the range handed to `AlignmentCorrector`, which is the reported item for every
    /// cop but `Layout/FirstArgumentIndentation`.
    pub(super) fn register(
        &mut self,
        context: &RuleContext<'_>,
        item: Range<usize>,
        correct: Range<usize>,
        delta: i64,
        message: impl FnOnce(bool) -> String,
        offenses: &mut Vec<Offense>,
    ) {
        let nested = self
            .reported
            .iter()
            .any(|outer| item.start >= outer.start && item.end <= outer.end);
        let mut offense = context.offense(message(nested), item.clone());
        if !nested && !holds_block_comment(context, &correct) {
            let taboo = string_interiors(context, &correct);
            offense =
                offense.corrected_by_all(alignment_corrections(context, correct, delta, &taboo));
        }
        self.reported.push(item);
        offenses.push(offense);
    }
}

/// The parameters of a method definition, in the shape upstream's parser gives them.
///
/// The grammar reads `def m(a = nil, b = nil)` as one `optional_parameter` whose default is the
/// multiple assignment `nil, b = nil`, because `nil` is spelled the same as an assignment target
/// there. Upstream has two `optarg`s, so the run has to be unfolded before a cop can say where each
/// parameter begins.
pub(super) fn definition_parameters(definition: Node<'_>) -> Vec<Range<usize>> {
    let Some(list) = definition.field("parameters") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    let mut found = Vec::new();
    for child in list.named_children(&mut cursor) {
        if matches!(child.kind_str(), "comment" | "heredoc_body") {
            continue;
        }
        match unfolded_defaults(child) {
            Some(parameters) => found.extend(parameters),
            None => found.push(child.byte_range()),
        }
    }
    found
}

/// The parameters one `optional_parameter` node really stands for, or `None` when its default is
/// the expression it looks like.
///
/// Each `left_assignment_list` in the folded chain holds the previous parameter's default followed
/// by the name the fold swallowed, so unwinding it recovers the pairs the source spells out.
fn unfolded_defaults(parameter: Node<'_>) -> Option<Vec<Range<usize>>> {
    if parameter.kind_str() != "optional_parameter" {
        return None;
    }
    let name = parameter.field("name")?;
    let mut current = parameter.field("value")?;
    folded_targets(current)?;

    let mut pending: VecDeque<Node<'_>> = VecDeque::from([name]);
    let mut found: Vec<Range<usize>> = Vec::new();
    loop {
        let Some(targets) = folded_targets(current) else {
            if let Some(name) = pending.pop_front() {
                found.push(name.start_byte()..current.end_byte());
            }
            return Some(found);
        };
        let Some((default, names)) = targets.split_first() else {
            return Some(found);
        };
        if let Some(name) = pending.pop_front() {
            found.push(name.start_byte()..default.end_byte());
        }
        pending.extend(names.iter().copied());
        let Some(right) = current.field("right") else {
            return Some(found);
        };
        current = right;
    }
}

/// The targets of the multiple assignment a folded run of defaults was read as. `def m(x = y = 1)`
/// assigns for real and has a single target, which is what tells the two apart.
fn folded_targets<'tree>(value: Node<'tree>) -> Option<Vec<Node<'tree>>> {
    if value.kind_str() != "assignment" {
        return None;
    }
    let left = value.field("left")?;
    if left.kind_str() != "left_assignment_list" {
        return None;
    }
    let mut cursor = left.walk();
    Some(left.named_children(&mut cursor).collect())
}

/// `Alignment#display_column`: how far into its line a range starts, measured the way a terminal
/// would render it.
pub(super) fn display_column(context: &RuleContext<'_>, offset: usize) -> i64 {
    let line = context.source.line_column(offset).0;
    let start = context.source.line_start(line);
    crate::display_width::display_width(&context.source.text()[start..offset])
}

/// `AlignmentCorrector.calculate_range` for a leftward move: the indentation the line gives back,
/// or `None` when it has less of it than `width` and so keeps what it has.
///
/// Upstream measures that span in *characters* from `line_begin` and then removes it only when
/// `/\A[ \t]+\z/` matches its source, so a span that would reach past the blanks into code is left
/// alone. A blank is one byte, so taking the blanks the line actually has and asking for the full
/// width back says exactly the same thing in bytes -- and, unlike stepping `width` bytes off
/// `line_begin`, it can never land inside a multi-byte character. `column_delta` is a display
/// width (`Alignment#display_column`) rather than a count of anything storable, so a single `"日"`
/// on the line is enough to make the two disagree and, before this, to abort the thread on a
/// slice that is not on a character boundary.
fn removable_indentation(text: &str, line_begin: usize, width: usize) -> Option<Range<usize>> {
    // `starts_with_space`: a line that opens with indentation gives it back from its front, and a
    // node that starts its line without one takes it off the blanks written before it.
    let range = if text[line_begin..].starts_with(' ') {
        let run = whitespace_after(text, line_begin);
        line_begin..line_begin.checked_add(width)?.min(run.end)
    } else {
        let run = whitespace_before(text, line_begin);
        line_begin.saturating_sub(width).max(run.start)..line_begin
    };
    (range.end - range.start == width).then_some(range)
}

/// `AlignmentCorrector.correct`: every line the node spans is moved sideways by `column_delta`.
pub(super) fn alignment_corrections(
    context: &RuleContext<'_>,
    expr: Range<usize>,
    column_delta: i64,
    taboo: &[Range<usize>],
) -> Vec<Edit> {
    if column_delta == 0 {
        return Vec::new();
    }
    let text = context.source.text();
    let mut edits = Vec::new();
    let mut line_begin = expr.start;
    for line in text[expr.clone()].split_inclusive('\n') {
        // The first position is the node's own start rather than its line's, which is what lets a
        // node that shares its line with something else be moved on its own.
        let range = if column_delta > 0 {
            Some(line_begin..line_begin)
        } else {
            removable_indentation(
                text,
                line_begin,
                usize::try_from(-column_delta).unwrap_or(0),
            )
        };
        let start = line_begin;
        line_begin += line.len();
        let Some(range) = range else {
            continue;
        };
        if taboo
            .iter()
            .any(|range_| range.start >= range_.start && range.end <= range_.end)
        {
            continue;
        }
        if column_delta > 0 {
            if !text[start..].starts_with('\n') {
                let width = usize::try_from(column_delta).unwrap_or(0);
                edits.push(Edit {
                    start,
                    end: start,
                    replacement: " ".repeat(width),
                    safe: true,
                });
            }
        } else {
            edits.push(Edit {
                start: range.start,
                end: range.end,
                replacement: String::new(),
                safe: true,
            });
        }
    }
    edits
}

/// The spans `AlignmentCorrector` refuses to move: the text inside a string literal, and the body
/// and terminator of a heredoc.
pub(super) fn string_interiors(
    context: &RuleContext<'_>,
    expr: &Range<usize>,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for node in context.nodes_of_any(&["string", "subshell"]) {
        if node.start_byte() < expr.start || node.end_byte() > expr.end {
            continue;
        }
        let count = node.child_count();
        if count < 2 {
            continue;
        }
        let (Some(first), Some(last)) = (
            node.child(0),
            node.child(u32::try_from(count).unwrap_or(0).saturating_sub(1)),
        ) else {
            continue;
        };
        if first.end_byte() <= last.start_byte() {
            ranges.push(first.end_byte()..last.start_byte());
        }
    }
    for node in context.nodes_of("heredoc_body") {
        if node.end_byte() > expr.start && node.start_byte() < expr.end {
            ranges.push(node.byte_range());
        }
    }
    ranges
}

/// Whether a `=begin` block comment lies inside the span, which stops the correction outright.
pub(super) fn holds_block_comment(context: &RuleContext<'_>, expr: &Range<usize>) -> bool {
    context.comment_ranges().iter().any(|comment| {
        comment.start >= expr.start
            && comment.end <= expr.end
            && context.source.text()[comment.clone()].starts_with("=begin")
    })
}

/// The literals of `kinds` written directly as arguments of `call`, paired with the call's opening
/// parenthesis.
///
/// This is `each_argument_node`, which walks each argument's subtree but stops at anything that is
/// a method call upstream -- so a literal nested inside another call belongs to that call instead.
pub(super) fn argument_literals<'tree>(
    context: &RuleContext<'_>,
    call: Node<'tree>,
    kinds: &[&str],
) -> Vec<(Node<'tree>, Node<'tree>)> {
    let mut cursor = call.walk();
    let Some(list) = call
        .children(&mut cursor)
        .find(|child| child.kind_str() == "argument_list")
    else {
        return Vec::new();
    };
    let Some(parenthesis) = list.child(0).filter(|child| child.kind_str() == "(") else {
        return Vec::new();
    };
    let parenthesis_line = context.source.line_column(parenthesis.start_byte()).0;

    let mut found = Vec::new();
    for argument in grouped_arguments(call) {
        for part in argument.parts {
            let mut stack = vec![part];
            while let Some(node) = stack.pop() {
                if kinds.contains(&node.kind_str()) {
                    if let Some(open) = literal_opening(node) {
                        if context.source.line_column(open.start_byte()).0 == parenthesis_line {
                            found.push((node, parenthesis));
                        }
                    }
                }
                if is_send_like(context, node) {
                    continue;
                }
                let mut inner = node.walk();
                stack.extend(node.named_children(&mut inner));
            }
        }
    }
    found
}

/// `loc.begin` of a literal: the brace, bracket or percent-literal opener it was written with.
pub(super) fn literal_opening<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let first = node.child(0)?;
    matches!(first.kind_str(), "{" | "[" | "%w(" | "%i(").then_some(first)
}

/// Whether upstream's parser calls the node a `send`, which is where `on_node`'s walk stops.
pub(super) fn is_send_like(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "call" | "element_reference" | "method_call" => true,
        "binary" => node.field("operator").is_some_and(|operator| {
            !matches!(
                &context.source.text()[operator.byte_range()],
                "&&" | "||" | "and" | "or"
            )
        }),
        "unary" => node
            .child(0)
            .is_some_and(|operator| matches!(operator.kind_str(), "!" | "-" | "+" | "~" | "not")),
        _ => false,
    }
}

/// What the first element's indentation is measured against.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum IndentBase {
    LeftBraceOrBracket,
    FirstColumnAfterLeftParenthesis,
    ParentHashKey,
    StartOfLine,
}

/// `MultilineElementIndentation#indent_base`.
pub(super) fn indent_base(
    context: &RuleContext<'_>,
    open: Node<'_>,
    first: Option<Node<'_>>,
    parenthesis: Option<Node<'_>>,
    style: &str,
    brace_style: &str,
) -> (i64, IndentBase) {
    if style == brace_style {
        return (
            character_column(context, open.start_byte()),
            IndentBase::LeftBraceOrBracket,
        );
    }
    if let Some(pair) = parent_pair(open, first) {
        if key_and_value_begin_on_same_line(pair) && right_sibling_begins_later(pair) {
            return (
                character_column(context, pair.start_byte()),
                IndentBase::ParentHashKey,
            );
        }
    }
    if let Some(parenthesis) = parenthesis {
        if style == "special_inside_parentheses" {
            return (
                character_column(context, parenthesis.start_byte()) + 1,
                IndentBase::FirstColumnAfterLeftParenthesis,
            );
        }
    }
    (
        line_indentation(context, open.start_byte()),
        IndentBase::StartOfLine,
    )
}

/// `hash_pair_where_value_beginning_with`: the literal is the value of an enclosing pair.
fn parent_pair<'tree>(open: Node<'_>, first: Option<Node<'tree>>) -> Option<Node<'tree>> {
    let first = first?;
    let literal = first.parent()?;
    if literal_opening(literal) != Some(open) {
        return None;
    }
    literal
        .parent()
        .filter(|parent| parent.kind_str() == "pair")
}

fn key_and_value_begin_on_same_line(pair: Node<'_>) -> bool {
    let (Some(key), Some(value)) = (pair.field("key"), pair.field("value")) else {
        return false;
    };
    key.start_position().row == value.start_position().row
}

fn right_sibling_begins_later(pair: Node<'_>) -> bool {
    let mut sibling = pair.next_named_sibling();
    while sibling.is_some_and(|node| matches!(node.kind_str(), "comment" | "heredoc_body")) {
        sibling = sibling.and_then(|node| node.next_named_sibling());
    }
    sibling.is_some_and(|sibling| pair.end_position().row < sibling.start_position().row)
}

/// A zero-based character column, which is the unit every `loc.column` is in.
pub(super) fn character_column(context: &RuleContext<'_>, offset: usize) -> i64 {
    context.source.line_column(offset).1 as i64 - 1
}

/// `effective_column`: the column as an editor shows it. Only `column_offset_between` goes through
/// this, so a cop measuring one column against another discounts a byte order mark on line 1 while
/// the columns it **reports** keep it.
pub(super) fn effective_character_column(context: &RuleContext<'_>, offset: usize) -> i64 {
    context.source.effective_column(offset) as i64 - 1
}

/// `source_line =~ /\S/`: where the line's first non-blank character sits.
pub(super) fn line_indentation(context: &RuleContext<'_>, offset: usize) -> i64 {
    let line = context.source.line_column(offset).0;
    let text = context.source.line(line);
    text.chars()
        .take_while(|character| character.is_whitespace() && *character != '\n')
        .count() as i64
}

/// Whether anything but blanks precedes `offset` on its line.
pub(super) fn preceded_by_code(context: &RuleContext<'_>, offset: usize) -> bool {
    !begins_its_line(context, offset)
}

/// A statement list upstream's parser wraps in a `begin` or `kwbegin` node.
///
/// The grammar has a container node for every body -- `body_statement`, `block_body`, `then`,
/// `else`, `do`, `ensure` -- while the parser only materializes a `begin` once a body holds more
/// than one statement. A parenthesized group is a `begin` even with a single statement in it, and
/// `begin ... end` is a `kwbegin` that carries its statements directly.
pub(super) struct StatementGroup<'tree> {
    pub(super) statements: Vec<Node<'tree>>,
    /// Where the node upstream would call this group's parent starts, if it has one.
    pub(super) parent_start: Option<usize>,
}

/// Node kinds that hold a statement list.
const STATEMENT_CONTAINERS: [&str; 6] = [
    "body_statement",
    "block_body",
    "then",
    "else",
    "do",
    "ensure",
];

pub(super) fn statement_groups<'ctx, 'tree>(
    context: &'ctx RuleContext<'tree>,
) -> Vec<StatementGroup<'tree>> {
    let mut groups = Vec::new();
    let mut push = |container: Node<'tree>, always: bool| {
        let statements = body_statements(container);
        if statements.is_empty() || (!always && statements.len() < 2) {
            return;
        }
        // A body with a `rescue` or `ensure` clause files its statements under that clause
        // upstream, so the group's parent is the clause rather than the body's own owner.
        let parent_start = if has_clause(container) {
            Some(statements[0].start_byte())
        } else {
            container.parent_of(context).map(parser_node_start)
        };
        groups.push(StatementGroup {
            statements,
            parent_start,
        });
    };
    for container in context.nodes_of_any(&STATEMENT_CONTAINERS) {
        push(container, false);
    }
    // A parenthesized group is a `begin` even with a single statement in it, and so is the code
    // inside a `#{...}`: the parser hangs every interpolation off a `begin` node.
    for container in context.nodes_of_any(&["parenthesized_statements", "interpolation"]) {
        push(container, true);
    }
    for container in context.nodes_of("program") {
        let statements = body_statements(container);
        if statements.len() >= 2 {
            groups.push(StatementGroup {
                statements,
                parent_start: None,
            });
        }
    }
    for container in context.nodes_of("begin") {
        let statements = body_statements(container);
        if statements.is_empty() {
            continue;
        }
        if has_clause(container) {
            // The `kwbegin` then holds only the clause node, which is one child and never
            // misaligned; the statements before it are their own `begin`.
            if statements.len() >= 2 {
                groups.push(StatementGroup {
                    statements,
                    parent_start: Some(container.start_byte()),
                });
            }
            continue;
        }
        groups.push(StatementGroup {
            statements,
            parent_start: container.parent_of(context).map(parser_node_start),
        });
    }
    groups
}

/// The statements of a body, without the clause nodes and the grammar's own bookkeeping.
pub(super) fn body_statements<'tree>(container: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = container.walk();
    container
        .named_children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind_str(),
                "rescue" | "ensure" | "else" | "heredoc_body" | "comment" | "empty_statement"
            )
        })
        .collect()
}

/// Where the node upstream's parser builds for `node` starts. A block literal is a `block` node
/// there that spans the call it hangs off, so it begins at the receiver rather than at `do`.
pub(super) fn parser_node_start(node: Node<'_>) -> usize {
    match node.kind_str() {
        "block" | "do_block" => node.parent().unwrap_or(node).start_byte(),
        _ => node.start_byte(),
    }
}

/// `first_part_of_call_chain`: the receiver a chain of calls hangs off, which is what an
/// assignment's right-hand side really begins with.
pub(super) fn first_part_of_call_chain(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node;
    while current.kind_str() == "call" {
        current = current.field("receiver")?;
    }
    Some(current)
}

/// `node.loc.end`: the `end` keyword a construct closes with. A loop keeps it inside the body
/// node the grammar gives it, so the token is one level further down there than for everything
/// else.
pub(super) fn end_keyword<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let last = last_child(node)?;
    if last.kind_str() == "end" {
        return Some(last);
    }
    last_child(last).filter(|inner| last.kind_str() == "do" && inner.kind_str() == "end")
}

fn last_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)
}

/// `start_line_range`: the line an offset sits on, without its indentation or its trailing blanks.
pub(super) fn start_line_range(context: &RuleContext<'_>, offset: usize) -> Range<usize> {
    let line = context.source.line_column(offset).0;
    let start = context.source.line_start(line);
    let text = context.source.line(line);
    let first = text.len() - text.trim_start().len();
    let last = text.trim_end().len();
    (start + first)..(start + last.max(first))
}

/// `EndKeywordAlignment#check_end_kw_alignment` and the `AlignmentCorrector.align_end` it corrects
/// with: an `end` is aligned when it shares a line with what it belongs to or opens at the same
/// column, and is moved to `align_to` when it does not.
pub(super) fn end_keyword_alignment(
    context: &RuleContext<'_>,
    end: Range<usize>,
    base: Range<usize>,
    align_to: i64,
) -> Option<Offense> {
    let end_line = context.source.line_column(end.start).0;
    let end_column = character_column(context, end.start);
    let base_line = context.source.line_column(base.start).0;
    let base_column = character_column(context, base.start);
    if end_line == base_line || end_column == base_column {
        return None;
    }
    let message = format!(
        "`end` at {end_line}, {end_column} is not aligned with `{}` at {base_line}, {base_column}.",
        &context.source.text()[base]
    );
    // `whitespace_range`: everything on the `end`'s line before it.
    let whitespace = context.source.line_start(end_line)..end.start;
    let filler = match context
        .setting_of::<String>("Layout/IndentationStyle", "EnforcedStyle")
        .as_deref()
    {
        Some("tabs") => "\t",
        _ => " ",
    };
    let indentation = filler.repeat(usize::try_from(align_to).unwrap_or(0));
    let offense = context.offense(message, end);
    Some(
        match context.source.text()[whitespace.clone()].trim().is_empty() {
            true => offense.corrected_by(Edit {
                start: whitespace.start,
                end: whitespace.end,
                replacement: indentation,
                safe: true,
            }),
            false => offense
                .corrected_by(Edit {
                    start: whitespace.end,
                    end: whitespace.end,
                    replacement: format!("\n{indentation}"),
                    safe: true,
                })
                .corrections_anchored_at(whitespace),
        },
    )
}

/// Every heredoc of the file, as the offset of its opener paired with the range of its terminator.
///
/// The opener and the body are far apart in the tree -- the body hangs off the statement the
/// opener was written in -- but both appear in source order, so the nth of one belongs to the nth
/// of the other.
pub(super) fn heredoc_terminators(context: &RuleContext<'_>) -> Vec<(usize, Range<usize>)> {
    let openers: Vec<usize> = context
        .nodes_of("heredoc_beginning")
        .map(|node| node.start_byte())
        .collect();
    if openers.is_empty() {
        return Vec::new();
    }
    context
        .nodes_of("heredoc_body")
        .enumerate()
        .filter_map(|(index, body)| {
            let opener = *openers.get(index)?;
            let mut cursor = body.walk();
            let terminator = body
                .named_children(&mut cursor)
                .find(|child| child.kind_str() == "heredoc_end")?;
            Some((opener, terminator.byte_range()))
        })
        .collect()
}

fn has_clause(container: Node<'_>) -> bool {
    let mut cursor = container.walk();
    container
        .named_children(&mut cursor)
        .any(|child| matches!(child.kind_str(), "rescue" | "ensure" | "else"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::config::Config;
    use crate::engine::{self, CorrectMode, Selection};

    use super::removable_indentation;

    /// The default configuration, resolved from nothing on disk so that the repository the tests
    /// run in cannot decide what they check.
    fn default_config() -> Config {
        Config::load_with_options(None, Path::new("/"), true)
            .expect("the embedded default configuration has to load")
    }

    fn corrected(cop: &str, source: &str) -> String {
        let config = default_config();
        let selection = Selection {
            only: vec![cop.to_owned()],
            correcting: true,
            ..Selection::default()
        };
        let report = engine::inspect_source("example.rb", source.to_owned(), &config, &selection)
            .expect("the source has to be inspectable");
        let (_, corrected, _) =
            engine::correct_until_stable(report, CorrectMode::All, &config, &selection)
                .expect("the correction has to run");
        corrected
    }

    fn offense_lines(cop: &str, source: &str) -> Vec<String> {
        let config = default_config();
        let selection = Selection {
            only: vec![cop.to_owned()],
            ..Selection::default()
        };
        let report = engine::inspect_source("example.rb", source.to_owned(), &config, &selection)
            .expect("the source has to be inspectable");
        report
            .offenses
            .iter()
            .map(|offense| {
                let (line, column) = report.source.line_column(offense.start);
                format!("{line}:{column} {} {}", offense.cop_name, offense.message)
            })
            .collect()
    }

    /// `column_delta` is a display width, so counting that many *bytes* off `line_begin` walked
    /// into the middle of a multi-byte character and aborted the thread the file was inspected on
    /// -- which, under the parallel runner, took the rayon worker and the reports of unrelated
    /// files with it.
    #[test]
    fn indentation_is_measured_over_the_blanks_that_are_there_rather_than_in_bytes() {
        // Five blanks to give and three columns asked for: the first three are the indentation.
        assert_eq!(removable_indentation("     [1,", 0, 3), Some(0..3));
        // One blank to give and three columns asked for. Reaching three bytes ahead lands inside
        // `日`; upstream reads three characters, sees ` "日` rather than blanks, and removes
        // nothing.
        assert_eq!(removable_indentation(" \"\u{65e5}\"]", 0, 3), None);
        // The same from the other side. `line_begin` is the second line, whose first character is
        // not a blank, so the span is taken off what precedes it -- and four bytes back is inside
        // the `日` on the line above.
        let across_lines = "\"\u{65e5}\",\n\"x\"";
        assert_eq!(across_lines.find('\n'), Some(6));
        assert_eq!(removable_indentation(across_lines, 7, 4), None);
        assert_eq!(removable_indentation("a\n    x", 6, 3), Some(3..6));
        // Fewer blanks than columns asked for is left alone, not shortened.
        assert_eq!(removable_indentation("a\n  x", 4, 3), None);
    }

    /// The two files the panic was first seen on, end to end: the offense stays where RuboCop
    /// 1.89.0 reports it and the correction is byte for byte what RuboCop 1.89.0 writes.
    #[test]
    fn a_multi_byte_character_on_a_realigned_line_neither_panics_nor_moves() {
        let ends_in_a_wide_character = "def m\n     [1,\n \"\u{65e5}\"]\nend\n";
        assert_eq!(
            offense_lines("Layout/IndentationWidth", ends_in_a_wide_character),
            ["2:1 Layout/IndentationWidth Use 2 (not 5) spaces for indentation."]
        );
        assert_eq!(
            corrected("Layout/IndentationWidth", ends_in_a_wide_character),
            "def m\n  [1,\n \"\u{65e5}\"]\nend\n"
        );

        let starts_after_a_wide_character = "def m\n      [1, \"\u{65e5}\",\n\"x\"]\nend\n";
        assert_eq!(
            offense_lines("Layout/IndentationWidth", starts_after_a_wide_character),
            ["2:1 Layout/IndentationWidth Use 2 (not 6) spaces for indentation."]
        );
        assert_eq!(
            corrected("Layout/IndentationWidth", starts_after_a_wide_character),
            "def m\n  [1, \"\u{65e5}\",\n\"x\"]\nend\n"
        );
    }

    /// Every width class the display-width table distinguishes, so that a fix aimed at one of them
    /// is not mistaken for a fix of the byte arithmetic.
    #[test]
    fn every_multi_byte_width_class_survives_realignment() {
        for character in ["\u{65e5}", "\u{e9}", "\u{1f600}", "\u{1d518}"] {
            let source = format!("def m\n     [1,\n \"{character}\"]\nend\n");
            assert_eq!(
                corrected("Layout/IndentationWidth", &source),
                format!("def m\n  [1,\n \"{character}\"]\nend\n"),
                "{character} was not left where RuboCop leaves it"
            );
        }
    }
}
