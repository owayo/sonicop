//! `Style/IfUnlessModifier`: a body of one statement belongs behind its condition, and a modifier
//! that made its line too long belongs back in block form.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use super::conditional::{
    UpstreamParent, descendants, first_line, last_line, self_statements, token, upstream_parent,
};
use super::line_length_help::LineLengthHelp;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG_USE_MODIFIER: &str = "Favor modifier `%<keyword>s` usage when having a single-line body. \
     Another good alternative is the usage of control flow `&&`/`||`.";
const MSG_USE_MODIFIER_PARENS: &str = "Favor modifier `%<keyword>s` usage when having a single-line body. Wrap the expression in \
     parentheses to keep the current behavior, as it is part of a larger expression.";
const MSG_USE_NORMAL: &str = "Modifier form of `%<keyword>s` makes the line too long.";

/// Node kinds standing for a `dstr`: a string whose parts are evaluated, which an `if` written
/// inside an interpolation of would be a descendant of.
const DSTR_KINDS: &[&str] = &["string", "bare_string", "heredoc_body"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        length: LineLengthHelp::new(context),
        comments: CommentIndex::new(context),
        heredocs: HeredocIndex::new(context),
    };
    let mut ignored: Vec<Range<usize>> = Vec::new();

    for node in context.nodes_of_any(&["if", "unless", "if_modifier", "unless_modifier"]) {
        let Some(conditional) = Conditional::new(node) else {
            continue;
        };
        if cop.endless_method_body(&conditional) || has_dstr_ancestor(node) {
            continue;
        }
        if cop.defined_argument_is_undefined(&conditional)
            || has_pattern_matching(conditional.condition)
        {
            continue;
        }
        let Some(message) = cop.message(&conditional) else {
            continue;
        };
        let offense = context.offense(
            message.replace("%<keyword>s", cop.keyword(&conditional)),
            conditional.keyword.byte_range(),
        );
        // `part_of_ignored_node?`: a conditional inside one that has already been rewritten is
        // reported but not corrected, since the enclosing rewrite covers its text.
        let correctable = !ignored
            .iter()
            .any(|range| range.start <= node.start_byte() && range.end >= node.end_byte())
            && !cop.another_modifier_if_on_same_line(&conditional);
        if correctable {
            offenses.push(offense.corrected_by_all(cop.autocorrect(&conditional)));
            ignored.push(node.byte_range());
        } else {
            offenses.push(offense);
        }
    }
}

struct Cop<'a> {
    context: &'a RuleContext<'a>,
    length: LineLengthHelp<'a>,
    comments: CommentIndex,
    heredocs: HeredocIndex,
}

/// One `if`/`unless`, read the way upstream's `IfNode` presents it.
struct Conditional<'t> {
    node: Node<'t>,
    modifier: bool,
    keyword: Node<'t>,
    condition: Node<'t>,
    /// The `end` keyword, which a modifier form has none of.
    end: Option<Node<'t>>,
    body: Body<'t>,
    /// `else?`, which an `elsif` counts towards as well.
    has_else: bool,
}

/// `node.body` as upstream builds it out of the `then` clause.
#[derive(Clone, Copy)]
enum Body<'t> {
    Missing,
    One(Node<'t>),
    /// More than one statement, or a parenthesized one: a `begin` either way.
    Begin,
}

impl<'t> Conditional<'t> {
    fn new(node: Node<'t>) -> Option<Self> {
        let modifier = matches!(node.kind(), "if_modifier" | "unless_modifier");
        let condition = node.child_by_field_name("condition")?;
        let keyword = token(node, &["if", "unless"])?;
        if modifier {
            return Some(Self {
                node,
                modifier,
                keyword,
                condition,
                end: None,
                body: Body::One(node.child_by_field_name("body")?),
                has_else: false,
            });
        }
        let statements = node
            .child_by_field_name("consequence")
            .map(super::nodes::children)
            .unwrap_or_default();
        let body = match statements.as_slice() {
            [] => Body::Missing,
            // `(foo)` is a `begin` holding one statement, not the statement itself.
            [only] if only.kind() != "parenthesized_statements" => Body::One(*only),
            _ => Body::Begin,
        };
        Some(Self {
            node,
            modifier,
            keyword,
            condition,
            end: token(node, &["end"]),
            body,
            has_else: node.child_by_field_name("alternative").is_some(),
        })
    }

