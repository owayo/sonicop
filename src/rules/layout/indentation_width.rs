//! `Layout/IndentationWidth`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    alignment_corrections, body_statements, character_column, holds_block_comment,
    line_indentation, string_interiors,
};
use crate::diagnostic::Offense;
use crate::rules::{RuleContext, push_named_children};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let width: i64 = context
        .setting::<i64>("Width")
        .filter(|width| *width > 0)
        .unwrap_or(2);
    let mut checker = Checker {
        context,
        width,
        outdented_modifiers: context
            .setting_of::<String>("Layout/AccessModifierIndentation", "EnforcedStyle")
            .as_deref()
            == Some("outdent"),
        indented_internal_methods: context
            .setting_of::<String>("Layout/IndentationConsistency", "EnforcedStyle")
            .as_deref()
            == Some("indented_internal_methods"),
        align_end_with_def: context
            .setting_of::<String>("Layout/DefEndAlignment", "EnforcedStyleAlignWith")
            .as_deref()
            == Some("def"),
        reported: HashSet::new(),
        ignored: HashSet::new(),
        corrected_ranges: Vec::new(),
    };
    // A pre-order walk reproduces the order upstream's commissioner calls the handlers in, which
    // is what decides who claims a body first and which offense a duplicate range belongs to.
    let mut stack = vec![context.root_node()];
    while let Some(node) = stack.pop() {
        checker.visit(node, offenses);
        push_named_children(node, &mut stack);
    }
}

struct Checker<'a, 'b> {
    context: &'a RuleContext<'b>,
    width: i64,
    outdented_modifiers: bool,
    indented_internal_methods: bool,
    align_end_with_def: bool,
    /// Ranges already reported, which upstream keeps per cop and per file.
    reported: HashSet<(usize, usize)>,
    /// Definitions a `private def foo` already claimed, which `on_def` then leaves alone.
    ignored: HashSet<usize>,
    /// `other_offense_in_same_range?`: the spans this cop has already handed a corrector.
    ///
    /// A correction here shifts every line the node spans, so two of them nested inside one
    /// another would shift the inner lines twice and corrupt the file. Upstream drops the
    /// corrector of the inner offense; the outer shift then makes it report again on the next
    /// pass, where it is no longer nested. The list is only kept while correcting, since
    /// upstream's `autocorrect? && other_offense_in_same_range?` never reaches the call
    /// otherwise -- an inspection that corrects nothing leaves every offense correctable.
    corrected_ranges: Vec<(usize, usize)>,
}

impl Checker<'_, '_> {
    fn visit(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        match node.kind_str() {
            "method" | "singleton_method" => {
                if !self.ignored.contains(&node.id()) {
                    if let Some(keyword) = child_of_kind(node, "def") {
                        self.check_body(keyword.byte_range(), node, offenses);
                    }
                }
            }
            "class" | "module" | "singleton_class" => self.on_class(node, offenses),
            "block" | "do_block" => self.on_block(node, offenses),
            "if" | "unless" | "elsif" => self.on_if(node, offenses),
            "while" | "until" => self.on_while(node, offenses),
            "case" => self.on_case(node, "when", offenses),
            "case_match" => self.on_case(node, "in_clause", offenses),
            // `on_for` is an alias of `on_resbody`: the keyword and the body.
            "for" => {
                if let Some(keyword) = child_of_kind(node, "for") {
                    let body = node.field("body");
                    self.check_container(keyword.byte_range(), body, offenses);
                }
            }
            "rescue" => {
                if let Some(keyword) = child_of_kind(node, "rescue") {
                    let body = child_of_kind(node, "then");
                    self.check_container(keyword.byte_range(), body, offenses);
                }
            }
            "ensure" => {
                if let Some(keyword) = child_of_kind(node, "ensure") {
                    self.check_container(keyword.byte_range(), Some(node), offenses);
                }
            }
            "begin" => self.on_kwbegin(node, offenses),
            "parenthesized_statements" => self.on_parenthesized(node, offenses),
            "call" => self.on_send(node, offenses),
            _ => {}
        }
        // `on_rescue` checks the `else` of a body that also has `rescue` clauses.
        if is_statement_container(node) && has_kind(node, "rescue") {
            if let Some(branch) = child_of_kind(node, "else") {
                if let Some(keyword) = child_of_kind(branch, "else") {
                    self.check_container(keyword.byte_range(), Some(branch), offenses);
                }
            }
        }
    }

