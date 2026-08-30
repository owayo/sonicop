//! `Layout/EndAlignment`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    character_column, end_keyword, end_keyword_alignment, first_part_of_call_chain,
    start_line_range,
};
use crate::diagnostic::Offense;
use crate::rules::node_ext::NodeExt;
use crate::rules::{RuleContext, push_named_children_in};

/// `EnforcedStyleAlignWith`.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Style {
    Keyword,
    Variable,
    StartOfLine,
}

/// The node kinds upstream calls `conditional?`, minus the ones that carry no `end`.
const CONDITIONAL_KINDS: [&str; 6] = ["if", "unless", "while", "until", "case", "case_match"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = match context
        .setting::<String>("EnforcedStyleAlignWith")
        .as_deref()
    {
        Some("variable") => Style::Variable,
        Some("start_of_line") => Style::StartOfLine,
        _ => Style::Keyword,
    };
    let mut checker = Checker {
        context,
        style,
        ignored: HashSet::new(),
        reported: HashSet::new(),
    };
    let mut stack = vec![context.root_node()];
    while let Some(node) = stack.pop() {
        checker.visit(node, offenses);
        push_named_children_in(node, context, &mut stack);
    }
}

struct Checker<'a, 'b> {
    context: &'a RuleContext<'b>,
    style: Style,
    /// `ignore_node`: the conditionals already checked against their assignment.
    ignored: HashSet<usize>,
    reported: HashSet<(usize, usize)>,
}

impl Checker<'_, '_> {
    fn visit(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        match node.kind_str() {
            "class" | "module" | "if" | "unless" | "while" | "until" => {
                self.check_other_alignment(node, offenses);
            }
            // `class << self` written as the right-hand side of an assignment aligns like one.
            "singleton_class" => match self.assignment_parent(node) {
                Some(outer) => self.check_asgn_alignment(outer, node, offenses),
                None => self.check_other_alignment(node, offenses),
            },
            "case" | "case_match" => match self.argument_owner(node) {
                Some(owner) => self.check_asgn_alignment(owner, node, offenses),
                None => self.check_other_alignment(node, offenses),
            },
            "assignment" | "operator_assignment" | "call" => self.check_assignment(node, offenses),
            // `variable + if ... end` is a `send` upstream, so `CheckAssignment#on_send` reads its
            // last argument. The grammar spells a binary operator as its own kind -- except for
            // `and` / `or`, which upstream keeps out of `on_send` as separate node types.
            "binary" => {
                let logical = node.field("operator").is_some_and(|operator| {
                    matches!(
                        self.context.source.node_text(operator),
                        "&&" | "||" | "and" | "or"
                    )
                });
                if !logical {
                    self.check_assignment(node, offenses);
                }
            }
            _ => {}
        }
    }

    /// `check_assignment`: an assignment whose right-hand side is a conditional owns that
    /// conditional's `end`.
    fn check_assignment(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let right = match node.kind_str() {
            "call" => last_argument(node),
            _ => node.field("right"),
        };
        let Some(mut right) = right.and_then(first_part_of_call_chain) else {
            return;
        };
        // `rhs.child_nodes.first while rhs.type?(:begin, :or, :and)`: the leading conditional of a
        // parenthesized group or of a logical operator is what carries the `end`.
        while let Some(inner) = leading_child(right) {
            right = inner;
        }
        if !CONDITIONAL_KINDS.contains(&right.kind_str()) {
            return;
        }
        self.check_asgn_alignment(node, right, offenses);
    }

    fn check_asgn_alignment(
        &mut self,
        outer: Node<'_>,
        inner: Node<'_>,
        offenses: &mut Vec<Offense>,
    ) {
        let Some(keyword) = inner.child(0) else {
            return;
        };
        let base = match self.style {
            Style::Keyword => keyword.byte_range(),
            Style::StartOfLine => start_line_range(self.context, inner.start_byte()),
            // `asgn_variable_align_with`: the assignment and the keyword together, unless the
            // keyword was pushed onto a line of its own.
            Style::Variable => match self.context.source.line_column(inner.start_byte()).0
                > self.context.source.line_column(outer.start_byte()).0
            {
                true => keyword.byte_range(),
                false => outer.start_byte()..keyword.end_byte(),
            },
        };
        self.check_end_kw_alignment(inner, base, offenses);
        self.ignored.insert(inner.id());
    }

