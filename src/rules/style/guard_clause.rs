//! `Style/GuardClause`: a body wrapped in a conditional that only ever leaves the scope belongs
//! behind a guard instead.

use std::ops::Range;

use tree_sitter::Node;

use super::conditional::{
    Body, UpstreamParent, body_of, descendants, first_line, token, upstream_parent,
};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

const MSG: &str = "Use a guard clause (`%<example>s`) instead of wrapping the code inside a \
     conditional expression.";

/// The calls `match_guard_clause?` accepts as leaving the scope, on top of the three keywords.
const GUARD_CALLS: &[&str] = &["raise", "fail"];

/// `%i[return break next]`, which the same pattern accepts as nodes rather than calls.
const GUARD_KEYWORDS: &[&str] = &["return", "break", "next"];

/// The blocks `on_block` treats as definitions.
const DEFINING_METHODS: &[&str] = &["define_method", "define_singleton_method"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        max_line_length: max_line_length(context),
        min_body_length: context.setting("MinBodyLength").unwrap_or(1),
        allow_consecutive_conditionals: context
            .setting("AllowConsecutiveConditionals")
            .unwrap_or(false),
    };

    for node in context.nodes_of_any(&["method", "singleton_method", "call", "method_call"]) {
        // `on_block` reaches the same code as `on_def` for the two methods that define one, and
        // the body it hands over is the block's rather than the call's.
        let definition = match node.kind_str() {
            "call" | "method_call" => match cop.method_definition_block(node) {
                Some(block) => block,
                None => continue,
            },
            _ => node,
        };
        if let Some(body) = definition.field("body") {
            cop.check_ending_body(&body_of(body), offenses);
        }
    }
    for node in context.nodes_of_any(&["if", "unless"]) {
        cop.on_if(node, offenses);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    max_line_length: Option<usize>,
    min_body_length: usize,
    allow_consecutive_conditionals: bool,
}

/// Which branch the correction drops, which is the one the guard clause came from.
#[derive(Clone, Copy, PartialEq)]
enum Guard {
    If,
    Else,
    /// The guard is one side of an `and`/`or`, so no branch can be dropped and the offense is
    /// reported without a correction.
    None,
}

impl Cop<'_, '_> {
    fn source(&self, node: Node<'_>) -> &str {
        self.context.source.node_text(node)
    }