    fn on_class(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let keyword = match node.kind_str() {
            "module" => child_of_kind(node, "module"),
            _ => child_of_kind(node, "class"),
        };
        let (Some(keyword), Some(container)) = (keyword, body_container(node)) else {
            return;
        };
        let Some(body) = self.parser_body(container) else {
            return;
        };
        if self.context.source.line_column(keyword.start_byte()).0
            == self.context.source.line_column(body.start).0
        {
            return;
        }
        self.check_members(keyword.byte_range(), &body, offenses);
    }

    /// `check_members`: the body as a whole, then every member of it against the same base.
    fn check_members(&mut self, base: Range<usize>, body: &Body, offenses: &mut Vec<Offense>) {
        // `select_check_member`: a body that opens with an access modifier is measured by that
        // modifier, unless the neighbouring cop outdents modifiers.
        let selected = if body.is_begin
            && body
                .statements
                .first()
                .is_some_and(|first| self.is_access_modifier(*first))
        {
            if self.outdented_modifiers {
                None
            } else {
                body.statements.first().map(|first| Body::plain(*first))
            }
        } else {
            Some(body.clone())
        };
        if let Some(selected) = selected {
            self.check_indentation(base.clone(), Some(&selected), "", offenses);
        }
        if !body.is_begin {
            return;
        }
        if self.indented_internal_methods {
            let mut previous: Option<Range<usize>> = None;
            for member in &body.statements {
                if self.is_special_modifier(*member) {
                    previous = Some(member.byte_range());
                } else if let Some(modifier) = previous.take() {
                    self.check_indentation(
                        modifier,
                        Some(&Body::plain(*member)),
                        " indented_internal_methods",
                        offenses,
                    );
                }
            }
            return;
        }
        for member in body.statements.clone() {
            if self.is_access_modifier(member) {
                continue;
            }
            self.check_indentation(base.clone(), Some(&Body::plain(member)), "", offenses);
        }
    }

    fn on_block(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let Some(end) = last_child(node).filter(|end| matches!(end.kind_str(), "end" | "}")) else {
            return;
        };
        if !self.begins_its_line(end.start_byte()) {
            return;
        }
        let container = body_container(node);
        self.check_container(end.byte_range(), container, offenses);
        if !self.indented_internal_methods {
            return;
        }
        let Some(container) = container else { return };
        let Some(body) = self.parser_body(container) else {
            return;
        };
        if body.is_begin
            && body
                .statements
                .iter()
                .any(|member| self.is_access_modifier(*member))
        {
            self.check_members(end.byte_range(), &body, offenses);
        }
    }

    fn on_if(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let base = node.byte_range();
        let branch = child_of_kind(node, "then");
        self.check_container(base, branch, offenses);
        let Some(alternative) = child_of_kind(node, "else") else {
            return;
        };
        let Some(keyword) = child_of_kind(alternative, "else") else {
            return;
        };
        self.check_container(keyword.byte_range(), Some(alternative), offenses);
    }

    fn on_while(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let (Some(keyword), Some(condition)) =
            (node.child(0), node.field("condition"))
        else {
            return;
        };
        // `single_line_condition?`: the condition opens on the keyword's own line.
        if self.context.source.line_column(keyword.start_byte()).0
            != self.context.source.line_column(condition.start_byte()).0
        {
            return;
        }
        self.check_container(
            node.byte_range(),
            node.field("body"),
            offenses,
        );
    }

