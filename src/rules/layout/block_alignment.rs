//! `Layout/BlockAlignment`.
//!
//! The `end` of a `do ... end` block may line up with two different things: the start of the
//! expression the block hangs off -- an assignment, a `def`, a chain of calls -- or the start of
//! the line the `do` itself was written on. `EnforcedStyleAlignWith` picks between them, and its
//! default `either` accepts both.
//!
//! Two pieces of the upstream cop carry most of the weight. `block_end_align_target` walks up from
//! the block for as long as the enclosing expression still begins on the same line, which is what
//! makes `x = y.map do` align its `end` with `x` rather than with `y`. And `do_line_anchor_loc`
//! moves the second alignment target off the `do`'s own line when that line is a continuation of a
//! multiline argument list, where its indentation was dictated by an opening bracket rather than
//! chosen.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{begins_its_line, body_statements, grouped_arguments};
use super::tokens::{Token, tokens};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `EnforcedStyleAlignWith`.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Style {
    Either,
    StartOfBlock,
    StartOfLine,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = match context
        .setting::<String>("EnforcedStyleAlignWith")
        .as_deref()
    {
        Some("start_of_block") => Style::StartOfBlock,
        Some("start_of_line") => Style::StartOfLine,
        _ => Style::Either,
    };
    for braces in context.nodes_of_any(&["block", "do_block"]) {
        // The `block` node upstream's parser builds spans the call as well, so it is the call --
        // or, for `-> {}`, the lambda -- that stands in for it here.
        let Some(node) = braces
            .parent()
            .filter(|parent| matches!(parent.kind_str(), "call" | "lambda"))
        else {
            continue;
        };
        let block = Block {
            context,
            node,
            braces,
        };
        if let Some(offense) = check_block_alignment(&block, style) {
            offenses.push(offense);
        }
    }
}

/// One block: the node upstream's parser calls a `block`, and the `do ... end` or `{ ... }` the
/// grammar hangs off it.
struct Block<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    node: Node<'tree>,
    braces: Node<'tree>,
}

impl<'tree> Block<'_, 'tree> {
    /// `node.loc.begin`: the `do` or the `{`.
    fn opening(&self) -> Option<Node<'tree>> {
        self.braces.child(0)
    }

    /// `node.loc.end`: the `end` or the `}`.
    fn closing(&self) -> Option<Node<'tree>> {
        let count = u32::try_from(self.braces.child_count()).ok()?;
        self.braces.child(count.checked_sub(1)?)
    }

    /// `node.send_node.source_range`: the call without the block hanging off it.
    fn send_range(&self) -> Range<usize> {
        let text = self.context.source.text();
        let start = self.node.start_byte();
        let end = &text[start..self.braces.start_byte()];
        start..(start + end.trim_end().len())
    }

    /// `node.send_node.selector`: the method name, or the `->` of a lambda literal.
    fn selector(&self) -> Option<Node<'tree>> {
        match self.node.kind_str() {
            "lambda" => self.node.child(0),
            _ => self.node.field("method"),
        }
    }

    /// `node.send_node.arguments + node.arguments`: what the call was given, and what the block
    /// takes.
    fn arguments(&self) -> Vec<Range<usize>> {
        let mut ranges: Vec<Range<usize>> = grouped_arguments(self.node)
            .into_iter()
            .map(|argument| argument.range)
            .collect();
        if let Some(parameters) = self
            .braces
            .field("parameters")
            .or_else(|| self.node.field("parameters"))
        {
            ranges.push(parameters.byte_range());
        }
        ranges
    }
}

/// `check_block_alignment`.
fn check_block_alignment(block: &Block<'_, '_>, style: Style) -> Option<Offense> {
    let context = block.context;
    let end = block.closing()?;
    if !begins_its_line(context, end.start_byte()) {
        return None;
    }
    let start = match style {
        Style::StartOfLine => start_for_line_node(block),
        _ => start_for_block_node(block),
    };
    let end_column = column_of(context, end.start_byte());
    if column_of(context, start.start) == end_column && style != Style::StartOfBlock {
        return None;
    }
    let anchor = compute_do_source_line_column(block, style, end_column)?;

    let preferred = match style {
        Style::StartOfBlock => anchor.clone(),
        _ => source_line_column(context, &start),
    };
    let message = format!(
        "{} is not aligned with {}{}.",
        format_source_line_column(&source_line_column(context, &(end.byte_range()))),
        format_source_line_column(&preferred),
        alternative(context, &start, &anchor, style),
    );

    let offense = context.offense(message, end.byte_range());
    let target = match style {
        Style::StartOfBlock => start_for_block_node(block),
        _ => start_for_line_node(block),
    };
    let start_column = match style {
        Style::StartOfBlock => anchor.column,
        _ => column_of(context, target.start),
    };
    let delta = start_column - end_column;
    Some(match delta {
        0 => offense,
        _ if delta > 0 => offense.corrected_by(Edit {
            start: end.start_byte(),
            end: end.start_byte(),
            replacement: " ".repeat(usize::try_from(delta).unwrap_or(0)),
            safe: true,
        }),
        _ => {
            let width = usize::try_from(-delta).unwrap_or(0);
            let removed = step_back(context, end.start_byte(), width);
            offense.corrected_by(Edit {
                start: removed,
                end: end.start_byte(),
                replacement: String::new(),
                safe: true,
            })
        }
    })
}