    fn method_definition_block<'t>(&self, node: Node<'t>) -> Option<Node<'t>> {
        let method = node.field("method")?;
        DEFINING_METHODS
            .contains(&self.source(method))
            .then(|| node.field("block"))
            .flatten()
    }

    /// `check_ending_body`: only the last expression of a definition can become a guard.
    fn check_ending_body(&self, body: &Body<'_>, offenses: &mut Vec<Offense>) {
        let Some(last) = body.last() else {
            return;
        };
        // `body.if_type?` reaches a lone conditional, `body.begin_type?` the last of several. A
        // single statement that is not a conditional is neither.
        if !body.is_begin() && body.single().is_none() {
            return;
        }
        if matches!(last.kind_str(), "if" | "unless") {
            self.check_ending_if(last, offenses);
        }
    }

    fn check_ending_if(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        if self.accepted_form(node, true) || !self.min_body_length(node) {
            return;
        }
        if self.allow_consecutive_conditionals && consecutive_conditionals(node) {
            return;
        }
        self.register_offense(
            node,
            "return",
            inverse_keyword(self.keyword(node)),
            Guard::None,
            offenses,
        );
        if let Some(consequence) = node.field("consequence") {
            self.check_ending_body(&body_of(consequence), offenses);
        }
    }

    /// `min_body_length?`: the conditional has to hold at least one line of its own.
    fn min_body_length(&self, node: Node<'_>) -> bool {
        let Some(end) = token(node, &["end"]) else {
            return false;
        };
        first_line(end).saturating_sub(first_line(node)) > self.min_body_length
    }

    fn on_if(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        if self.accepted_form(node, false) {
            return;
        }
        let branch = |field: &str| {
            node.field(field)
                .map(body_of)
                .and_then(|body| body.single())
        };
        let (clause, keyword, guard) = if let Some(clause) =
            branch("consequence").and_then(|branch| self.guard_clause(branch))
        {
            (clause, self.keyword(node).to_owned(), Guard::If)
        } else if let Some(clause) =
            branch("alternative").and_then(|branch| self.guard_clause(branch))
        {
            (
                clause,
                inverse_keyword(self.keyword(node)).to_owned(),
                Guard::Else,
            )
        } else {
            return;
        };
        // `and_or_guard_clause?`: the whole `a || raise` is the guard, and neither branch can be
        // dropped for it.
        let (source, guard) = match clause.operator_keyword {
            Some(whole) => (self.source(whole).to_owned(), Guard::None),
            None => (self.source(clause.node).to_owned(), guard),
        };
        self.register_offense(node, &source, &keyword, guard, offenses);
    }

    /// `guard_clause?`: a `raise`/`fail` call or a `return`/`break`/`next` written on one line,
    /// possibly as the right-hand side of an `and`/`or`.
    fn guard_clause<'t>(&self, branch: Node<'t>) -> Option<GuardClause<'t>> {
        let operator_keyword = matches!(branch.kind_str(), "binary")
            .then(|| branch.field("operator"))
            .flatten()
            .filter(|operator| matches!(self.source(*operator), "&&" | "||" | "and" | "or"))
            .and(Some(branch));
        let node = match operator_keyword {
            Some(_) => branch.field("right")?,
            None => branch,
        };
        if node.start_position().row != node.end_position().row {
            return None;
        }
        let leaves = match node.kind_str() {
            "call" | "method_call" => {
                node.field("receiver").is_none()
                    && node
                        .field("method")
                        .is_some_and(|method| GUARD_CALLS.contains(&self.source(method)))
            }
            // A bare `raise` carries no argument list, so the grammar leaves it a plain name.
            "identifier" => GUARD_CALLS.contains(&self.source(node)),
            kind => GUARD_KEYWORDS.contains(&kind),
        };
        leaves.then_some(GuardClause {
            node,
            operator_keyword,
        })
    }

    fn register_offense(
        &self,
        node: Node<'_>,
        scope_exiting_keyword: &str,
        conditional_keyword: &str,
        guard: Guard,
        offenses: &mut Vec<Offense>,
    ) {
        let Some(condition) = node.field("condition") else {
            return;
        };
        let condition_source = self.source(condition);
        let mut example =
            format!("{scope_exiting_keyword} {conditional_keyword} {condition_source}");
        let mut replacement = None;
        if self.too_long_for_single_line(node, &example) {
            if trivial(node) {
                return;
            }
            replacement = Some(format!(
                "{conditional_keyword} {condition_source}\n  {scope_exiting_keyword}\nend"
            ));
            example =
                format!("{conditional_keyword} {condition_source}; {scope_exiting_keyword}; end");
        }
        let Some(keyword) = token(node, &["if", "unless"]) else {
            return;
        };
        let offense = self
            .context
            .offense(MSG.replace("%<example>s", &example), keyword.byte_range());
        let has_else = node.field("alternative").is_some();
        offenses.push(match has_else && guard == Guard::None {
            true => offense,
            false => offense.corrected_by_all(self.autocorrect(
                node,
                condition,
                replacement.as_deref().unwrap_or(&example),
                guard,
            )),
        });
    }

    /// `too_long_for_single_line?`: the guard would not fit where the conditional stands.
    fn too_long_for_single_line(&self, node: Node<'_>, example: &str) -> bool {
        self.max_line_length
            .is_some_and(|max| node.start_position().column + example.chars().count() > max)
    }

    fn autocorrect(
        &self,
        node: Node<'_>,
        condition: Node<'_>,
        replacement: &str,
        guard: Guard,
    ) -> Vec<Edit> {
        let Some(keyword) = token(node, &["if", "unless"]) else {
            return Vec::new();
        };
        let mut edits = vec![Edit {
            start: keyword.start_byte(),
            end: condition.end_byte(),
            replacement: replacement.to_owned(),
            safe: true,
        }];
        // `node.then?`: only the written keyword is replaced, not the `;` that can stand for it.
        if let Some(then) = node
            .field("consequence")
            .and_then(|consequence| token(consequence, &["then"]))
        {
            edits.push(Edit {
                start: then.start_byte(),
                end: then.end_byte(),
                replacement: "\n".to_owned(),
                safe: true,
            });
        }
        let if_branch = self.branch_range(node, "consequence");
        let else_branch = self.branch_range(node, "alternative");
        let Some(end) = token(node, &["end"]) else {
            return edits;
        };
        let else_keyword = node
            .field("alternative")
            .and_then(|alternative| token(alternative, &["else"]));

        let heredoc = self
            .heredoc_argument(node, "consequence")
            .map(|heredoc| (heredoc, else_branch.clone()))
            .or_else(|| {
                self.heredoc_argument(node, "alternative")
                    .map(|heredoc| (heredoc, if_branch.clone()))
            });
        if let Some((heredoc, leave_branch)) = heredoc {
            edits.push(remove(self.whole_lines(end.byte_range())));
            let Some(else_keyword) = else_keyword else {
                return edits;
            };
            if let Some(leave) = leave_branch {
                edits.push(remove(self.whole_lines(leave.clone())));
                edits.push(Edit {
                    start: heredoc.end,
                    end: heredoc.end,
                    replacement: format!("\n{}", &self.context.source.text()[leave]),
                    safe: true,
                });
            }
            edits.push(remove(self.whole_lines(else_keyword.byte_range())));
            if let Some(dropped) = self.dropped_branch(guard, &if_branch, &else_branch) {
                edits.push(remove(self.whole_lines(dropped)));
            }
            return edits;
        }

        edits.push(remove(end.byte_range()));
        let Some(else_keyword) = else_keyword else {
            return edits;
        };
        edits.push(remove(else_keyword.byte_range()));
        if let Some(dropped) = self.dropped_branch(guard, &if_branch, &else_branch) {
            edits.push(remove(dropped));
        }
        edits
    }

    fn dropped_branch(
        &self,
        guard: Guard,
        if_branch: &Option<Range<usize>>,
        else_branch: &Option<Range<usize>>,
    ) -> Option<Range<usize>> {
        match guard {
            Guard::If => if_branch.clone(),
            Guard::Else => else_branch.clone(),
            Guard::None => None,
        }
    }

    /// The span of one branch, which is the whole statement list when it holds more than one.
    fn branch_range(&self, node: Node<'_>, field: &str) -> Option<Range<usize>> {
        let clause = node.field(field)?;
        let statements = super::nodes::children_in(clause, self.context);
        let first = statements.first()?;
        let last = statements.last()?;
        Some(first.start_byte()..last.end_byte())
    }

    /// `find_heredoc_argument`: the heredoc opened inside the branch, whose body sits below the
    /// conditional and so cannot simply be moved with it.
    fn heredoc_argument(&self, node: Node<'_>, field: &str) -> Option<Range<usize>> {
        let clause = node.field(field)?;
        let branch = match body_of(clause) {
            Body::Missing => return None,
            Body::One(only) => only,
            // `node = node.children.first while node.begin_type?`.
            Body::Begin(statements) => *statements.first()?,
        };
        let beginning = find_heredoc_argument(branch)?;
        heredoc_end(self.context, beginning)
    }

    /// `range_by_whole_lines(range, include_final_newline: true)`.
    fn whole_lines(&self, range: Range<usize>) -> Range<usize> {
        let source = self.context.source;
        let first = source.line_column(range.start).0;
        let last = source.line_column(range.end).0;
        source.line_start(first)..source.line_range(last).end.min(source.text().len())
    }

    fn keyword(&self, node: Node<'_>) -> &str {
        match node.kind_str() {
            "unless" => "unless",
            _ => "if",
        }
    }

    /// `accepted_form?`: forms that either cannot become a guard or are already one.
    fn accepted_form(&self, node: Node<'_>, ending: bool) -> bool {
        self.accepted_if(node, ending)
            || node
                .field("condition")
                .is_some_and(|condition| {
                    condition.start_position().row != condition.end_position().row
                })
            || matches!(
                upstream_parent(node),
                Some(UpstreamParent::Node(parent))
                    if matches!(parent.kind_str(), "assignment" | "operator_assignment")
            )
    }

    fn accepted_if(&self, node: Node<'_>, ending: bool) -> bool {
        let alternative = node.field("alternative");
        // `elsif_conditional?`: the chain would have to be unwound first.
        if alternative.is_some_and(|alternative| alternative.kind_str() == "elsif") {
            return true;
        }
        if self.assigned_lvar_used_in_if_branch(node) {
            return true;
        }
        match ending {
            true => alternative.is_some(),
            false => alternative.is_none(),
        }
    }

    /// `assigned_lvar_used_in_if_branch?`: the guard would move the assignment out of the reach of
    /// the code that reads it.
    fn assigned_lvar_used_in_if_branch(&self, node: Node<'_>) -> bool {
        let Some(condition) = node.field("condition") else {
            return false;
        };
        let assigned = self.assigned_locals(condition);
        if assigned.is_empty() {
            return false;
        }
        let Some(consequence) = node.field("consequence") else {
            return false;
        };
        self.branch_identifiers(consequence)
            .into_iter()
            .any(|name| assigned.contains(&name))
    }

    /// The names of the `lvasgn` nodes standing beneath the condition.
    ///
    /// The node upstream counts is the assignment itself for `x = 1`, but the name inside it for
    /// `x += 1` and for every element of `x, y = 1, 2` -- so a bare `if x = 1` writes no name a
    /// *descendant* of the condition holds, while `if x, y = foo` writes two.
    fn assigned_locals(&self, condition: Node<'_>) -> Vec<String> {
        let mut names = Vec::new();
        for inner in descendants(condition, self.context) {
            if !matches!(inner.kind_str(), "assignment" | "operator_assignment") {
                continue;
            }
            let Some(left) = inner.field("left") else {
                continue;
            };
            if left.kind_str() == "identifier" {
                let lvasgn = match inner.kind_str() {
                    "assignment" => inner,
                    _ => left,
                };
                if lvasgn.id() != condition.id() {
                    names.push(self.source(left).to_owned());
                }
                continue;
            }
            names.extend(
                destructured_names(left)
                    .into_iter()
                    .map(|name| self.source(name).to_owned()),
            );
        }
        names
    }

    /// The names the branch reads, which include those written inside a heredoc the branch opens:
    /// upstream hangs the heredoc's parts off the literal, while the grammar parks its body after
    /// the statement that opened it.
    fn branch_identifiers(&self, consequence: Node<'_>) -> Vec<String> {
        let body = body_of(consequence);
        let roots: Vec<Node<'_>> = match &body {
            Body::Missing => return Vec::new(),
            Body::One(node) => super::nodes::children_in(*node, self.context),
            Body::Begin(statements) => statements.clone(),
        };
        let mut names = Vec::new();
        let mut pending: Vec<Node<'_>> = roots;
        while let Some(root) = pending.pop() {
            for inner in descendants(root, self.context) {
                if inner.kind_str() == "heredoc_beginning"
                    && let Some(body) = self.heredoc_body(inner)
                {
                    pending.push(body);
                }
                if inner.kind_str() == "identifier" && reads_a_variable(inner) {
                    names.push(self.source(inner).to_owned());
                }
            }
        }
        names
    }

    fn heredoc_body<'t>(&self, beginning: Node<'t>) -> Option<Node<'t>>
    where
        Self: 't,
    {
        let index = self
            .context
            .nodes_of("heredoc_beginning")
            .position(|node| node.id() == beginning.id())?;
        self.context.nodes_of("heredoc_body").nth(index)
    }
}

