//! `Layout/ElseAlignment`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    alignment_corrections, begins_its_line, character_column, first_part_of_call_chain,
    start_line_range,
};
use crate::diagnostic::Offense;
use crate::rules::{RuleContext, push_named_children};
use crate::rules::node_ext::NodeExt;

/// The node kinds upstream's parser calls an `if`.
const IF_KINDS: [&str; 6] = [
    "if",
    "unless",
    "elsif",
    "conditional",
    "if_modifier",
    "unless_modifier",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut checker = Checker {
        context,
        // `variable_alignment?` reads the neighbouring cop, whose default keeps the base at the
        // right-hand side of the assignment rather than at the variable.
        align_with_variable: context
            .setting_of::<String>("Layout/EndAlignment", "EnforcedStyleAlignWith")
            .as_deref()
            .is_some_and(|style| style != "keyword"),
        ignored: HashSet::new(),
        reported: HashSet::new(),
    };
    // A pre-order walk puts the handlers in the order upstream's commissioner calls them, which is
    // what decides whether an `elsif` is reached with the base of its `if` or on its own.
    let mut stack = vec![context.root_node()];
    while let Some(node) = stack.pop() {
        checker.visit(node, offenses);
        push_named_children(node, &mut stack);
    }
}

struct Checker<'a, 'b> {
    context: &'a RuleContext<'b>,
    align_with_variable: bool,
    /// `ignore_node`: the `elsif` branches already checked against their `if`.
    ignored: HashSet<usize>,
    reported: HashSet<(usize, usize)>,
}

impl Checker<'_, '_> {
    fn visit(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        match node.kind_str() {
            "if" | "unless" | "elsif" => self.on_if(node, None, offenses),
            "case" => self.on_case(node, "when", offenses),
            "case_match" => self.on_case(node, "in_clause", offenses),
            "assignment" | "operator_assignment" | "call" => self.check_assignment(node, offenses),
            _ => {}
        }
        // `on_rescue` sees the node a body with `rescue` clauses is folded into, which the grammar
        // spells as the clauses sitting beside the statements.
        if child_of_kind(node, "rescue").is_some() {
            self.on_rescue(node, offenses);
        }
    }

    fn on_if(&mut self, node: Node<'_>, base: Option<Range<usize>>, offenses: &mut Vec<Offense>) {
        if self.ignored.contains(&node.id()) {
            return;
        }
        let Some(branch) = node
            .named_children(&mut node.walk())
            .find(|child| matches!(child.kind_str(), "else" | "elsif"))
        else {
            return;
        };
        let Some(keyword) = branch.child(0) else {
            return;
        };
        if !begins_its_line(self.context, keyword.start_byte()) {
            return;
        }
        let base_range = match &base {
            Some(base) => base.clone(),
            None => self.base_range_of_if(node),
        };
        self.check_alignment(base_range, keyword.byte_range(), offenses);
        // `elsif_conditional?`: the branch is another `if` upstream, checked against the same base.
        if branch.kind_str() == "elsif" {
            self.on_if(branch, base, offenses);
            self.ignored.insert(branch.id());
        }
    }

    /// `base_range_of_if`: the keyword of the nearest enclosing `if` or `unless`, which is what an
    /// `elsif` several branches down still aligns with.
    fn base_range_of_if(&self, node: Node<'_>) -> Range<usize> {
        let mut current = Some(node);
        while let Some(candidate) = current {
            if matches!(candidate.kind_str(), "if" | "unless") {
                if let Some(keyword) = candidate.child(0) {
                    return keyword.byte_range();
                }
            }
            current = candidate
                .parent_of(self.context)
                .filter(|parent| IF_KINDS.contains(&parent.kind_str()));
        }
        node.byte_range()
    }

