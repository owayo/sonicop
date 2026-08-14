//! Line counting shared by the length cops.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::node_ext::NodeExt;
use crate::rules::{RuleContext, push_named_children, walk_named};

/// What kind of construct a length cop measures. The variants differ in how the body is counted
/// and in which node RuboCop handed `check_code_length`, since that node fixes both the offense
/// location and the line span of the cheap pre-check. Naming the kind keeps those differences in
/// one place instead of spreading cop-name comparisons through the counting code.
#[derive(Clone, Copy)]
pub(super) enum LengthTarget<'tree> {
    /// Counted over its body and reported against itself: a method, or a `class << self`.
    ///
    /// `CodeLengthCalculator::CLASSLIKE_TYPES` holds `class` and `module` only, so a singleton
    /// class falls through to `extract_body` like a method does -- it is measured over its body
    /// rather than over its interior line range.
    Body,
    /// A class or module, counted over its interior with nested classes and modules removed.
    Classlike,
    /// A block, counted over its body and reported against the call that owns it.
    Block,
    /// `CONST = Module.new { ... }`, whose block body is what gets counted while RuboCop is handed
    /// the assignment: the pre-check spans the assignment and the offense lands on the constant,
    /// because `CodeLength#location` answers `loc.name` for a constant assignment.
    ConstantAssignment {
        assignment: Node<'tree>,
        name: Node<'tree>,
    },
}

/// Where each heredoc's terminator sits, keyed by the offset of the `<<~FOO` that opened it.
///
/// RuboCop's AST gives a heredoc node the range of its opener alone, and `CodeLengthCalculator`
/// reaches past that to `loc.heredoc_end` when a body holds one. tree-sitter instead hangs the
/// content off a `heredoc_body` sibling, so the two have to be paired back up. Openers and bodies
/// both appear in source order and Ruby stacks them in that same order, which is what makes
/// pairing them by rank correct.
pub(super) struct HeredocEnds(HashMap<usize, usize>);

impl HeredocEnds {
    pub(super) fn new(context: &RuleContext<'_>) -> Self {
        let bodies: Vec<Node<'_>> = context.nodes_of("heredoc_body").collect();
        Self(
            context
                .nodes_of("heredoc_beginning")
                .zip(bodies)
                .map(|(opener, body)| (opener.start_byte(), body.end_position().row))
                .collect(),
        )
    }

    fn end_row(&self, opener: Node<'_>) -> usize {
        self.0
            .get(&opener.start_byte())
            .copied()
            .unwrap_or_else(|| opener.end_position().row)
    }
}

/// Reports `node` when it holds more than `max` lines of code, in the shape RuboCop's length cops
/// use: `Method has too many lines. [12/10]`.
pub(super) fn report_length(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    max: usize,
    label: &str,
    target: LengthTarget<'_>,
    heredocs: &HeredocEnds,
) {
    let count_comments: bool = context.setting("CountComments").unwrap_or(false);
    if node.field("body").is_none() {
        return;
    }
    let location = match target {
        LengthTarget::Block => block_location(node),
        LengthTarget::ConstantAssignment { name, .. } => name,
        _ => node,
    };
    // RuboCop skips the count outright when the construct cannot span more than `max` lines. That
    // reads like a pure optimisation but is observable: a body ending in a heredoc is measured out
    // to the terminator, which can sit past the construct's own last line.
    let span = match target {
        LengthTarget::Block => location,
        LengthTarget::ConstantAssignment { assignment, .. } => assignment,
        _ => node,
    };
    let spanned_lines = span.end_position().row - span.start_position().row + 1;
    if spanned_lines <= max {
        return;
    }
    let length = match target {
        LengthTarget::Classlike => classlike_code_line_count(node, context, count_comments),
        _ => body_code_line_count(node, context, count_comments, heredocs),
    };
    if length <= max {
        return;
    }
    offenses.push(context.offense(
        format!("{label} has too many lines. [{length}/{max}]"),
        location.byte_range(),
    ));
}

/// The node a block's offense is reported against: RuboCop's `block` node starts at the call that
/// takes the block, or at the `->` of a lambda literal, never at the brace.
pub(super) fn block_location<'tree>(node: Node<'tree>) -> Node<'tree> {
    node.parent()
        .filter(|parent| matches!(parent.kind_str(), "call" | "lambda"))
        .unwrap_or(node)
}

/// The lines RuboCop counts for the body of a method or block.
///
/// RuboCop takes the *source of the body node* rather than the span of the enclosing definition,
/// and switches to whole source lines only when the body holds a heredoc. The two differ at both
/// ends, which is why the distinction is worth reproducing rather than approximating: a body that
/// is nothing but a heredoc measures one line (its opener), while a body whose last statement runs
/// past a heredoc terminator has to be followed out to that terminator.
fn body_code_line_count(
    node: Node<'_>,
    context: &RuleContext<'_>,
    count_comments: bool,
    heredocs: &HeredocEnds,
) -> usize {
    let Some(body) = node.field("body") else {
        return 0;
    };
    let statements = statements_of(body);
    let (Some(first), Some(last)) = (statements.first(), statements.last()) else {
        return 0;
    };
    let start = first.start_position().row;
    let end = heredoc_extended_end(&statements, heredocs).unwrap_or(last.end_position().row);
    count_code_lines(context, start, end, count_comments)
}

