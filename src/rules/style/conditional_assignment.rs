//! `Style/ConditionalAssignment`: assign the conditional's value rather than assigning in each
//! branch.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;
use crate::rules::support;

const MSG: &str = "Use the return of the conditional for variable assignment and comparison.";
const ASSIGN_TO_CONDITION_MSG: &str = "Assign variables inside of conditionals.";

/// The operator methods `assignment_type?` accepts alongside every name ending in `=`, which
/// upstream's parser writes as a `send` like any other call.
const ASSIGNMENT_OPERATORS: &[&str] = &["[]=", "<<", "=~", "!~", "<=>", "<", ">"];

/// `Node::COMPARISON_OPERATORS`, the names ending in `=` that assign nothing.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

/// `assignment_type?` for a call: an operator method the cop names, or any setter.
fn is_assignment_method(name: &str) -> bool {
    ASSIGNMENT_OPERATORS.contains(&name) || name.ends_with('=')
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        assign_to_condition: context
            .setting::<String>("EnforcedStyle")
            .is_none_or(|style| style == "assign_to_condition"),
        single_line_only: context.setting("SingleLineConditionsOnly").unwrap_or(true),
        include_ternary: context.setting("IncludeTernaryExpressions").unwrap_or(true),
        line_length: context
            .setting_of::<bool>("Layout/LineLength", "Enabled")
            .unwrap_or(true)
            .then(|| {
                context
                    .setting_of::<usize>("Layout/LineLength", "Max")
                    .unwrap_or(120)
            }),
        keyword_aligned: context
            .setting_of::<String>("Layout/EndAlignment", "EnforcedStyleAlignWith")
            .is_none_or(|style| style == "keyword"),
    };
    match cop.assign_to_condition {
        true => cop.check_conditionals(offenses),
        false => cop.check_assignments(offenses),
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    assign_to_condition: bool,
    single_line_only: bool,
    include_ternary: bool,
    /// `Layout/LineLength`'s `Max`, absent when that cop is off and the length cannot be exceeded.
    line_length: Option<usize>,
    /// Whether `Layout/EndAlignment` wants the `end` under the keyword, which is what decides
    /// whether the `end` is pushed right by the assignment moved above it.
    keyword_aligned: bool,
}

/// A conditional whose branches all assign the same thing.
struct Conditional<'t> {
    node: Node<'t>,
    /// Each branch as written, in the order upstream hands them over.
    branches: Vec<Node<'t>>,
    ternary: bool,
    case_like: bool,
}

impl Cop<'_, '_> {
    /// `on_if` / `on_case` / `on_case_match`: the `assign_to_condition` half of the cop.
    fn check_conditionals(&self, offenses: &mut Vec<Offense>) {
        for node in
            self.context
                .nodes_of_any(&["if", "unless", "conditional", "case", "case_match"])
        {
            let Some(conditional) = self.conditional(node, false) else {
                continue;
            };
            // `allowed_ternary?`.
            if conditional.ternary && !self.include_ternary {
                continue;
            }
            let Some(statements) = self.allowed_statements(&conditional.branches) else {
                continue;
            };
            if self.allowed_single_line(&conditional.branches)
                || self.correction_exceeds_line_limit(node, &statements)
            {
                continue;
            }
            let edits = self.move_assignment_outside_condition(&conditional, &statements);
            if !support::correction_parses(self.context, &edits) {
                continue;
            }
            offenses.push(
                self.context
                    .offense(MSG, node.byte_range())
                    .corrected_by_all(edits),
            );
        }
    }