    fn on_case(&mut self, node: Node<'_>, branch: &str, offenses: &mut Vec<Offense>) {
        let Some(keyword) = child_of_kind(node, "else").and_then(|node| node.child(0)) else {
            return;
        };
        let Some(last) = node
            .named_children(&mut node.walk())
            .filter(|child| child.kind_str() == branch)
            .last()
            .and_then(|child| child.child(0))
        else {
            return;
        };
        self.check_alignment(last.byte_range(), keyword.byte_range(), offenses);
    }

    fn on_rescue(&mut self, container: Node<'_>, offenses: &mut Vec<Offense>) {
        let Some(keyword) = child_of_kind(container, "else").and_then(|node| node.child(0)) else {
            return;
        };
        let Some(base) = self.base_range_of_rescue(container) else {
            return;
        };
        self.check_alignment(base, keyword.byte_range(), offenses);
    }

    /// `base_range_of_rescue`: what the body belongs to. A body written straight into a class or a
    /// module reaches a branch upstream where the range it reads is `nil`, which raises rather than
    /// reports, so nothing is checked there.
    fn base_range_of_rescue(&self, container: Node<'_>) -> Option<Range<usize>> {
        if container.kind_str() == "begin" {
            return container.child(0).map(|keyword| keyword.byte_range());
        }
        let owner = container.parent_of(self.context)?;
        match owner.kind_str() {
            "method" | "singleton_method" => Some(self.base_for_method_definition(owner)),
            "block" | "do_block" => {
                let block = owner.parent_of(self.context)?;
                Some(start_line_range(self.context, block.start_byte()))
            }
            _ => None,
        }
    }

    /// `base_for_method_definition`: a definition passed to `private` aligns with the modifier.
    fn base_for_method_definition(&self, definition: Node<'_>) -> Range<usize> {
        let keyword = definition
            .child(0)
            .map_or_else(|| definition.byte_range(), |node| node.byte_range());
        let Some(call) = definition
            .parent_of(self.context)
            .filter(|parent| parent.kind_str() == "argument_list")
            .and_then(|list| list.parent_of(self.context))
            .filter(|call| call.kind_str() == "call")
        else {
            return keyword;
        };
        call.field("method")
            .map_or(keyword, |method| method.byte_range())
    }

    /// `check_assignment`: the right-hand side of an assignment is checked against the assignment
    /// rather than against its own keyword.
    fn check_assignment(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let right = match node.kind_str() {
            "call" => last_argument(node),
            _ => node.field("right"),
        };
        let Some(right) = right.and_then(first_part_of_call_chain) else {
            return;
        };
        if !IF_KINDS.contains(&right.kind_str()) {
            return;
        }
        let base = match self.align_with_variable
            && self.context.source.line_column(right.start_byte()).0
                <= self.context.source.line_column(node.start_byte()).0
        {
            true => node.byte_range(),
            false => right.byte_range(),
        };
        self.on_if(right, Some(base), offenses);
        self.ignored.insert(right.id());
    }

    fn check_alignment(
        &mut self,
        base: Range<usize>,
        branch: Range<usize>,
        offenses: &mut Vec<Offense>,
    ) {
        if !begins_its_line(self.context, branch.start) {
            return;
        }
        let delta = character_column(self.context, base.start)
            - character_column(self.context, branch.start);
        if delta == 0 {
            return;
        }
        if !self.reported.insert((branch.start, branch.end)) {
            return;
        }
        let text = self.context.source.text();
        let message = format!(
            "Align `{}` with `{}`.",
            &text[branch.clone()],
            first_word(&text[base])
        );
        offenses.push(
            self.context
                .offense(message, branch.clone())
                .corrected_by_all(alignment_corrections(self.context, branch, delta, &[])),
        );
    }
}

/// `base_range.source[/^\S*/]`: the base is named by the first word of what it covers.
fn first_word(text: &str) -> &str {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    &text[..end]
}

fn last_argument<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let arguments = call.field("arguments")?;
    arguments
        .named_children(&mut arguments.walk())
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .last()
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    node.named_children(&mut node.walk())
        .find(|child| child.kind_str() == kind)
}