/// A location as the message spells it: the first line of the range's source, and where it starts.
#[derive(Clone)]
struct SourceLineColumn {
    source: String,
    line: usize,
    column: i64,
}

fn source_line_column(context: &RuleContext<'_>, range: &Range<usize>) -> SourceLineColumn {
    let (line, column) = context.source.line_column(range.start);
    SourceLineColumn {
        source: first_source_line(context, range).to_owned(),
        line,
        column: column as i64 - 1,
    }
}

fn format_source_line_column(location: &SourceLineColumn) -> String {
    format!(
        "`{}` at {}, {}",
        location.source, location.line, location.column
    )
}

/// `alt_start_msg`: with `either` the second target is named too, unless it is the same place.
fn alternative(
    context: &RuleContext<'_>,
    start: &Range<usize>,
    anchor: &SourceLineColumn,
    style: Style,
) -> String {
    if style != Style::Either {
        return String::new();
    }
    let (line, column) = context.source.line_column(start.start);
    if line == anchor.line && column as i64 - 1 == anchor.column {
        return String::new();
    }
    format!(" or {}", format_source_line_column(anchor))
}

/// `compute_do_source_line_column`: where the `do`'s line begins, unless that line is a
/// continuation of a bracketed argument list and so was never a target the author chose.
fn compute_do_source_line_column(
    block: &Block<'_, '_>,
    style: Style,
    end_column: i64,
) -> Option<SourceLineColumn> {
    let context = block.context;
    let opening = block.opening()?;
    let anchor = do_line_anchor(block, opening);
    let line = context.source.line_column(anchor).0;
    // `Range#source_line` drops the line feed and keeps a carriage return, which is what ends up
    // in the message.
    let text = context.source.line(line);
    let text = crate::rules::support::chomp(text);
    let indentation = text
        .chars()
        .position(|character| !character.is_whitespace())? as i64;

    let mut permitted = vec![indentation];
    if style == Style::Either {
        // The `do`'s own line still counts, so that code aligned the way the cop used to demand
        // does not become an offense.
        let opening_line = context
            .source
            .line(context.source.line_column(opening.start_byte()).0);
        if let Some(column) = opening_line
            .chars()
            .position(|character| !character.is_whitespace())
        {
            permitted.push(column as i64);
        }
    }
    if permitted.contains(&end_column) && style != Style::StartOfLine {
        return None;
    }
    Some(SourceLineColumn {
        source: text.chars().skip(indentation as usize).collect(),
        line,
        column: indentation,
    })
}

/// `do_line_anchor_loc`: the offset whose line the second alignment target is read off.
fn do_line_anchor(block: &Block<'_, '_>, opening: Node<'_>) -> usize {
    if !do_line_begins_inside_argument(block, opening) {
        return opening.start_byte();
    }
    block
        .selector()
        .map_or_else(|| block.send_range().start, |node| node.start_byte())
}

/// `do_line_begins_inside_argument?`: the `do` sits on a line that opened inside one of the call's
/// arguments, with a bracket still unclosed in front of it.
fn do_line_begins_inside_argument(block: &Block<'_, '_>, opening: Node<'_>) -> bool {
    let context = block.context;
    let line = context.source.line_column(opening.start_byte()).0;
    let text = context.source.line(line);
    let Some(offset) = text.find(|character: char| !character.is_whitespace()) else {
        return false;
    };
    let first = context.source.line_start(line) + offset;
    let tokens = tokens(context);
    if !inside_brackets(tokens, block.node.start_byte(), first) {
        return false;
    }
    block
        .arguments()
        .iter()
        .any(|argument| argument.start <= first && first < argument.end)
}