    /// The branches of a conditional that has an `else`, which is the only shape this cop reads.
    fn conditional<'t>(
        &self,
        node: Node<'t>,
        allow_elsif_without_else: bool,
    ) -> Option<Conditional<'t>> {
        if matches!(node.kind_str(), "case" | "case_match") {
            let children = super::nodes::children(node);
            let otherwise = children.iter().find(|child| child.kind_str() == "else")?;
            // `branches.all?`: a `when` with an empty body has nothing to compare, and drops the
            // conditional out of the check rather than out of the list.
            let mut branches: Vec<Node<'t>> = Vec::new();
            for clause in children
                .iter()
                .filter(|child| matches!(child.kind_str(), "when" | "in_clause"))
            {
                branches.push(branch(clause.field("body"))?);
            }
            branches.push(branch(Some(*otherwise))?);
            return Some(Conditional {
                node,
                branches,
                ternary: false,
                case_like: true,
            });
        }
        // A ternary is an `if` upstream, with its two operands for branches.
        if node.kind_str() == "conditional" {
            return Some(Conditional {
                node,
                branches: vec![node.field("consequence")?, node.field("alternative")?],
                ternary: true,
                case_like: false,
            });
        }
        let mut branches = vec![branch(node.field("consequence"))?];
        // `expand_elses`: an `elsif` is a nested conditional whose branches belong to this one.
        let mut alternative = node.field("alternative")?;
        while alternative.kind_str() == "elsif" {
            branches.push(branch(alternative.field("consequence"))?);
            let Some(next) = alternative.field("alternative") else {
                // Parser exposes the final `elsif` itself as the top `if`'s else branch even when
                // there is no explicit `else`. That is enough for `assignment.else_branch` and
                // the corrector moves the assignment into every branch that actually exists.
                return allow_elsif_without_else.then_some(Conditional {
                    node,
                    branches,
                    ternary: false,
                    case_like: false,
                });
            };
            alternative = next;
        }
        branches.push(branch(Some(alternative))?);
        Some(Conditional {
            node,
            branches,
            ternary: false,
            case_like: false,
        })
    }

    /// `allowed_statements?`: every branch ends in the same assignment, written the same way.
    fn allowed_statements<'t>(&self, branches: &[Node<'t>]) -> Option<Vec<Assignment<'t>>> {
        let statements: Vec<Assignment<'t>> = branches
            .iter()
            .filter_map(|branch| self.classify(tail(*branch)))
            .collect();
        if statements.len() != branches.len() {
            return None;
        }
        let first = statements.first()?;
        // A multiple assignment has more than one name to hand the value to.
        if statements
            .iter()
            .any(|statement| statement.kind == Kind::Masgn)
        {
            return None;
        }
        (statements
            .iter()
            .all(|statement| statement.lhs == first.lhs && statement.kind == first.kind))
        .then_some(statements)
    }

    /// `allowed_single_line?`: a branch holding more than one statement leaves nothing to hoist.
    fn allowed_single_line(&self, branches: &[Node<'_>]) -> bool {
        self.single_line_only && branches.iter().copied().any(holds_several)
    }

    /// `correction_exceeds_line_limit?`: the assignment moved onto the conditional's first line
    /// must not push a line past `Layout/LineLength`.
    fn correction_exceeds_line_limit(&self, node: Node<'_>, statements: &[Assignment<'_>]) -> bool {
        let Some(max) = self.line_length else {
            return false;
        };
        let Some(assignment) = statements.first().map(|statement| statement.lhs.clone()) else {
            return false;
        };
        let pattern = format!(r"\s*{}", regex::escape(&assignment).replace(' ', r"\s*"));
        let Ok(pattern) = regex::Regex::new(&pattern) else {
            return false;
        };
        let longest = self
            .context
            .source
            .node_text(node)
            .lines()
            .map(|line| {
                pattern
                    .replacen(line.trim_end_matches('\r'), 1, "")
                    .chars()
                    .count()
            })
            .max()
            .unwrap_or_default();
        assignment.chars().count() + longest > max
    }
}

/// What a branch's last statement assigns, as the pieces the correction is written from.
struct Assignment<'t> {
    kind: Kind,
    /// `lhs`: the text the assignment is written with, up to and including the operator.
    lhs: String,
    /// The value assigned, which is what is left where the assignment stood.
    value: Node<'t>,
    /// The whole statement, which the correction replaces with the value.
    node: Node<'t>,
    /// Whether the call assigns an element (`a[0] = 1`), which is the one setter a ternary needs
    /// no parentheses around.
    element_setter: bool,
}