    fn body_node(&self) -> Option<Node<'t>> {
        match self.body {
            Body::One(node) => Some(node),
            _ => None,
        }
    }
}

impl Cop<'_> {
    fn keyword<'a>(&'a self, conditional: &Conditional<'_>) -> &'a str {
        self.context.source.node_text(conditional.keyword)
    }

    fn source(&self, node: Node<'_>) -> &str {
        self.context.source.node_text(node)
    }

    /// `endless_method?`: the body is a `def` written with `=` rather than an `end`, which
    /// `Style/AmbiguousEndlessMethodDefinition` asks to be left in block form.
    fn endless_method_body(&self, conditional: &Conditional<'_>) -> bool {
        conditional.body_node().is_some_and(|body| {
            matches!(body.kind(), "method" | "singleton_method") && token(body, &["end"]).is_none()
        })
    }

    /// `defined_nodes(condition).any? { |n| defined_argument_is_undefined?(node, n) }`.
    fn defined_argument_is_undefined(&self, conditional: &Conditional<'_>) -> bool {
        defined_nodes(conditional.condition)
            .into_iter()
            .any(|defined| self.argument_is_undefined(conditional.node, defined))
    }

    fn argument_is_undefined(&self, node: Node<'_>, defined: Node<'_>) -> bool {
        let Some(argument) = defined_argument(defined) else {
            return false;
        };
        // `first_argument.type?(:lvar, :send)`: anything else -- an ivar, a constant -- is left to
        // the rest of the cop.
        let name = match argument.kind() {
            "identifier" => self.source(argument),
            // A call is a `send`, whose `node_parts[0]` is its receiver and so never equal to the
            // name of a local variable assignment.
            "call" | "method_call" => return true,
            _ => return false,
        };
        !left_siblings(node)
            .into_iter()
            .any(|sibling| self.assigns_local(sibling, name))
    }

    /// Whether the statement is an `lvasgn` writing `name`.
    fn assigns_local(&self, node: Node<'_>, name: &str) -> bool {
        node.kind() == "assignment"
            && node
                .child_by_field_name("left")
                .is_some_and(|left| left.kind() == "identifier" && self.source(left) == name)
    }

    fn message(&self, conditional: &Conditional<'_>) -> Option<&'static str> {
        if self.single_line_as_modifier(conditional)
            && !self.named_capture_in_condition(conditional)
        {
            return Some(match self.parenthesize(conditional) {
                true => MSG_USE_MODIFIER_PARENS,
                false => MSG_USE_MODIFIER,
            });
        }
        self.too_long_due_to_modifier(conditional)
            .then_some(MSG_USE_NORMAL)
    }

    /// `named_capture_in_condition?`: the condition is a `match_with_lvasgn`.
    ///
    /// The parser builds that node for *every* `=~` whose left side is a regexp it can compile at
    /// parse time (`Builders::Default#match_op` tests the capture list for truth, and an empty
    /// list is still truthy), so a regexp without a single named group counts too.
    fn named_capture_in_condition(&self, conditional: &Conditional<'_>) -> bool {
        let condition = conditional.condition;
        condition.kind() == "binary"
            && condition
                .child_by_field_name("operator")
                .is_some_and(|operator| self.source(operator) == "=~")
            && condition
                .child_by_field_name("left")
                .is_some_and(|left| left.kind() == "regex" && is_static_regexp(left))
    }

    fn single_line_as_modifier(&self, conditional: &Conditional<'_>) -> bool {
        if self.non_eligible_node(conditional)
            || self.non_eligible_body(conditional)
            || non_eligible_condition(conditional.condition)
        {
            return false;
        }
        self.modifier_fits_on_single_line(conditional)
    }

    fn non_eligible_node(&self, conditional: &Conditional<'_>) -> bool {
        conditional.modifier
            || conditional.has_else
            || self.chained(conditional.node)
            || self.nested_conditional(conditional)
            || self.multiline_inside_collection(conditional)
            || nonempty_line_count(self.source(conditional.node)) > 3
            || self.comments.on_line(last_line(conditional.node))
            || (self.first_line_comment(conditional.node).is_some()
                && self.code_after(conditional).is_some())
    }

    /// `chained?`: the conditional stands where a receiver goes, so the call after it would bind to
    /// the condition instead once the form changed.
    fn chained(&self, node: Node<'_>) -> bool {
        let Some(parent) = node.parent() else {
            return false;
        };
        let receiver = match parent.kind() {
            "call" | "method_call" => parent.child_by_field_name("receiver"),
            "element_reference" => parent.child_by_field_name("object"),
            "binary" => parent
                .child_by_field_name("operator")
                .filter(|operator| !is_operator_keyword(self.source(*operator)))
                .and_then(|_| parent.child_by_field_name("left")),
            "unary" => parent
                .child_by_field_name("operator")
                .filter(|operator| self.source(*operator) != "defined?")
                .and_then(|_| parent.child_by_field_name("operand")),
            _ => None,
        };
        receiver.is_some_and(|receiver| receiver.id() == node.id())
    }

    /// `nested_conditional?`: another conditional inside the body, which the modifier form would
    /// have to hold on the same line.
    fn nested_conditional(&self, conditional: &Conditional<'_>) -> bool {
        let Some(consequence) = conditional.node.child_by_field_name("consequence") else {
            return false;
        };
        let mut found = false;
        crate::rules::walk_named(consequence, &mut |node| {
            // `elsif` is written as a nested `if` upstream and does not count; here it is its own
            // kind, so only the four spellings of a real conditional are looked for.
            found |= matches!(
                node.kind(),
                "if" | "unless" | "if_modifier" | "unless_modifier" | "conditional"
            );
        });
        found
    }

    fn non_eligible_body(&self, conditional: &Conditional<'_>) -> bool {
        match conditional.body {
            Body::Missing | Body::Begin => true,
            Body::One(body) => {
                body.byte_range().is_empty() || self.comments.in_lines(line_span(body))
            }
        }
    }

    /// `modifier_fits_on_single_line?`: the line the correction would write is put to
    /// `Layout/LineLength` exactly as an existing line is, exemptions included.
    fn modifier_fits_on_single_line(&self, conditional: &Conditional<'_>) -> bool {
        self.length.acceptable_line_length(
            &self.line_in_modifier_form(conditional),
            first_line(conditional.node),
        )
    }

    fn line_in_modifier_form(&self, conditional: &Conditional<'_>) -> String {
        let line = self.context.source.line(first_line(conditional.keyword));
        let column = conditional.keyword.start_position().column;
        let before: String = line.chars().take(column).collect();
        format!(
            "{before}{}{}",
            self.to_modifier_form(conditional),
            self.code_after(conditional).unwrap_or_default()
        )
    }

    /// `to_modifier_form`: the body, the keyword, the condition, wrapped and commented as the
    /// surroundings need.
    fn to_modifier_form(&self, conditional: &Conditional<'_>) -> String {
        let body = self.if_body_source(conditional);
        let mut expression = format!(
            "{body} {} {}",
            self.keyword(conditional),
            self.source(conditional.condition)
        );
        if self.parenthesize(conditional) {
            expression = format!("({expression})");
        }
        match self.first_line_comment(conditional.node) {
            Some(comment) => format!("{expression} {}", &self.context.source.text()[comment]),
            None => expression,
        }
    }

    /// `if_body_source`: a call whose last argument ends in an omitted hash value needs its
    /// parentheses back, or the modifier keyword would be read as part of that value.
    fn if_body_source(&self, conditional: &Conditional<'_>) -> String {
        let Some(body) = conditional.body_node() else {
            return String::new();
        };
        match self.omitted_value_call(body) {
            Some(rewritten) => rewritten,
            None => self.source(body).to_owned(),
        }
    }

    fn omitted_value_call(&self, body: Node<'_>) -> Option<String> {
        if !matches!(body.kind(), "call" | "method_call") {
            return None;
        }
        let list = body.child_by_field_name("arguments")?;
        let arguments = super::nodes::children(list);
        let last = arguments.last()?;
        // `foo(bar:)` parks the pairs straight in the argument list, so the omitted value shows up
        // as a pair without one.
        let omitted = match last.kind() {
            "pair" => last.child_by_field_name("value").is_none(),
            "hash" => super::nodes::children(*last)
                .last()
                .is_some_and(|pair| pair.child_by_field_name("value").is_none()),
            _ => false,
        };
        if !omitted {
            return None;
        }
        let selector = body.child_by_field_name("method")?;
        let head = &self.context.source.text()[body.start_byte()..selector.end_byte()];
        let joined = arguments
            .iter()
            .map(|argument| self.source(*argument))
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("{head}({joined})"))
    }

    /// `parenthesize?`: modifier `if` binds so loosely that a conditional standing inside a larger
    /// expression has to keep parentheses around it.
    fn parenthesize(&self, conditional: &Conditional<'_>) -> bool {
        match upstream_parent(conditional.node) {
            Some(UpstreamParent::Node(parent)) => match parent.kind() {
                "assignment"
                | "operator_assignment"
                | "array"
                | "right_assignment_list"
                | "pair"
                | "call"
                | "method_call"
                | "element_reference" => true,
                // An `and`/`or` is an operator keyword and everything else spelled this way is a
                // call, so either branch of upstream's test is satisfied.
                "binary" => true,
                "unary" => parent
                    .child_by_field_name("operator")
                    .is_some_and(|operator| self.source(operator) != "defined?"),
                _ => false,
            },
            _ => false,
        }
    }

    /// `first_line_comment`: the comment closing the conditional's own line, which the modifier
    /// form carries along -- unless it is the directive switching this cop off.
    fn first_line_comment(&self, node: Node<'_>) -> Option<Range<usize>> {
        let comment = self.comments.at_line(first_line(node))?;
        let text = &self.context.source.text()[comment.clone()];
        (!DISABLES_COP.is_match(text)).then_some(comment)
    }

    /// `code_after`: whatever follows the `end` on its line, which the modifier form has to keep.
    fn code_after(&self, conditional: &Conditional<'_>) -> Option<String> {
        let end = conditional.end?;
        let line = self.context.source.line(last_line(end));
        let line = line.strip_suffix('\n').unwrap_or(line);
        let column = end.end_position().column;
        let code: String = line.chars().skip(column).collect();
        (!code.is_empty()).then_some(code)
    }

    fn too_long_due_to_modifier(&self, conditional: &Conditional<'_>) -> bool {
        conditional.modifier
            && self.too_long_single_line(conditional.node)
            && !another_statement_on_same_line(conditional.node)
    }

    fn too_long_single_line(&self, node: Node<'_>) -> bool {
        if node.start_position().row != node.end_position().row {
            return false;
        }
        let number = first_line(node);
        let line = self.context.source.line(number);
        !self
            .length
            .acceptable_line_length(line.strip_suffix('\n').unwrap_or(line), number)
    }

    /// `multiline_inside_collection?`: two multi-line conditionals sharing a line inside one
    /// collection cannot both be rewritten without running them together.
    fn multiline_inside_collection(&self, conditional: &Conditional<'_>) -> bool {
        if conditional.modifier {
            return false;
        }
        let Some(collection) = containing_collection(conditional.node) else {
            return false;
        };
        collection_children(collection)
            .into_iter()
            .any(|child| self.sibling_if_shares_line(child, conditional))
    }

    fn sibling_if_shares_line(&self, child: Node<'_>, conditional: &Conditional<'_>) -> bool {
        let Some(inner) = unwrap_begin(child) else {
            return false;
        };
        if !matches!(
            inner.kind(),
            "if" | "unless" | "if_modifier" | "unless_modifier"
        ) {
            return false;
        }
        let shares_start = conditional
            .end
            .is_some_and(|end| first_line(inner) == first_line(end));
        let shares_end = token(inner, &["end"])
            .is_some_and(|end| first_line(end) == first_line(conditional.node));
        shares_start || shares_end
    }

    fn another_modifier_if_on_same_line(&self, conditional: &Conditional<'_>) -> bool {
        let Some(collection) = containing_collection(conditional.node) else {
            return false;
        };
        let line = first_line(conditional.node);
        let mut found = false;
        crate::rules::walk_named(collection, &mut |node| {
            found |= node.id() != conditional.node.id()
                && matches!(node.kind(), "if_modifier" | "unless_modifier")
                && first_line(node) == line;
        });
        found
    }

    fn autocorrect(&self, conditional: &Conditional<'_>) -> Vec<Edit> {
        match conditional.modifier {
            true => self.replacement_for_modifier_form(conditional),
            false => vec![replace(
                conditional.node,
                self.to_modifier_form(conditional),
            )],
        }
    }

    fn replacement_for_modifier_form(&self, conditional: &Conditional<'_>) -> Vec<Edit> {
        let indent = " ".repeat(conditional.node.start_position().column);
        let body = self.body_source(conditional);
        let moved_comment = self
            .comments
            .at_line(first_line(conditional.node))
            .filter(|comment| self.too_long_due_to_comment_after_modifier(conditional, comment));
        if let Some(comment) = moved_comment {
            let moved = &self.context.source.text()[comment.clone()];
            return vec![
                remove(self.with_space_on_the_left(comment.clone())),
                replace(
                    conditional.node,
                    format!(
                        "{moved}\n{indent}{body} {} {}",
                        self.keyword(conditional),
                        self.source(conditional.condition)
                    ),
                ),
            ];
        }
        let heredoc = conditional
            .body_node()
            .filter(|body| matches!(body.kind(), "call" | "method_call"))
            .and_then(|body| self.trailing_heredoc(body));
        let head = format!(
            "{} {}\n{indent}  {body}",
            self.keyword(conditional),
            self.source(conditional.condition)
        );
        match heredoc {
            Some(heredoc) => vec![
                remove(heredoc.lines.clone()),
                replace(
                    conditional.node,
                    format!(
                        "{head}\n{indent}  {}\n{indent}  {}\n{indent}end",
                        heredoc.body.trim_end_matches('\n'),
                        heredoc.terminator
                    ),
                ),
            ],
            None => vec![replace(conditional.node, format!("{head}\n{indent}end"))],
        }
    }

    fn body_source(&self, conditional: &Conditional<'_>) -> String {
        conditional
            .body_node()
            .map(|body| self.source(body).to_owned())
            .unwrap_or_default()
    }

    /// `too_long_due_to_comment_after_modifier?`: the line is over the limit only because of the
    /// comment closing it, so lifting the comment is enough.
    fn too_long_due_to_comment_after_modifier(
        &self,
        conditional: &Conditional<'_>,
        comment: &Range<usize>,
    ) -> bool {
        let Some(max) = self.length.max() else {
            return false;
        };
        let line = self.context.source.line(first_line(conditional.node));
        // `processed_source.lines` holds the line without its newline, and both lengths here are
        // character counts.
        let length = line.strip_suffix('\n').unwrap_or(line).chars().count();
        let comment_length = self.context.source.text()[comment.clone()].chars().count();
        length.saturating_sub(comment_length) <= max && max <= length
    }

    /// `range_with_surrounding_space(side: :left)` over a comment: the blanks in front of it go
    /// with it, so removing it leaves no trailing whitespace behind.
    fn with_space_on_the_left(&self, comment: Range<usize>) -> Range<usize> {
        let text = self.context.source.text().as_bytes();
        let mut start = comment.start;
        while start > 0 && matches!(text[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        while start > 0 && text[start - 1] == b'\n' {
            start -= 1;
        }
        start..comment.end
    }

    /// The heredoc opened by the call's last argument, with the lines the correction lifts into the
    /// block form.
    fn trailing_heredoc(&self, body: Node<'_>) -> Option<Heredoc> {
        let list = body.child_by_field_name("arguments")?;
        let last = super::nodes::children(list).pop()?;
        if last.kind() != "heredoc_beginning" {
            return None;
        }
        self.heredocs.body_for(last, self.context)
    }
}

/// The body and terminator of one heredoc, plus the lines the correction removes.
struct Heredoc {
    body: String,
    terminator: String,
    lines: Range<usize>,
}

fn replace(node: Node<'_>, replacement: String) -> Edit {
    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement,
        safe: true,
    }
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

fn line_span(node: Node<'_>) -> Range<usize> {
    first_line(node)..last_line(node) + 1
}

fn nonempty_line_count(source: &str) -> usize {
    source
        .lines()
        .filter(|line| line.contains(|character: char| !character.is_whitespace()))
        .count()
}

fn is_operator_keyword(operator: &str) -> bool {
    matches!(operator, "&&" | "||" | "and" | "or")
}

/// `condition.each_node.any?(&:lvasgn_type?)`: a condition that binds a local cannot move behind
/// the body that reads it.
fn non_eligible_condition(condition: Node<'_>) -> bool {
    descendants(condition).into_iter().any(|node| {
        matches!(node.kind(), "assignment" | "operator_assignment")
            && node.child_by_field_name("left").is_some_and(binds_local)
    })
}

/// Whether the left-hand side of an assignment writes a local anywhere in it. A multiple
/// assignment is one `masgn` upstream, but every name it writes is still an `lvasgn` beneath it.
fn binds_local(left: Node<'_>) -> bool {
    match left.kind() {
        "identifier" => true,
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
            super::nodes::children(left).into_iter().any(binds_local)
        }
        _ => false,
    }
}

