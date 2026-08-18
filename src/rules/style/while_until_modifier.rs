use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::comments::CommentIndex;
use super::conditional::{
    Body, UpstreamParent, body_of, first_line, last_line, token, upstream_parent,
};
use super::line_length_help::LineLengthHelp;
use super::statement_modifier::non_eligible_condition;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Favor modifier `%<keyword>s` usage when having a single-line body.";

/// `body&.conditional?`, which `Style/WhileUntilModifier` adds to `StatementModifier`'s own list.
/// A ternary is an `if` upstream and counts, and so does every modifier form.
const CONDITIONALS: &[&str] = &[
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
    "case",
    "case_match",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        length: LineLengthHelp::new(context),
        comments: CommentIndex::new(context),
    };
    for node in context.nodes_of_any(&["while", "until"]) {
        let Some(loop_node) = Loop::new(node) else {
            continue;
        };
        if !cop.single_line_as_modifier(&loop_node) {
            continue;
        }
        let keyword = context.source.node_text(loop_node.keyword);
        offenses.push(
            context
                .offense(
                    MSG.replace("%<keyword>s", keyword),
                    loop_node.keyword.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: cop.to_modifier_form(&loop_node),
                    safe: true,
                }),
        );
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    length: LineLengthHelp<'a, 'tree>,
    comments: CommentIndex,
}

/// One `while`/`until` in block form, read the way upstream's `WhileNode` presents it.
struct Loop<'t> {
    node: Node<'t>,
    keyword: Node<'t>,
    condition: Node<'t>,
    end: Node<'t>,
    body: Body<'t>,
}

impl<'t> Loop<'t> {
    fn new(node: Node<'t>) -> Option<Self> {
        let keyword = token(node, &["while", "until"])?;
        let condition = node.field("condition")?;
        let container = node.field("body")?;
        Some(Self {
            node,
            keyword,
            condition,
            // The grammar keeps the loop's `end` inside the `do` clause rather than beside it.
            end: token(container, &["end"])?,
            body: body_of(container),
        })
    }
}

impl Cop<'_, '_> {
    fn source(&self, node: Node<'_>) -> &str {
        self.context.source.node_text(node)
    }

    fn single_line_as_modifier(&self, loop_node: &Loop<'_>) -> bool {
        if self.non_eligible_node(loop_node)
            || self.non_eligible_body(loop_node)
            || non_eligible_condition(loop_node.condition)
        {
            return false;
        }
        self.modifier_fits_on_single_line(loop_node)
    }

    fn non_eligible_node(&self, loop_node: &Loop<'_>) -> bool {
        nonempty_line_count(self.source(loop_node.node)) > 3
            || self.comments.on_line(last_line(loop_node.node))
            || (self.first_line_comment(loop_node.node).is_some()
                && self.code_after(loop_node).is_some())
    }

    fn non_eligible_body(&self, loop_node: &Loop<'_>) -> bool {
        match loop_node.body {
            Body::Missing | Body::Begin(_) => true,
            Body::One(body) => {
                CONDITIONALS.contains(&body.kind_str())
                    || body.byte_range().is_empty()
                    || self
                        .comments
                        .in_lines(first_line(body)..last_line(body) + 1)
            }
        }
    }

    /// `modifier_fits_on_single_line?`: the line the correction would write is put to
    /// `Layout/LineLength` exactly as an existing line is, exemptions included.
    fn modifier_fits_on_single_line(&self, loop_node: &Loop<'_>) -> bool {
        self.length.acceptable_line_length(
            &self.line_in_modifier_form(loop_node),
            first_line(loop_node.node),
        )
    }

    fn line_in_modifier_form(&self, loop_node: &Loop<'_>) -> String {
        let line = self.context.source.line(first_line(loop_node.keyword));
        let column = loop_node.keyword.start_position().column;
        let before: String = line.chars().take(column).collect();
        format!(
            "{before}{}{}",
            self.to_modifier_form(loop_node),
            self.code_after(loop_node).unwrap_or_default()
        )
    }

