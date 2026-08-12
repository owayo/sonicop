//! `Style/Next`: an iteration whose whole tail sits inside one conditional should skip instead.

use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use super::conditional::{Body, body_of, descendants, first_line, last_line, token};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Use `next` to skip iteration.";

/// `ENUMERATOR_METHODS`, which together with any `each_*` name is what makes a block an iteration.
const ENUMERATOR_METHODS: &[&str] = &[
    "collect",
    "collect_concat",
    "detect",
    "downto",
    "each",
    "find",
    "find_all",
    "find_index",
    "inject",
    "loop",
    "map!",
    "map",
    "reduce",
    "reject",
    "reject!",
    "reverse_each",
    "select",
    "select!",
    "times",
    "upto",
];

/// `EXIT_TYPES`: a body that already leaves the iteration needs no `next`.
const EXIT_TYPES: &[&str] = &["break", "return"];

/// Node kinds the grammar wraps a body in, which upstream reads through.
const BODY_CONTAINERS: &[&str] = &["do", "body_statement", "block_body", "then", "else"];

const CONDITIONAL_KINDS: &[&str] = &[
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        skip_modifier_ifs: context
            .setting::<String>("EnforcedStyle")
            .unwrap_or_else(|| "skip_modifier_ifs".to_owned())
            == "skip_modifier_ifs",
        min_body_length: context.setting("MinBodyLength").unwrap_or(3),
        allow_consecutive_conditionals: context
            .setting("AllowConsecutiveConditionals")
            .unwrap_or(false),
    };
    // `on_new_investigation`: how far each line has already been pulled left, so that a nested
    // correction moves it the rest of the way rather than the same way twice.
    let mut reindented: HashMap<usize, usize> = HashMap::new();

    for node in context.nodes_of_any(&[
        "call",
        "method_call",
        "while",
        "until",
        "for",
        "while_modifier",
        "until_modifier",
    ]) {
        let loop_body = match node.kind() {
            "call" | "method_call" => match cop.iteration_block(node) {
                Some(block) => block.child_by_field_name("body"),
                None => continue,
            },
            _ => node.child_by_field_name("body"),
        };
        let Some(loop_body) = loop_body else {
            continue;
        };
        cop.check(loop_body, &mut reindented, offenses);
    }
}

struct Cop<'a> {
    context: &'a RuleContext<'a>,
    skip_modifier_ifs: bool,
    min_body_length: usize,
    allow_consecutive_conditionals: bool,
}