fn has_dstr_ancestor(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if DSTR_KINDS.contains(&parent.kind()) {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// `pattern_matching_nodes(condition).any?`: `in` and `=>` bind names the modifier form would
/// leave undefined.
fn has_pattern_matching(condition: Node<'_>) -> bool {
    let mut found = false;
    crate::rules::walk_named(condition, &mut |node| {
        found |= matches!(node.kind(), "match_pattern" | "test_pattern");
    });
    found
}

/// `defined_nodes(condition)`: the condition itself when it is a `defined?`, its `defined?`
/// descendants otherwise.
fn defined_nodes(condition: Node<'_>) -> Vec<Node<'_>> {
    let is_defined = |node: Node<'_>| {
        node.kind() == "unary"
            && node
                .child_by_field_name("operator")
                .is_some_and(|operator| operator.kind() == "defined?")
    };
    if is_defined(condition) {
        return vec![condition];
    }
    descendants(condition)
        .into_iter()
        .filter(|node| node.id() != condition.id() && is_defined(*node))
        .collect()
}

/// `defined_node.first_argument`, with the parentheses upstream's parser does not build a node for.
fn defined_argument<'t>(defined: Node<'t>) -> Option<Node<'t>> {
    let operand = defined.child_by_field_name("operand")?;
    if operand.kind() != "parenthesized_statements" {
        return Some(operand);
    }
    let statements = super::nodes::children(operand);
    match statements.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `node.left_siblings`, which only ever holds statements when upstream's parent is a `begin`.
fn left_siblings<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let Some(UpstreamParent::Begin(container)) = upstream_parent(node) else {
        return Vec::new();
    };
    let statements = self_statements(container);
    let Some(index) = statements
        .iter()
        .position(|statement| covers(*statement, node))
    else {
        return Vec::new();
    };
    statements[..index].to_vec()
}