/// The node type upstream's parser builds, which `assignment_types_match?` compares.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    Lvasgn,
    Ivasgn,
    Cvasgn,
    Gvasgn,
    Casgn,
    Masgn,
    OpAsgn,
    OrAsgn,
    AndAsgn,
    Send,
}

impl Cop<'_, '_> {
    /// `assignment_type?` together with `lhs`: what the statement assigns and how it says so.
    fn classify<'t>(&self, node: Node<'t>) -> Option<Assignment<'t>> {
        let text = |inner: Node<'_>| self.context.source.node_text(inner).to_owned();
        match node.kind_str() {
            "assignment" => {
                let left = node.field("left")?;
                let value = node.field("right")?;
                let (kind, lhs) = match left.kind_str() {
                    "identifier" => (Kind::Lvasgn, format!("{} = ", text(left))),
                    "instance_variable" => (Kind::Ivasgn, format!("{} = ", text(left))),
                    "class_variable" => (Kind::Cvasgn, format!("{} = ", text(left))),
                    "global_variable" => (Kind::Gvasgn, format!("{} = ", text(left))),
                    "constant" | "scope_resolution" => (Kind::Casgn, format!("{} = ", text(left))),
                    "left_assignment_list" => (Kind::Masgn, format!("{} = ", text(left))),
                    // `a[0] = 1` and `a.b = 1` are calls upstream, named after the setter.
                    "element_reference" => {
                        let object = left.field("object")?;
                        let mut indices = super::nodes::children(left);
                        indices.retain(|index| index.id() != object.id());
                        let written: Vec<String> =
                            indices.iter().map(|index| text(*index)).collect();
                        (
                            Kind::Send,
                            format!("{}[{}] = ", text(object), written.join(", ")),
                        )
                    }
                    "call" => {
                        let method = left.field("method")?;
                        // `a&.b = 1` is a `csend` upstream, and this cop defines `on_send` without
                        // an `on_csend`, so upstream never visits it. `csend` is a *type of its
                        // own*, not a flavour of `send`: `send_type?` answers false for it. The
                        // grammar spells both as a `call` with different operators, so the
                        // distinction has to be made here or the cop reports what upstream is
                        // silent about.
                        if left
                            .field("operator")
                            .is_some_and(|operator| self.context.source.node_text(operator) == "&.")
                        {
                            return None;
                        }
                        let receiver = left.field("receiver").map_or_else(String::new, text);
                        (Kind::Send, format!("{receiver}.{} = ", text(method)))
                    }
                    _ => return None,
                };
                Some(Assignment {
                    kind,
                    lhs,
                    value,
                    node,
                    element_setter: left.kind_str() == "element_reference",
                })
            }
            "operator_assignment" => {
                let left = node.field("left")?;
                let value = node.field("right")?;
                let operator = node.field("operator")?;
                let written = self.context.source.node_text(operator);
                let kind = match written {
                    "||=" => Kind::OrAsgn,
                    "&&=" => Kind::AndAsgn,
                    _ => Kind::OpAsgn,
                };
                Some(Assignment {
                    kind,
                    lhs: format!("{} {written} ", text(left)),
                    value,
                    node,
                    element_setter: false,
                })
            }
            // `a << b` and `a == b` are calls upstream, named after the operator.
            "binary" => {
                let operator = node.field("operator")?;
                let written = self.context.source.node_text(operator);
                if !is_assignment_method(written) {
                    return None;
                }
                let left = node.field("left")?;
                Some(Assignment {
                    kind: Kind::Send,
                    lhs: format!("{} {written} ", text(left)),
                    value: node.field("right")?,
                    node,
                    element_setter: false,
                })
            }
            // A call written with an argument list rather than as an assignment.
            "call" => {
                let method = node.field("method")?;
                let name = self.context.source.node_text(method);
                if !is_assignment_method(name) {
                    return None;
                }
                let arguments = send_node::arguments(node);
                let value = arguments.last()?.first();
                let receiver = node.field("receiver").map_or_else(String::new, text);
                // `lhs_for_send`: a setter is written with an `=`, and every other operator with
                // the name it was called by.
                let lhs = match name.ends_with('=') && !COMPARISON_OPERATORS.contains(&name) {
                    true => format!("{receiver}.{} = ", &name[..name.len() - 1]),
                    false => format!("{receiver} {name} "),
                };
                Some(Assignment {
                    kind: Kind::Send,
                    lhs,
                    value,
                    node,
                    element_setter: name == "[]=",
                })
            }
            _ => None,
        }
    }

    /// `move_assignment_outside_condition`: the assignment is written once, above the conditional.
    fn move_assignment_outside_condition(
        &self,
        conditional: &Conditional<'_>,
        statements: &[Assignment<'_>],
    ) -> Vec<Edit> {
        if conditional.ternary {
            return vec![Edit {
                start: conditional.node.start_byte(),
                end: conditional.node.end_byte(),
                replacement: self.ternary_correction(conditional, statements),
                safe: true,
            }];
        }
        // `CaseCorrector` writes the `else` branch's assignment above; `IfCorrector` the first
        // branch's. The two agree, since every branch had to assign the same thing.
        let leading = match conditional.case_like {
            true => statements.last(),
            false => statements.first(),
        };
        let Some(leading) = leading else {
            return Vec::new();
        };
        let mut edits = vec![Edit {
            start: conditional.node.start_byte(),
            end: conditional.node.start_byte(),
            replacement: leading.lhs.clone(),
            safe: true,
        }];
        // `replace_branch_assignment` brackets a bare array where `correct_branches` does not:
        // an `if` writes the first and last branches the first way and the `elsif`s the second,
        // while a `case` writes only its `else` branch the first way.
        let bracketed: &[usize] = match conditional.case_like {
            true => &[statements.len() - 1],
            false => &[0, statements.len() - 1],
        };
        for (position, statement) in statements.iter().enumerate() {
            let source = self.context.source.node_text(statement.value);
            let replacement = match bracketed.contains(&position)
                && statement.value.kind_str() == "right_assignment_list"
            {
                true => format!("[{source}]"),
                false => source.to_owned(),
            };
            edits.push(Edit {
                start: statement.node.start_byte(),
                end: statement.node.end_byte(),
                replacement,
                safe: true,
            });
        }
        if let Some(end) = end_keyword(conditional.node) {
            edits.push(Edit {
                start: end.start_byte(),
                end: end.start_byte(),
                replacement: match self.keyword_aligned {
                    true => " ".repeat(leading.lhs.chars().count()),
                    false => String::new(),
                },
                safe: true,
            });
        }
        edits
    }

    /// `TernaryCorrector#correction`.
    fn ternary_correction(
        &self,
        conditional: &Conditional<'_>,
        statements: &[Assignment<'_>],
    ) -> String {
        let condition = conditional
            .node
            .field("condition")
            .map_or_else(String::new, |condition| {
                self.context.source.node_text(condition).to_owned()
            });
        let [if_branch, else_branch] = statements else {
            return String::new();
        };
        let expression = format!(
            "{condition} ? {} : {}",
            self.context.source.node_text(if_branch.value),
            self.context.source.node_text(else_branch.value)
        );
        // `element_assignment?`: a setter other than `[]=` binds looser than the ternary.
        let wrapped = if_branch.kind == Kind::Send && !if_branch.element_setter;
        match wrapped {
            true => format!("{}({expression})", if_branch.lhs),
            false => format!("{}{expression}", if_branch.lhs),
        }
    }

    /// `check_assignment_to_condition`: the `assign_inside_condition` half of the cop, which the
    /// bundled configuration never selects.
    fn check_assignments(&self, offenses: &mut Vec<Offense>) {
        for node in
            self.context
                .nodes_of_any(&["assignment", "operator_assignment", "binary", "call"])
        {
            // Every variable-assignment callback asks `part_of_ignored_node?`. An enclosing
            // assignment calls `ignore_node` before checking whether its RHS is a conditional,
            // so assignments inside a memoization block or a constant-assigned class are silent.
            if matches!(node.kind_str(), "assignment" | "operator_assignment")
                && self.ignored_by_outer_assignment(node)
            {
                continue;
            }
            let Some(assignment) = self.classify(node) else {
                continue;
            };
            // `assignment_rhs_exist?`: a target of a multiple assignment or a `rescue => e` has no
            // right-hand side of its own.
            if node.parent_of(self.context).is_some_and(|parent| {
                matches!(parent.kind_str(), "left_assignment_list" | "rescue")
            }) {
                continue;
            }
            let value = strip_parentheses(assignment.value);
            let Some(conditional) = self.conditional(value, true) else {
                continue;
            };
            // RuboCop 1.89's ternary corrector produces an unparsable rewrite for a multiple
            // assignment (`cond ? a, b = x : a, b = y`), and its reparse guard suppresses it.
            if conditional.ternary && assignment.kind == Kind::Masgn {
                continue;
            }
            if conditional.ternary && !self.include_ternary {
                continue;
            }
            // `allowed_single_line?` is handed the conditional's own children here rather than
            // the branch bodies: a `case` puts its `when` nodes there, and only its `else` body
            // can be the `begin` that makes the conditional too long to move an assignment into.
            if self.single_line_only && raw_branches(value).into_iter().any(holds_several) {
                continue;
            }
            let edits = self.move_assignment_inside_condition(node, &assignment, &conditional);
            if edits.is_empty() || !support::correction_parses(self.context, &edits) {
                continue;
            }
            offenses.push(
                self.context
                    .offense(ASSIGN_TO_CONDITION_MSG, node.byte_range())
                    .corrected_by_all(edits),
            );
        }
    }

    fn ignored_by_outer_assignment(&self, node: Node<'_>) -> bool {
        let mut ancestor = node.parent_of(self.context);
        while let Some(parent) = ancestor {
            if self.classify(parent).is_some()
                && !parent.parent_of(self.context).is_some_and(|owner| {
                    matches!(owner.kind_str(), "left_assignment_list" | "rescue")
                })
            {
                return true;
            }
            ancestor = parent.parent_of(self.context);
        }
        false
    }

    /// `move_assignment_inside_condition`: the assignment is written again in each branch.
    fn move_assignment_inside_condition(
        &self,
        node: Node<'_>,
        assignment: &Assignment<'_>,
        conditional: &Conditional<'_>,
    ) -> Vec<Edit> {
        let value = strip_parentheses(assignment.value);
        let prefix = self
            .context
            .source
            .slice(node.start_byte()..assignment.value.start_byte())
            .to_owned();
        let mut edits = vec![Edit {
            start: node.start_byte(),
            end: assignment.value.start_byte(),
            replacement: String::new(),
            safe: true,
        }];
        // The parentheses that grouped the conditional are what the assignment used to need.
        if value.id() != assignment.value.id() {
            edits.push(Edit {
                start: assignment.value.start_byte(),
                end: value.start_byte(),
                replacement: String::new(),
                safe: true,
            });
            edits.push(Edit {
                start: value.end_byte(),
                end: assignment.value.end_byte(),
                replacement: String::new(),
                safe: true,
            });
        }
        // `remove_whitespace_in_branches`: the lines are **aligned**, not shifted. Upstream removes
        // `child.column - column - 2` in front of each node of a branch, which lands every one of
        // them on `column + 2` whatever it started from, and `else.column - column` in front of an
        // `elsif` / `else` / `end`, which lands those on `column`. Taking the same width off every
        // line instead leaves an uneven source uneven: `bar = if foo` with its first branch written
        // four columns deeper than the rest kept those four columns.
        let assignment_column = self.context.source.line_column(node.start_byte()).1;
        if !conditional.ternary {
            let text = self.context.source.text();
            let literal_interiors = literal_interiors(self.context, &value.byte_range());
            let mut line_start = value.start_byte();
            while let Some(newline) = text[line_start..value.end_byte()].find('\n') {
                line_start += newline + 1;
                let rest = &text[line_start..value.end_byte()];
                let blanks = rest
                    .as_bytes()
                    .iter()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count();
                // The corrector moves branch nodes, not physical lines inside their literals.
                // In particular, regex/string continuation lines and heredoc bodies retain their
                // original indentation when the assignment prefix moves into the branch.
                if literal_interiors
                    .iter()
                    .any(|range| range.contains(&(line_start + blanks)))
                {
                    continue;
                }
                // `line_column` is 1-based; the columns upstream compares are 0-based.
                let column = self.context.source.line_column(line_start + blanks).1 - 1;
                // `condition.loc.end` is **the conditional's own** `end`, which is the last thing
                // the node holds. A block written inside a branch ends with an `end` too, and
                // pulling that one back to the assignment's column unindented the branch body.
                let closes_the_conditional = rest[blanks..].starts_with("end")
                    && line_start + blanks + 3 >= value.end_byte();
                let keyword = rest[blanks..].starts_with("elsif")
                    || rest[blanks..].starts_with("else")
                    || closes_the_conditional
                    || rest[blanks..].starts_with("when")
                    || rest[blanks..].starts_with("in ");
                let target = match keyword {
                    true => assignment_column.saturating_sub(1),
                    false => assignment_column.saturating_sub(1) + 2,
                };
                let removable = column.saturating_sub(target).min(blanks);
                if removable > 0 {
                    edits.push(Edit {
                        start: line_start,
                        end: line_start + removable,
                        replacement: String::new(),
                        safe: true,
                    });
                }
            }
        }
        for branch in &conditional.branches {
            let statement = tail(*branch);
            edits.push(Edit {
                start: statement.start_byte(),
                end: statement.start_byte(),
                replacement: prefix.clone(),
                safe: true,
            });
        }
        edits
    }
}

/// Literal text whose line indentation is content rather than branch indentation.
fn literal_interiors(
    context: &RuleContext<'_>,
    expression: &std::ops::Range<usize>,
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    for node in context.nodes_of_any(&["string", "regex", "subshell"]) {
        if node.start_byte() < expression.start || node.end_byte() > expression.end {
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
        ranges.push(first.end_byte()..last.start_byte());
    }
    ranges.extend(
        context
            .nodes_of("heredoc_body")
            .filter(|body| body.end_byte() > expression.start && body.start_byte() < expression.end)
            .map(|body| body.byte_range()),
    );
    ranges
}

/// The statements a branch holds, or `None` when it holds nothing.
///
/// Upstream's branch is one node -- a `begin` when more than one statement was written -- which is
/// what `tail` reads the last statement off of.
fn branch<'t>(node: Option<Node<'t>>) -> Option<Node<'t>> {
    let node = node?;
    (!super::nodes::children(node).is_empty()).then_some(node)
}