    fn on_case(&mut self, node: Node<'_>, branch_kind: &str, offenses: &mut Vec<Offense>) {
        let mut cursor = node.walk();
        let branches: Vec<Node<'_>> = node
            .named_children(&mut cursor)
            .filter(|child| child.kind_str() == branch_kind)
            .collect();
        let mut last_keyword = None;
        for branch in &branches {
            let Some(keyword) = branch.child(0) else {
                continue;
            };
            last_keyword = Some(keyword.byte_range());
            self.check_container(
                keyword.byte_range(),
                child_of_kind(*branch, "then"),
                offenses,
            );
        }
        let Some(last_keyword) = last_keyword else {
            return;
        };
        let alternative = child_of_kind(node, "else");
        self.check_container(last_keyword, alternative, offenses);
    }

    fn on_kwbegin(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let Some(end) = last_child(node).filter(|end| end.kind_str() == "end") else {
            return;
        };
        if !self.begins_its_line(end.start_byte()) {
            return;
        }
        // `node.children.first` is the first statement, or the clause node the body was folded
        // into.
        let Some(first) = self.first_child_of_kwbegin(node) else {
            return;
        };
        self.check_indentation(end.byte_range(), Some(&first), "", offenses);
    }

    fn on_parenthesized(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let (Some(open), Some(close)) = (node.child(0), last_child(node)) else {
            return;
        };
        if open.kind_str() != "(" || close.kind_str() != ")" || !self.begins_its_line(close.start_byte()) {
            return;
        }
        // `opening_line_start`: the first non-blank column of the line the parenthesis is on.
        let line = self.context.source.line_column(open.start_byte()).0;
        let start = self.context.source.line_start(line)
            + usize::try_from(line_indentation(self.context, open.start_byte())).unwrap_or(0);
        let Some(first) = body_statements(node).first().copied() else {
            return;
        };
        self.check_indentation(start..start, Some(&Body::plain(first)), "", offenses);
    }

    /// `private def foo` reaches the cop as a call whose only argument is the definition.
    fn on_send(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        if node.field("receiver").is_some() {
            return;
        }
        let mut cursor = node.walk();
        let Some(list) = node
            .children(&mut cursor)
            .find(|child| child.kind_str() == "argument_list")
        else {
            return;
        };
        let arguments = body_statements(list);
        if arguments.len() != 1 {
            return;
        }
        let definition = arguments[0];
        if !matches!(definition.kind_str(), "method" | "singleton_method") {
            return;
        }
        let base = if self.align_end_with_def {
            definition.byte_range()
        } else {
            leftmost_modifier(node).byte_range()
        };
        let container = body_container(definition);
        self.check_container(base, container, offenses);
        self.ignored.insert(definition.id());
    }

    fn check_container(
        &mut self,
        base: Range<usize>,
        container: Option<Node<'_>>,
        offenses: &mut Vec<Offense>,
    ) {
        let body = container.and_then(|container| self.parser_body(container));
        self.check_indentation(base, body.as_ref(), "", offenses);
    }

    fn check_body(&mut self, base: Range<usize>, owner: Node<'_>, offenses: &mut Vec<Offense>) {
        self.check_container(base, body_container(owner), offenses);
    }

    fn check_indentation(
        &mut self,
        base: Range<usize>,
        body: Option<&Body>,
        style: &str,
        offenses: &mut Vec<Offense>,
    ) {
        let Some(body) = body else { return };
        if self.skip_check(&base, body) || !body.worth_checking() {
            return;
        }
        let indentation =
            character_column(self.context, body.start) - character_column(self.context, base.start);
        let delta = self.width - indentation;
        if delta == 0 {
            return;
        }
        self.offense(body, indentation, delta, style, offenses);
    }

    fn skip_check(&self, base: &Range<usize>, body: &Body) -> bool {
        if self.context.source.line_column(body.start).0
            == self.context.source.line_column(base.start).0
        {
            return true;
        }
        // A body that opens with an access modifier belongs to the modifier's own cop.
        if body.is_begin
            && body
                .statements
                .first()
                .is_some_and(|first| self.is_bare_access_modifier(*first))
        {
            return true;
        }
        // Only a body that starts its line is measured; `else do_something` is not.
        !self.begins_its_line(body.start)
    }