/// The statements RuboCop would see as the body, in the order they are written.
///
/// tree-sitter parks two kinds of node here that RuboCop has nothing at all for: a `heredoc_body`,
/// which is content rather than a statement, and a `comment`. Leaving either in place moves the
/// ends of the measured span -- a trailing `end # note` would push the last line past where the
/// body really stops -- and makes a one-statement body look like several, which changes which
/// nodes `heredoc_extended_end` treats as its own.
fn statements_of(body: Node<'_>) -> Vec<Node<'_>> {
    if !matches!(body.kind_str(), "body_statement" | "block_body") {
        return vec![body];
    }
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "heredoc_body" | "comment"))
        .collect()
}

/// The last line touched by anything *inside* the body, with heredocs followed out to their
/// terminator -- `None` when the body holds no heredoc and the plain body range applies.
///
/// A single statement stands in for RuboCop's body node itself, so only its descendants count and
/// its own closing `end` drops out of the span. Several statements are wrapped in a `begin` whose
/// descendants include the statements themselves, so those do count.
fn heredoc_extended_end(statements: &[Node<'_>], heredocs: &HeredocEnds) -> Option<usize> {
    let mut stack = Vec::new();
    if let [only] = statements {
        push_named_children(*only, &mut stack);
    } else {
        stack.extend(statements.iter().copied());
    }
    let mut found = false;
    let mut last_row = 0;
    while let Some(current) = stack.pop() {
        if current.kind_str() == "heredoc_beginning" {
            found = true;
            last_row = last_row.max(heredocs.end_row(current));
        } else if !outside_rubocop_ast(current.kind_str()) {
            last_row = last_row.max(current.end_position().row);
        }
        push_named_children(current, &mut stack);
    }
    found.then_some(last_row)
}

/// Node kinds whose own extent RuboCop's `each_descendant` never reaches.
///
/// A `do`/brace block and a parenthesized argument list both belong to the surrounding call in
/// RuboCop's tree, so their `end`, `}` and `)` are the *call's* closing tokens -- and a node's own
/// closing token is what this span deliberately stops short of. Counting these wrappers as if they
/// were children would put those tokens back, adding a line RuboCop never counts. A `comment` is
/// not part of the AST at all. Their contents are still visited; only their own extent is ignored.
fn outside_rubocop_ast(kind: &str) -> bool {
    matches!(kind, "block" | "do_block" | "argument_list" | "comment")
}

fn count_code_lines(
    context: &RuleContext<'_>,
    start_row: usize,
    end_row: usize,
    count_comments: bool,
) -> usize {
    (start_row..=end_row)
        .filter(|row| {
            let text = context.source.line(row + 1).trim();
            !text.is_empty() && (count_comments || !text.starts_with('#'))
        })
        .count()
}

fn classlike_code_line_count(
    node: Node<'_>,
    context: &RuleContext<'_>,
    count_comments: bool,
) -> usize {
    if is_namespace(node) {
        return 0;
    }
    let mut excluded_lines = HashSet::new();
    walk_named(node, &mut |descendant| {
        if descendant == node || !matches!(descendant.kind_str(), "class" | "module") {
            return;
        }
        let first = descendant.start_position().row + 1;
        let last = descendant.end_position().row + 1;
        excluded_lines.extend(first..=last);
    });

    // RuboCop's ProcessedSource is indexed from zero after constructing the
    // one-based interior line range. Preserve that observable offset exactly.
    let start = node.start_position().row + 2;
    let end = node.end_position().row;
    (start..=end)
        .filter(|line| {
            if excluded_lines.contains(line) {
                return false;
            }
            let text = context.source.line(*line + 1).trim();
            !text.is_empty() && (count_comments || !text.starts_with('#'))
        })
        .count()
}

/// Whether the class or module exists only to namespace a single class or module, which RuboCop
/// measures as zero lines however far apart the two `end`s are.
fn is_namespace(node: Node<'_>) -> bool {
    node.field("body")
        .map(statements_of)
        .is_some_and(|statements| {
            matches!(statements.as_slice(), [only] if matches!(only.kind_str(), "class" | "module"))
        })
}

/// The receiver constant and method name of `Const.method(...)`, for the receiver being a plain
/// (possibly `::`-rooted) constant -- RuboCop's `#global_const?` accepts exactly those two shapes,
/// so a namespaced `Foo::Struct` must not match.
pub(super) fn constructor_call<'a>(
    context: &'a RuleContext<'_>,
    call: Node<'_>,
) -> Option<(&'a str, &'a str)> {
    let receiver = call.field("receiver")?;
    let method = call.field("method")?;
    if !matches!(receiver.kind_str(), "constant" | "scope_resolution") {
        return None;
    }
    let text = context.source.node_text(receiver);
    let name = text.strip_prefix("::").unwrap_or(text);
    if name.contains("::") {
        return None;
    }
    Some((name, context.source.node_text(method)))
}