    /// `to_modifier_form`: the body, the keyword, the condition, wrapped and commented as the
    /// surroundings need.
    fn to_modifier_form(&self, loop_node: &Loop<'_>) -> String {
        let mut expression = format!(
            "{} {} {}",
            self.body_source(loop_node),
            self.source(loop_node.keyword),
            self.source(loop_node.condition)
        );
        if self.parenthesize(loop_node) {
            expression = format!("({expression})");
        }
        match self.first_line_comment(loop_node.node) {
            Some(comment) => format!("{expression} {}", &self.context.source.text()[comment]),
            None => expression,
        }
    }

    /// `if_body_source`: a call whose last argument ends in an omitted hash value needs its
    /// parentheses back, or the modifier keyword would be read as part of that value.
    fn body_source(&self, loop_node: &Loop<'_>) -> String {
        let Some(body) = loop_node.body.single() else {
            return String::new();
        };
        match self.omitted_value_call(body) {
            Some(rewritten) => rewritten,
            None => self.source(body).to_owned(),
        }
    }

    fn omitted_value_call(&self, body: Node<'_>) -> Option<String> {
        if !matches!(body.kind_str(), "call" | "method_call") {
            return None;
        }
        let list = body.field("arguments")?;
        let arguments = super::nodes::children(list);
        let last = arguments.last()?;
        // `foo(bar:)` parks the pairs straight in the argument list, so the omitted value shows up
        // as a pair without one.
        let omitted = match last.kind_str() {
            "pair" => last.field("value").is_none(),
            "hash" => super::nodes::children(*last)
                .last()
                .is_some_and(|pair| pair.field("value").is_none()),
            _ => false,
        };
        if !omitted {
            return None;
        }
        let selector = body.field("method")?;
        let head = &self.context.source.text()[body.start_byte()..selector.end_byte()];
        let joined = arguments
            .iter()
            .map(|argument| self.source(*argument))
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("{head}({joined})"))
    }

    /// `parenthesize?`: a modifier keyword binds so loosely that a loop standing inside a larger
    /// expression has to keep parentheses around it.
    fn parenthesize(&self, loop_node: &Loop<'_>) -> bool {
        match upstream_parent(loop_node.node) {
            Some(UpstreamParent::Node(parent)) => match parent.kind_str() {
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
                    .field("operator")
                    .is_some_and(|operator| self.source(operator) != "defined?"),
                _ => false,
            },
            _ => false,
        }
    }

    /// `first_line_comment`: the comment closing the loop's own line, which the modifier form
    /// carries along -- unless it is the directive switching this cop off.
    fn first_line_comment(&self, node: Node<'_>) -> Option<Range<usize>> {
        let comment = self.comments.at_line(first_line(node))?;
        let text = &self.context.source.text()[comment.clone()];
        (!DISABLES_COP.is_match(text)).then_some(comment)
    }

    /// `code_after`: whatever follows the `end` on its line, which the modifier form has to keep.
    fn code_after(&self, loop_node: &Loop<'_>) -> Option<String> {
        let line = self.context.source.line(last_line(loop_node.end));
        let line = crate::rules::support::chomp(line);
        let column = loop_node.end.end_position().column;
        let code: String = line.chars().skip(column).collect();
        (!code.is_empty()).then_some(code)
    }
}

fn nonempty_line_count(source: &str) -> usize {
    source
        .lines()
        .filter(|line| line.contains(|character: char| !character.is_whitespace()))
        .count()
}

/// `comment_disables_cop?`, whose `([^,],)*` only ever matches one-character names and so leaves a
/// directive listing this cop after another one alone.
static DISABLES_COP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"#(?-u:\s)*rubocop(?-u:\s)*:(?-u:\s)*(?:disable|todo)(?-u:\s)*(?:[^,],)*(?-u:\s)*(?:all|Style/WhileUntilModifier)",
    )
    .unwrap()
});