    fn offense(
        &mut self,
        body: &Body,
        indentation: i64,
        delta: i64,
        style: &str,
        offenses: &mut Vec<Offense>,
    ) {
        // Only the first statement of a body is moved, not the whole run.
        let target = if body.is_begin && !body.parenthesized {
            body.statements
                .first()
                .map_or(body.range.clone(), tree_sitter::Node::byte_range)
        } else {
            body.range.clone()
        };
        let start = target.start;
        // `begin_pos - indentation` counts characters upstream, and the text a negative
        // indentation runs into is the body itself, which can hold multibyte characters.
        let text = self.context.source.text();
        let range = if indentation >= 0 {
            let width = usize::try_from(indentation).unwrap_or(0);
            step_back(text, start, width)..start
        } else {
            let width = usize::try_from(-indentation).unwrap_or(0);
            start..step_forward(text, start, width)
        };
        if !self.reported.insert((range.start, range.end)) {
            return;
        }
        let message = format!(
            "Use {} (not {indentation}) spaces for{style} indentation.",
            self.width
        );
        let mut offense = self.context.offense(message, range);
        if !self.other_offense_in_same_range(&target) && !holds_block_comment(self.context, &target)
        {
            let taboo = string_interiors(self.context, &target);
            offense = offense.corrected_by_all(alignment_corrections(
                self.context,
                target,
                delta,
                &taboo,
            ));
        }
        offenses.push(offense);
    }

    /// Whether an offense already corrected covers `target`, recording it when none does. See
    /// [`Checker::corrected_ranges`].
    fn other_offense_in_same_range(&mut self, target: &Range<usize>) -> bool {
        if !self.context.correcting() {
            return false;
        }
        if self
            .corrected_ranges
            .iter()
            .any(|(start, end)| target.start >= *start && target.end <= *end)
        {
            return true;
        }
        self.corrected_ranges.push((target.start, target.end));
        false
    }

    fn begins_its_line(&self, offset: usize) -> bool {
        super::support::begins_its_line(self.context, offset)
    }

    /// The body upstream's parser builds for a container: a single statement, a `begin`, or the
    /// `rescue` / `ensure` node the statements were folded into.
    fn parser_body<'tree>(&self, container: Node<'tree>) -> Option<Body<'tree>> {
        if container.kind_str() == "parenthesized_statements" {
            let statements = body_statements(container);
            if statements.is_empty() {
                return None;
            }
            return Some(Body {
                start: container.start_byte(),
                range: container.byte_range(),
                is_begin: true,
                parenthesized: true,
                statements,
                clause: Clause::None,
            });
        }
        let statements = body_statements(container);
        let rescue = has_kind(container, "rescue");
        let ensure = has_kind(container, "ensure");
        if ensure || rescue {
            let keyword = if rescue {
                child_of_kind(container, "rescue")
            } else {
                child_of_kind(container, "ensure")
            };
            let start = statements.first().map_or_else(
                || keyword.map(|node| node.start_byte()),
                |first| Some(first.start_byte()),
            )?;
            // Upstream's body here is the `rescue` node, which stops at the last statement of
            // the last clause. The grammar's container reaches past it to the `end` keyword, and
            // shifting that line moves the very thing the indentation is measured against -- the
            // offence then never resolves and the body marches to column one.
            let last = super::support::body_statements(container)
                .last()
                .map_or(container.end_byte(), |node| node.end_byte());
            let last = child_of_kind(container, "rescue")
                .or_else(|| child_of_kind(container, "ensure"))
                .map_or(last, |clause| clause.end_byte().max(last));
            return Some(Body {
                start,
                range: start..last,
                is_begin: false,
                parenthesized: false,
                clause: if ensure {
                    Clause::Ensure
                } else {
                    Clause::Rescue
                },
                statements,
            });
        }
        let first = *statements.first()?;
        if statements.len() == 1 {
            if first.kind_str() == "parenthesized_statements" {
                return self.parser_body(first);
            }
            return Some(Body::plain(first));
        }
        Some(Body {
            start: first.start_byte(),
            range: first.start_byte()..statements[statements.len() - 1].end_byte(),
            is_begin: true,
            parenthesized: false,
            statements,
            clause: Clause::None,
        })
    }

    /// `node.children.first` of a `kwbegin`, which is the clause node once the body has one.
    fn first_child_of_kwbegin<'tree>(&self, node: Node<'tree>) -> Option<Body<'tree>> {
        let statements = body_statements(node);
        if has_kind(node, "rescue") || has_kind(node, "ensure") {
            return self.parser_body(node);
        }
        statements.first().map(|first| Body::plain(*first))
    }

    fn is_bare_access_modifier(&self, node: Node<'_>) -> bool {
        node.kind_str() == "identifier"
            && is_modifier_name(&self.context.source.text()[node.byte_range()])
    }

    fn is_special_modifier(&self, node: Node<'_>) -> bool {
        node.kind_str() == "identifier"
            && matches!(
                &self.context.source.text()[node.byte_range()],
                "private" | "protected"
            )
    }

    /// `access_modifier?`, which covers `private` as well as `private :foo`.
    fn is_access_modifier(&self, node: Node<'_>) -> bool {
        if self.is_bare_access_modifier(node) {
            return true;
        }
        node.kind_str() == "call"
            && node.field("receiver").is_none()
            && node.field("method").is_some_and(|method| {
                is_modifier_name(&self.context.source.text()[method.byte_range()])
            })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Clause {
    None,
    Rescue,
    Ensure,
}

/// One body as upstream's parser presents it.
#[derive(Clone)]
struct Body<'tree> {
    start: usize,
    range: Range<usize>,
    is_begin: bool,
    parenthesized: bool,
    statements: Vec<Node<'tree>>,
    clause: Clause,
}