/// `another_statement_on_same_line?`: a statement written after the modifier on its line, which the
/// block form would swallow.
fn another_statement_on_same_line(node: Node<'_>) -> bool {
    let line = last_line(node);
    let Some(UpstreamParent::Begin(container)) = upstream_parent(node) else {
        return false;
    };
    let statements = self_statements(container);
    let Some(index) = statements
        .iter()
        .position(|statement| covers(*statement, node))
    else {
        return false;
    };
    statements
        .get(index + 1)
        .is_some_and(|next| first_line(*next) == line)
}

fn covers(outer: Node<'_>, inner: Node<'_>) -> bool {
    outer.start_byte() <= inner.start_byte() && outer.end_byte() >= inner.end_byte()
}

/// `find_containing_collection`: the array, call or hash the conditional is written inside.
fn containing_collection<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let ancestor = match upstream_parent(node)? {
        UpstreamParent::Begin(begin) => match upstream_parent(begin)? {
            UpstreamParent::Begin(outer) => outer,
            UpstreamParent::Node(outer) => outer,
        },
        UpstreamParent::Node(parent) => parent,
    };
    match ancestor.kind() {
        "array" | "call" | "method_call" => Some(ancestor),
        "pair" => ancestor.parent(),
        _ => None,
    }
}

/// The children upstream's node holds, which for a call are its receiver and arguments.
fn collection_children<'t>(collection: Node<'t>) -> Vec<Node<'t>> {
    let mut children = Vec::new();
    let mut cursor = collection.walk();
    for child in collection.named_children(&mut cursor) {
        if child.kind() == "argument_list" {
            children.extend(super::nodes::children(child));
        } else if super::nodes::is_child(child) {
            children.push(child);
        }
    }
    children
}