/// `inside_parentheses?`: more brackets were opened than closed between the block's own start and
/// the position. Only a `(` or a `[` counts -- a `{` is a literal or another block, never a
/// continuation the author was forced into.
fn inside_brackets(tokens: &[Token], from: usize, to: usize) -> bool {
    let mut depth = 0i64;
    for token in tokens {
        if token.range.start < from || token.range.start >= to {
            continue;
        }
        if token.opens_bracket() {
            depth += 1;
        } else if token.closes_bracket() {
            depth -= 1;
        }
    }
    depth > 0
}

/// What the walk out of the block stopped on.
///
/// A call that carries a block of its own is one node to the grammar and two to upstream's
/// parser -- a `send`, and the `block` wrapped around it. The walk can only ever reach the
/// `send`: `block_end_align_target?` does not accept a `block`, so the step after it always
/// stops. Keeping the distinction here is what makes `foo.map do end.to_h.reject { }` line its
/// `end` up with `foo`, the start of the `send`, rather than with whatever encloses the chain.
enum AlignTarget<'tree> {
    /// A node the grammar and the parser agree on.
    Node(Node<'tree>),
    /// A call with a block, reduced to the `send` upstream would have stopped on.
    Send(Node<'tree>),
}

impl<'tree> AlignTarget<'tree> {
    fn node(&self) -> Node<'tree> {
        match *self {
            Self::Node(node) | Self::Send(node) => node,
        }
    }

    /// The source range upstream reads off the node it stopped on.
    fn range(&self, context: &RuleContext<'_>) -> Range<usize> {
        match *self {
            Self::Node(node) => find_lhs_node(context, node),
            Self::Send(call) => send_range(context, call),
        }
    }
}

/// `node.send_node.source_range` for a call the walk stopped on: the call without its block.
fn send_range(context: &RuleContext<'_>, call: Node<'_>) -> Range<usize> {
    let Some(braces) = call.field("block") else {
        return call.byte_range();
    };
    let start = call.start_byte();
    let head = &context.source.text()[start..braces.start_byte()];
    start..(start + head.trim_end().len())
}

/// `start_for_block_node`: the expression the `end` belongs to, reduced to its left-hand side.
fn start_for_block_node(block: &Block<'_, '_>) -> Range<usize> {
    block_end_align_target(block).range(block.context)
}

/// `start_for_line_node`: the outermost expression that still begins on the align target's line.
fn start_for_line_node(block: &Block<'_, '_>) -> Range<usize> {
    let context = block.context;
    let target = block_end_align_target(block);
    let base = target.range(context);
    let line = context.source.line_column(base.start).0;
    let mut ancestors = parser_ancestors(target.node());
    if let AlignTarget::Send(call) = target {
        // The `block` upstream wraps around the `send` is its first ancestor. The grammar has the
        // two as one node, so it has to be put back before the chain reads the same.
        ancestors.insert(
            0,
            Ancestor {
                range: call.byte_range(),
                node: Some(call),
            },
        );
    }
    let outermost = ancestors
        .into_iter()
        .rev()
        .find(|ancestor| context.source.line_column(ancestor.range.start).0 == line);
    match outermost {
        Some(ancestor) => match ancestor.node {
            Some(node) => find_lhs_node(context, node),
            None => ancestor.range,
        },
        None => base,
    }
}

/// `find_lhs_node`: the message names the variable an operator assignment writes to rather than
/// the whole assignment.
///
/// Only `op_asgn` and `masgn` are reduced. `||=` and `&&=` are `or_asgn` and `and_asgn` to the
/// parser, which are neither, so `@a ||= foo do` keeps naming the whole assignment.
fn find_lhs_node(context: &RuleContext<'_>, node: Node<'_>) -> Range<usize> {
    let mut current = node;
    while is_operator_assignment(context, current)
        || (current.kind_str() == "assignment" && is_multiple_assignment(current))
    {
        let Some(left) = current.field("left") else {
            break;
        };
        current = left;
    }
    current.byte_range()
}

fn is_operator_assignment(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "operator_assignment"
        && node.field("operator").is_some_and(|operator| {
            !matches!(&context.source.text()[operator.byte_range()], "||=" | "&&=")
        })
}

fn is_multiple_assignment(node: Node<'_>) -> bool {
    node.field("left")
        .is_some_and(|left| left.kind_str() == "left_assignment_list")
}

/// `block_end_align_target`: walk out of the block for as long as the enclosing expression owns
/// the block's `end`, and stop at the first one that does not.
fn block_end_align_target<'tree>(block: &Block<'_, 'tree>) -> AlignTarget<'tree> {
    let mut current = block.node;
    while let Some(parent) = current.parent() {
        if !is_align_target(block.context, parent, current) {
            return AlignTarget::Node(current);
        }
        if parent.kind_str() == "call" && parent.field("block").is_some() {
            // Upstream reaches this call's `send` and stops at the `block` above it. See
            // `AlignTarget`.
            return AlignTarget::Send(parent);
        }
        current = parent;
    }
    AlignTarget::Node(current)
}