impl Cop<'_> {
    fn source(&self, node: Node<'_>) -> &str {
        self.context.source.node_text(node)
    }

    /// `node.send_node.call_type? && node.send_node.enumerator_method?`.
    fn iteration_block<'t>(&self, node: Node<'t>) -> Option<Node<'t>> {
        let block = node.child_by_field_name("block")?;
        let method = node.child_by_field_name("method")?;
        let name = self.source(method);
        (ENUMERATOR_METHODS.contains(&name) || name.starts_with("each_")).then_some(block)
    }

    fn check(
        &self,
        loop_body: Node<'_>,
        reindented: &mut HashMap<usize, usize>,
        offenses: &mut Vec<Offense>,
    ) {
        let body = body_of_loop(loop_body);
        let Some(last) = body.last() else {
            return;
        };
        if !self.simple_if_without_break(last) {
            return;
        }
        if self.allow_consecutive_conditionals && consecutive_conditionals(&body, last) {
            return;
        }
        let Some(condition) = last.child_by_field_name("condition") else {
            return;
        };
        let offense = self
            .context
            .offense(MSG, last.start_byte()..condition.end_byte());
        offenses.push(offense.corrected_by_all(self.autocorrect(last, condition, reindented)));
    }

    /// `simple_if_without_break?`.
    fn simple_if_without_break(&self, node: Node<'_>) -> bool {
        let modifier = matches!(node.kind(), "if_modifier" | "unless_modifier");
        if !modifier && !matches!(node.kind(), "if" | "unless") {
            return false;
        }
        if node.child_by_field_name("alternative").is_some() {
            return false;
        }
        if self.if_else_children(node) {
            return false;
        }
        // `allowed_modifier_if?`: the default style leaves modifier forms alone, and a block form
        // has to be tall enough to be worth unwrapping.
        let allowed = match modifier {
            true => self.skip_modifier_ifs,
            false => !self.min_body_length(node),
        };
        if allowed {
            return false;
        }
        !self.exit_body(node)
    }

    /// `if_else_children?`: one of the node's own children is a conditional carrying an `else`.
    fn if_else_children(&self, node: Node<'_>) -> bool {
        let body = node
            .child_by_field_name("consequence")
            .map(body_of)
            .or_else(|| node.child_by_field_name("body").map(Body::One));
        [
            node.child_by_field_name("condition"),
            body.and_then(|body| body.single()),
        ]
        .into_iter()
        .flatten()
        .any(|child| {
            matches!(child.kind(), "if" | "unless")
                && child.child_by_field_name("alternative").is_some()
        })
    }

    fn min_body_length(&self, node: Node<'_>) -> bool {
        let Some(end) = token(node, &["end"]) else {
            return false;
        };
        first_line(end).saturating_sub(first_line(node)) > self.min_body_length
    }

    /// `exit_body_type?`: the branch already leaves the iteration.
    fn exit_body(&self, node: Node<'_>) -> bool {
        let branch = match node.child_by_field_name("consequence") {
            Some(consequence) => body_of(consequence).single(),
            None => node.child_by_field_name("body"),
        };
        branch.is_some_and(|branch| EXIT_TYPES.contains(&branch.kind()))
    }

    fn autocorrect(
        &self,
        node: Node<'_>,
        condition: Node<'_>,
        reindented: &mut HashMap<usize, usize>,
    ) -> Vec<Edit> {
        let inverse = inverse_keyword(node);
        let condition_source = self.source(condition);
        if matches!(node.kind(), "if_modifier" | "unless_modifier") {
            let Some(body) = node.child_by_field_name("body") else {
                return Vec::new();
            };
            let indent = " ".repeat(node.start_position().column);
            return vec![Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!(
                    "next {inverse} {condition_source}\n{indent}{}",
                    self.source(body)
                ),
                safe: true,
            }];
        }
        let Some(end) = token(node, &["end"]) else {
            return Vec::new();
        };
        let mut edits = vec![Edit {
            start: node.start_byte(),
            end: node.start_byte(),
            replacement: format!("next {inverse} {condition_source}"),
            safe: true,
        }];
        edits.push(remove(
            node.start_byte()..self.condition_end(node, condition),
        ));
        edits.push(remove(self.end_range(end)));
        edits.extend(self.reindent(node, condition, reindented));
        edits
    }

    /// `cond_range`: everything up to and including the `then`, or up to the condition when the
    /// keyword is not written.
    fn condition_end(&self, node: Node<'_>, condition: Node<'_>) -> usize {
        node.child_by_field_name("consequence")
            .and_then(|consequence| token(consequence, &["then"]))
            .map_or(condition.end_byte(), |then| then.end_byte())
    }

    /// `end_range`: the `end` with its own indentation, and the newline in front of it when
    /// nothing else shares its line.
    fn end_range(&self, end: Node<'_>) -> Range<usize> {
        let source = self.context.source;
        let mut start = source.line_start(first_line(end));
        // `/\A\s*$/` matches whenever the `end` closes its line.
        if source.text()[end.end_byte()..]
            .chars()
            .take_while(|character| *character != '\n')
            .all(char::is_whitespace)
            && start > 0
        {
            start -= 1;
        }
        start..end.end_byte()
    }

    /// `reindent`: the body moves left by as much as the conditional itself is indented.
    fn reindent(
        &self,
        node: Node<'_>,
        condition: Node<'_>,
        reindented: &mut HashMap<usize, usize>,
    ) -> Vec<Edit> {
        let source = self.context.source;
        let lines = self.reindentable_lines(node);
        if lines.is_empty() {
            return Vec::new();
        }
        let Some(target) = indentation(source.line(first_line(condition))) else {
            return Vec::new();
        };
        let Some(actual) = lines
            .iter()
            .filter_map(|line| indentation(source.line(*line)))
            .min()
        else {
            return Vec::new();
        };
        let delta = actual.saturating_sub(target);
        lines
            .into_iter()
            .filter_map(|line| {
                let adjustment = delta + reindented.get(&line).copied().unwrap_or(0);
                reindented.insert(line, adjustment);
                let start = source.line_start(line);
                (adjustment > 0).then(|| remove(start..start + adjustment))
            })
            .collect()
    }

    /// `reindentable_lines`: the body's own lines, leaving out the blank ones and everything a
    /// heredoc owns.
    fn reindentable_lines(&self, node: Node<'_>) -> Vec<usize> {
        let Some(end) = token(node, &["end"]) else {
            return Vec::new();
        };
        let heredocs = self.heredoc_lines(node);
        (first_line(node) + 1..last_line(end))
            .filter(|line| !heredocs.contains(line))
            .filter(|line| !self.context.source.line(*line).trim().is_empty())
            .collect()
    }

    /// `heredoc_lines`: `(body.line...body.last_line)` of every heredoc the node opens.
    fn heredoc_lines(&self, node: Node<'_>) -> Vec<usize> {
        let beginnings: Vec<usize> = descendants(node)
            .into_iter()
            .filter(|inner| inner.kind() == "heredoc_beginning")
            .map(|inner| inner.start_byte())
            .collect();
        if beginnings.is_empty() {
            return Vec::new();
        }
        let all: Vec<usize> = self
            .context
            .nodes_of("heredoc_beginning")
            .map(|inner| inner.start_byte())
            .collect();
        let bodies: Vec<Node<'_>> = self.context.nodes_of("heredoc_body").collect();
        let mut lines = Vec::new();
        for start in beginnings {
            let Some(index) = all.iter().position(|other| *other == start) else {
                continue;
            };
            let Some(body) = bodies.get(index) else {
                continue;
            };
            // The grammar hangs the newline that closed the opener's line off the front of the
            // body, where upstream's `heredoc_body` starts on the line after it.
            let first = self.context.source.line_column(body.start_byte()).0 + 1;
            lines.extend(first..last_line(*body));
        }
        lines
    }
}

/// The body a loop or block holds, read through the wrapper the grammar puts around it.
fn body_of_loop<'t>(body: Node<'t>) -> Body<'t> {
    match BODY_CONTAINERS.contains(&body.kind()) {
        true => body_of(body),
        false => Body::One(body),
    }
}

/// `consecutive_conditionals?`: the statement before this one is a conditional too.
fn consecutive_conditionals(body: &Body<'_>, node: Node<'_>) -> bool {
    let Body::Begin(statements) = body else {
        return false;
    };
    let Some(index) = statements
        .iter()
        .position(|statement| statement.id() == node.id())
    else {
        return false;
    };
    index > 0 && CONDITIONAL_KINDS.contains(&statements[index - 1].kind())
}

fn inverse_keyword(node: Node<'_>) -> &'static str {
    match node.kind() {
        "unless" | "unless_modifier" => "if",
        _ => "unless",
    }
}

fn indentation(line: &str) -> Option<usize> {
    line.find(|character: char| !character.is_whitespace())
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}