/// `find_heredoc_argument`, which walks a call's arguments from the last one back and then its
/// receiver, so that the heredoc written closest to the end of the branch is the one found.
fn find_heredoc_argument<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut node = node;
    while node.kind_str() == "parenthesized_statements" {
        node = *super::nodes::children(node).first()?;
    }
    if node.kind_str() == "heredoc_beginning" {
        return Some(node);
    }
    if !matches!(node.kind_str(), "call" | "method_call") {
        return None;
    }
    if let Some(list) = node.field("arguments") {
        for argument in super::nodes::children(list).into_iter().rev() {
            if let Some(found) = find_heredoc_argument(argument) {
                return Some(found);
            }
        }
    }
    find_heredoc_argument(node.field("receiver")?)
}

/// Every name a destructuring left-hand side writes.
fn destructured_names<'t>(left: Node<'t>) -> Vec<Node<'t>> {
    match left.kind_str() {
        "identifier" => vec![left],
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
            super::nodes::children(left)
                .into_iter()
                .flat_map(destructured_names)
                .collect()
        }
        _ => Vec::new(),
    }
}

struct GuardClause<'t> {
    node: Node<'t>,
    /// The `and`/`or` the clause is the right-hand side of.
    operator_keyword: Option<Node<'t>>,
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