/// `end_align_target?` inverted: whether the parent takes the block's `end` over.
fn is_align_target(context: &RuleContext<'_>, parent: Node<'_>, node: Node<'_>) -> bool {
    // `disqualified_parent?`: an expression opening on an earlier line is not what the `end` lines
    // up with -- except for a multiple assignment, whose targets may be spread over several lines.
    let multiple = parent.kind_str() == "assignment" && is_multiple_assignment(parent);
    if !multiple
        && context.source.line_column(parent.start_byte()).0
            != context.source.line_column(node.start_byte()).0
    {
        return false;
    }
    match parent.kind_str() {
        // `assignment?` includes setter sends upstream. Tree-sitter keeps `foo.bar = x` and
        // `foo[0] = x` as assignments whose left side is a call/element reference, but their
        // outer assignment still owns a block nested in the value.
        "assignment" => true,
        "operator_assignment" => true,
        // `any_def`.
        "method" | "singleton_method" => true,
        // `splat`.
        "splat_argument" | "splat_parameter" => true,
        // `and`, `or` and `(send _ :<< ...)`.
        "binary" => parent.field("operator").is_some_and(|operator| {
            matches!(
                &context.source.text()[operator.byte_range()],
                "&&" | "||" | "and" | "or" | "<<"
            )
        }),
        // `(send equal?(%1) !:[] ...)`: a call written on the block, `foo.bar do end.baz`. An
        // index read is `:[]` and so excluded, which `element_reference` stands for.
        "call" => parent.field("receiver") == Some(node),
        // The same pattern, for the operators the parser also files as sends whose receiver is the
        // node: `~xyz { }` is `(send (block ...) :~)`. **The `end` lines up with the operator, not
        // with the method name** -- upstream's message names `~xyz { |x|` and its column, so a block
        // written under one has to keep walking out to the operator.
        "unary" => parent.field("operand") == Some(node),
        _ => false,
    }
}

/// One entry of the ancestor chain upstream's parser would have built.
struct Ancestor<'tree> {
    range: Range<usize>,
    /// The tree-sitter node, when the ancestor is one the grammar has as well.
    node: Option<Node<'tree>>,
}

/// The node kinds holding a statement list, which the parser only materializes as a `begin` once
/// the list holds more than one statement.
const STATEMENT_CONTAINERS: [&str; 7] = [
    "body_statement",
    "block_body",
    "then",
    "else",
    "do",
    "ensure",
    "program",
];

/// `node.each_ancestor`, expressed in the nodes upstream's parser builds: the grammar's argument
/// lists and block bodies have no counterpart there, and a statement list becomes a `begin` only
/// when it holds more than one statement.
fn parser_ancestors<'tree>(node: Node<'tree>) -> Vec<Ancestor<'tree>> {
    let mut ancestors = Vec::new();
    let mut current = node;
    while let Some(parent) = current.parent() {
        current = parent;
        match parent.kind_str() {
            // The grammar's own bookkeeping, which the parser folds into its parent.
            "argument_list" | "block" | "do_block" => {}
            kind if STATEMENT_CONTAINERS.contains(&kind) => {
                let statements = body_statements(parent);
                if statements.len() >= 2 {
                    ancestors.push(Ancestor {
                        range: statements[0].start_byte()
                            ..statements[statements.len() - 1].end_byte(),
                        node: None,
                    });
                }
            }
            _ => ancestors.push(Ancestor {
                range: parent.byte_range(),
                node: Some(parent),
            }),
        }
    }
    ancestors
}

/// The first line of a range's source, which is what `loc.source.lines.first.chomp` yields.
fn first_source_line<'a>(context: &'a RuleContext<'_>, range: &Range<usize>) -> &'a str {
    let text = &context.source.text()[range.clone()];
    let line = text.split('\n').next().unwrap_or(text);
    line.strip_suffix('\r').unwrap_or(line)
}

fn column_of(context: &RuleContext<'_>, offset: usize) -> i64 {
    context.source.line_column(offset).1 as i64 - 1
}

/// The offset `width` characters before `offset`, which is where `remove_space_before` starts.
fn step_back(context: &RuleContext<'_>, offset: usize, width: usize) -> usize {
    let text = context.source.text();
    let mut start = offset;
    for _ in 0..width {
        if start == 0 {
            break;
        }
        start -= 1;
        while start > 0 && !text.is_char_boundary(start) {
            start -= 1;
        }
    }
    start
}