/// `unwrap_begin`: a pair stands for its value and a `begin` for the statement it holds.
fn unwrap_begin<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let node = match node.kind() {
        "pair" => node.child_by_field_name("value")?,
        _ => node,
    };
    match node.kind() {
        "parenthesized_statements" => super::nodes::children(node).first().copied(),
        _ => Some(node),
    }
}

/// The comments of the file indexed by line, as `processed_source.comment_index` holds them.
struct CommentIndex {
    by_line: HashMap<usize, Range<usize>>,
}

impl CommentIndex {
    fn new(context: &RuleContext<'_>) -> Self {
        Self {
            by_line: context
                .comment_ranges()
                .iter()
                .map(|range| (context.source.line_column(range.start).0, range.clone()))
                .collect(),
        }
    }

    fn at_line(&self, line: usize) -> Option<Range<usize>> {
        self.by_line.get(&line).cloned()
    }

    fn on_line(&self, line: usize) -> bool {
        self.by_line.contains_key(&line)
    }

    /// `contains_comment?`, which asks about whole lines rather than the range itself.
    fn in_lines(&self, lines: Range<usize>) -> bool {
        lines.into_iter().any(|line| self.on_line(line))
    }
}

/// The heredoc bodies of the file, paired with their openers in the order both appear.
struct HeredocIndex {
    beginnings: Vec<usize>,
    bodies: Vec<(usize, usize)>,
}