fn inverse_keyword(keyword: &str) -> &'static str {
    match keyword {
        "unless" => "if",
        _ => "unless",
    }
}

/// `trivial?`: one branch holding a single expression, which reads no worse wrapped than guarded.
fn trivial(node: Node<'_>) -> bool {
    let Some(branch) = node.field("consequence").map(body_of) else {
        return false;
    };
    if branch.last().is_none() {
        return false;
    }
    // `node.branches.one?`: an `else` adds a second branch.
    node.field("alternative").is_none()
        && branch.single().is_some_and(|only| {
            !matches!(
                only.kind_str(),
                "if" | "unless" | "if_modifier" | "unless_modifier" | "conditional"
            )
        })
}

/// `consecutive_conditionals?`: the statement written just before this one is a conditional too.
fn consecutive_conditionals(node: Node<'_>) -> bool {
    let Some(UpstreamParent::Begin(container)) = upstream_parent(node) else {
        return false;
    };
    let statements = super::conditional::self_statements(container);
    let Some(index) = statements
        .iter()
        .position(|statement| statement.id() == node.id())
    else {
        return false;
    };
    index > 0
        && matches!(
            statements[index - 1].kind_str(),
            "if" | "unless" | "if_modifier" | "unless_modifier" | "conditional"
        )
}