/// `_condition, *branches, else_branch = *assignment`: the conditional's children after its
/// condition, which for a `case` are the `when` clauses rather than their bodies.
fn raw_branches<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    match node.kind_str() {
        "case" | "case_match" => super::nodes::children(node)
            .into_iter()
            .filter(|child| matches!(child.kind_str(), "when" | "in_clause" | "else"))
            .collect(),
        _ => [node.field("consequence"), node.field("alternative")]
            .into_iter()
            .flatten()
            .collect(),
    }
}

/// `begin_type?`: whether the branch upstream wraps more than one statement, which only a branch
/// written between keywords can.
fn holds_several(branch: Node<'_>) -> bool {
    matches!(branch.kind_str(), "then" | "else") && super::nodes::children(branch).len() > 1
}

/// `tail`: the statement a branch ends with, which is the one that assigns.
fn tail<'t>(branch: Node<'t>) -> Node<'t> {
    match branch.kind_str() {
        "then" | "else" => super::nodes::children(branch)
            .last()
            .copied()
            .unwrap_or(branch),
        _ => branch,
    }
}

/// A conditional written in parentheses, which upstream reads as a `begin` around it.
fn strip_parentheses<'t>(node: Node<'t>) -> Node<'t> {
    if node.kind_str() != "parenthesized_statements" {
        return node;
    }
    let children = super::nodes::children(node);
    match children.as_slice() {
        [only] => *only,
        _ => node,
    }
}

fn end_keyword<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
    children
        .into_iter()
        .rev()
        .find(|child| !child.is_named() && child.kind_str() == "end")
}