    fn check_other_alignment(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let Some(keyword) = node.child(0) else {
            return;
        };
        let base = match self.style {
            Style::StartOfLine => start_line_range(self.context, node.start_byte()),
            Style::Keyword | Style::Variable => keyword.byte_range(),
        };
        self.check_end_kw_alignment(node, base, offenses);
    }

    fn check_end_kw_alignment(
        &mut self,
        node: Node<'_>,
        base: Range<usize>,
        offenses: &mut Vec<Offense>,
    ) {
        if self.ignored.contains(&node.id()) {
            return;
        }
        let Some(end) = end_keyword(node) else {
            return;
        };
        let column = self.alignment_column(node);
        if let Some(offense) = end_keyword_alignment(self.context, end.byte_range(), base, column)
            && self.reported.insert((end.start_byte(), end.end_byte()))
        {
            offenses.push(offense);
        }
    }

    /// `alignment_node`: the column the correction moves the `end` to.
    fn alignment_column(&self, node: Node<'_>) -> i64 {
        match self.style {
            Style::Keyword => character_column(self.context, node.start_byte()),
            Style::StartOfLine => character_column(
                self.context,
                start_line_range(self.context, node.start_byte()).start,
            ),
            Style::Variable => {
                let mut align_to = self.alignment_node_for_variable_style(node);
                // A call written around the aligned node on the same line takes its place.
                while let Some(parent) = send_parent(align_to) {
                    if self.context.source.line_column(parent.start_byte()).0
                        != self.context.source.line_column(align_to.start_byte()).0
                    {
                        break;
                    }
                    align_to = parent;
                }
                character_column(self.context, align_to.start_byte())
            }
        }
    }

    fn alignment_node_for_variable_style<'tree>(&self, node: Node<'tree>) -> Node<'tree> {
        if matches!(node.kind_str(), "case" | "case_match")
            && let Some(owner) = self.argument_owner(node)
            && self.context.source.line_column(owner.start_byte()).0
                == self.context.source.line_column(node.start_byte()).0
        {
            return owner;
        }
        let Some(assignment) = assignment_or_operator_method(node) else {
            return node;
        };
        match self.context.source.line_column(node.start_byte()).0
            > self.context.source.line_column(assignment.start_byte()).0
        {
            true => node,
            false => assignment,
        }
    }

    /// `node.parent&.assignment?`.
    fn assignment_parent<'tree>(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        node.parent()
            .filter(|parent| matches!(parent.kind_str(), "assignment" | "operator_assignment"))
    }

    /// `node.argument?`, answered with the call the node is an argument of.
    fn argument_owner<'tree>(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        node.parent()
            .filter(|parent| parent.kind_str() == "argument_list")
            .and_then(|list| list.parent())
            .filter(|call| call.kind_str() == "call")
    }
}

/// `rhs.child_nodes.first` for the node kinds upstream unwraps: a parenthesized group and the two
/// logical operators.
fn leading_child(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind_str() {
        "parenthesized_statements" => node.named_child(0),
        "binary" => {
            let operator = node.field("operator")?;
            matches!(operator.kind_str(), "&&" | "||" | "and" | "or")
                .then(|| node.field("left"))
                .flatten()
        }
        _ => None,
    }
}

/// `assignment_or_operator_method`: the nearest enclosing assignment, `<<` or operator call.
fn assignment_or_operator_method(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        let operator_method = candidate.kind_str() == "binary" && !is_logical(candidate);
        if matches!(candidate.kind_str(), "assignment" | "operator_assignment") || operator_method {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// Whether a `binary` is one of the four operators upstream spells as its own node type rather
/// than as a send.
///
/// `operator_method?` is asked of a `send`, and `a || b` is an `or` -- so the search walks past it
/// to the assignment above. Answering yes here instead aligns the `end` of `var = if ... end || x`
/// to the `if` it already sits under, which corrects nothing and reads as a correction loop.
fn is_logical(node: Node<'_>) -> bool {
    node.field("operator")
        .is_some_and(|operator| matches!(operator.kind_str(), "&&" | "||" | "and" | "or"))
}

/// The call a node sits directly inside, which is what `parent.send_type?` asks for.
fn send_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    match parent.kind_str() {
        // `parent.send_type?`, which the four logical operators are not.
        "binary" => (!is_logical(parent)).then_some(parent),
        "call" | "unary" | "element_reference" => Some(parent),
        "argument_list" => parent.parent().filter(|call| call.kind_str() == "call"),
        _ => None,
    }
}

fn last_argument<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let arguments = call.field("arguments")?;
    arguments
        .named_children(&mut arguments.walk())
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .last()
}