impl HeredocIndex {
    fn new(context: &RuleContext<'_>) -> Self {
        Self {
            beginnings: context
                .nodes_of("heredoc_beginning")
                .map(|node| node.start_byte())
                .collect(),
            bodies: context
                .nodes_of("heredoc_body")
                .map(|node| (node.start_byte(), node.end_byte()))
                .collect(),
        }
    }

    fn body_for(&self, beginning: Node<'_>, context: &RuleContext<'_>) -> Option<Heredoc> {
        let index = self
            .beginnings
            .iter()
            .position(|start| *start == beginning.start_byte())?;
        let (start, end) = *self.bodies.get(index)?;
        let text = context.source.text();
        // The grammar hangs the newline that closed the opener's line off the front of the body.
        let content_start = start + usize::from(text[start..end].starts_with('\n'));
        let terminator_start = text[content_start..end]
            .rfind('\n')
            .map_or(content_start, |offset| content_start + offset + 1);
        let first = context.source.line_column(content_start).0;
        let last = context.source.line_column(terminator_start).0;
        Some(Heredoc {
            body: text[content_start..terminator_start].to_owned(),
            terminator: text[terminator_start..end]
                .trim_end_matches('\n')
                .to_owned(),
            lines: context.source.line_start(first)
                ..context
                    .source
                    .line_range(last)
                    .end
                    .min(context.source.text().len()),
        })
    }
}

/// `comment_disables_cop?`, whose `([^,],)*` only ever matches one-character names and so leaves a
/// directive listing this cop after another one alone.
static DISABLES_COP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#\s*rubocop\s*:\s*(?:disable|todo)\s*(?:[^,],)*\s*(?:all|Style/IfUnlessModifier)")
        .unwrap()
});

/// `static_regexp_node`: a regexp the parser can compile while parsing, which is any literal whose
/// parts are all plain text.
fn is_static_regexp(regexp: Node<'_>) -> bool {
    let mut cursor = regexp.walk();
    regexp
        .named_children(&mut cursor)
        .all(|part| part.kind() != "interpolation")
}