/// Whether the identifier stands for a value rather than for the name of a call or a parameter.
fn reads_a_variable(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    if matches!(parent.kind_str(), "block_parameters" | "method_parameters") {
        return false;
    }
    parent
        .field("method")
        .is_none_or(|method| method.id() != node.id())
}

/// The end of the heredoc terminator that `beginning` opened, which is where a moved branch is
/// written back in.
fn heredoc_end(context: &RuleContext<'_>, beginning: Node<'_>) -> Option<Range<usize>> {
    let index = context
        .nodes_of("heredoc_beginning")
        .position(|node| node.id() == beginning.id())?;
    let body = context.nodes_of("heredoc_body").nth(index)?;
    let _cursor = body.walk();
    let terminator = named_children_of(body, context)
        .into_iter()
        .find(|child| child.kind_str() == "heredoc_end")?;
    Some(terminator.byte_range())
}

/// `max_line_length`, which is `nil` when `Layout/LineLength` is switched off entirely.
fn max_line_length(context: &RuleContext<'_>) -> Option<usize> {
    context
        .setting_of::<bool>("Layout/LineLength", "Enabled")
        .unwrap_or(true)
        .then(|| {
            context
                .setting_of("Layout/LineLength", "Max")
                .unwrap_or(120)
        })
}