impl<'tree> Body<'tree> {
    fn plain(node: Node<'tree>) -> Self {
        Self {
            start: node.start_byte(),
            range: node.byte_range(),
            is_begin: false,
            parenthesized: false,
            statements: vec![node],
            clause: Clause::None,
        }
    }

    /// `indentation_to_check?`: a `rescue` or `ensure` with nothing before the keyword has no body
    /// to measure.
    fn worth_checking(&self) -> bool {
        match self.clause {
            Clause::None => true,
            Clause::Rescue | Clause::Ensure => !self.statements.is_empty(),
        }
    }
}

fn step_back(text: &str, offset: usize, characters: usize) -> usize {
    let mut cursor = offset;
    for _ in 0..characters {
        if cursor == 0 {
            break;
        }
        cursor -= 1;
        while cursor > 0 && !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
    }
    cursor
}

fn step_forward(text: &str, offset: usize, characters: usize) -> usize {
    let mut cursor = offset;
    for _ in 0..characters {
        match text[cursor..].chars().next() {
            Some(character) => cursor += character.len_utf8(),
            None => break,
        }
    }
    cursor
}

fn is_modifier_name(name: &str) -> bool {
    matches!(name, "public" | "protected" | "private" | "module_function")
}

fn is_statement_container(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "body_statement" | "block_body" | "begin" | "do" | "then" | "else" | "program"
    )
}

fn body_container<'tree>(owner: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = owner.walk();
    owner
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind_str(), "body_statement" | "block_body" | "do"))
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind_str() == kind)
}

fn has_kind(node: Node<'_>, kind: &str) -> bool {
    child_of_kind(node, kind).is_some()
}

fn last_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let count = u32::try_from(node.child_count()).ok()?;
    node.child(count.checked_sub(1)?)
}

/// `leftmost_modifier_of`: `private public def foo` measures from the outermost modifier.
fn leftmost_modifier<'tree>(node: Node<'tree>) -> Node<'tree> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind_str() != "call" && parent.kind_str() != "argument_list" {
            break;
        }
        if parent.kind_str() == "argument_list" {
            match parent.parent() {
                Some(call) if call.kind_str() == "call" => current = call,
                _ => break,
            }
        } else {
            current = parent;
        }
    }
    current
}
