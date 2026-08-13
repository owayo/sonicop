//! A loop whose condition never changes is `Kernel#loop`.
//!
//! Not every one of them can be rewritten. A name first assigned inside a `while` body is in scope
//! below the loop, while one first assigned inside a block is not, so a loop whose body introduces
//! a variable that is read afterwards is left as it stands -- which is a question only the file's
//! whole variable table can answer.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::{LocalVariables, VariableSpans};

const MSG: &str = "Use `Kernel#loop` for infinite loops.";

/// `TRUTHY_LITERALS`, as the node kinds that spell them. A `regopt` never stands on its own, and a
/// range is one node here whether it was written with two dots or three.
const TRUTHY_LITERAL_KINDS: &[&str] = &[
    "string",
    "chained_string",
    "character",
    "heredoc_beginning",
    "subshell",
    "integer",
    "float",
    "rational",
    "complex",
    "simple_symbol",
    "delimited_symbol",
    "array",
    "hash",
    "regex",
    "range",
    "true",
];

/// `FALSEY_LITERALS`.
const FALSEY_LITERAL_KINDS: &[&str] = &["false", "nil"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    let mut variables: Option<Vec<VariableSpans>> = None;
    for node in context.nodes_of_any(&["while", "until", "while_modifier", "until_modifier"]) {
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        let kinds = match node.kind() {
            "while" | "while_modifier" => TRUTHY_LITERAL_KINDS,
            _ => FALSEY_LITERAL_KINDS,
        };
        if !literal(condition, context, kinds) {
            continue;
        }
        let range = node.byte_range();
        let variables = variables.get_or_insert_with(|| locals.variable_spans());
        if variables
            .iter()
            .any(|variable| outlives_loop(variable, &range))
        {
            continue;
        }
        let Some(keyword) = keyword_token(node) else {
            continue;
        };
        let mut offense = context.offense(MSG, keyword.byte_range());
        if let Some(edits) = correct(context, node) {
            offense = offense.corrected_by_all(edits);
        }
        offenses.push(offense);
    }
}

/// Whether the condition is one of the literals whose truth the parser already knows.
fn literal(node: Node<'_>, context: &RuleContext<'_>, kinds: &[&str]) -> bool {
    if kinds.contains(&node.kind()) {
        return true;
    }
    // A sign written against a numeric literal is folded into the literal upstream, so `-1` is one
    // `int` there rather than a call on another node.
    node.kind() == "unary"
        && kinds.contains(&"integer")
        && node
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "-" | "+"))
        && node.child_by_field_name("operand").is_some_and(|operand| {
            matches!(operand.kind(), "integer" | "float" | "rational" | "complex")
        })
}

/// A variable the loop body introduces and something below the loop still reads. Rewriting the loop
/// as a block would put that name out of reach.
fn outlives_loop(variable: &VariableSpans, range: &Range<usize>) -> bool {
    let assigned_inside = variable
        .assignments
        .iter()
        .any(|assignment| contains(range, assignment));
    let assigned_before = variable
        .assignments
        .iter()
        .any(|assignment| assignment.end < range.start);
    let referenced_after = variable
        .references
        .iter()
        .any(|reference| reference.start > range.end);
    assigned_inside && !assigned_before && referenced_after
}

/// `Parser::Source::Range#contains?`, which holds only for a range strictly smaller at one end.
fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    inner.start >= outer.start && inner.end <= outer.end && inner != outer
}

fn correct(context: &RuleContext<'_>, node: Node<'_>) -> Option<Vec<Edit>> {
    let body = node.child_by_field_name("body")?;
    if !matches!(node.kind(), "while_modifier" | "until_modifier") {
        // `non_modifier_range`: the keyword, the condition and the `do` that may follow it.
        let end = match body.child(0) {
            Some(first) if !first.is_named() && context.source.node_text(first) == "do" => {
                first.end_byte()
            }
            _ => node.child_by_field_name("condition")?.end_byte(),
        };
        return Some(vec![Edit {
            start: node.start_byte(),
            end,
            replacement: "loop do".to_owned(),
            safe: false,
        }]);
    }
    // `post_condition_loop?`: a `begin ... end` body runs once before the condition is read.
    if body.kind() == "begin" {
        let last = u32::try_from(body.child_count()).ok()?.checked_sub(1)?;
        let (open, close) = (body.child(0)?, body.child(last)?);
        if open.kind() != "begin" || close.kind() != "end" {
            return None;
        }
        return Some(vec![
            Edit {
                start: open.start_byte(),
                end: open.end_byte(),
                replacement: "loop do".to_owned(),
                safe: false,
            },
            Edit {
                start: close.end_byte(),
                end: node.end_byte(),
                replacement: String::new(),
                safe: false,
            },
        ]);
    }
    Some(vec![Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: modifier_replacement(context, node, body),
        safe: false,
    }])
}

/// `modifier_replacement`: `foo while true` becomes a one-line block, and a body spanning lines
/// becomes a `loop do` indented one step past the loop itself.
fn modifier_replacement(context: &RuleContext<'_>, node: Node<'_>, body: Node<'_>) -> String {
    let source = context.source;
    let text = source.node_text(body);
    let (first_line, _) = source.line_column(node.start_byte());
    let (last_line, _) = source.line_column(node.end_byte());
    if first_line == last_line {
        return format!("loop {{ {text} }}");
    }
    // `indentation(node)` offsets by the *loop's* column, while the leading whitespace that joins
    // the three parts comes from the line the body begins on.
    let (_, column) = source.line_column(node.start_byte());
    let (body_line, _) = source.line_column(body.start_byte());
    let line = source.line(body_line);
    let outer = &line[..line.len() - line.trim_start().len()];
    let width: usize = context
        .setting("IndentationWidth")
        .or_else(|| context.setting_of("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let inner = " ".repeat(column - 1 + width);
    let indented: Vec<String> = text
        .split('\n')
        .map(|line| format!("{inner}{line}"))
        .collect();
    format!("loop do\n{outer}{}\n{outer}end", indented.join("\n"),)
}

fn keyword_token<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let keyword = match node.kind() {
        "while" | "while_modifier" => "while",
        _ => "until",
    };
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && child.kind() == keyword)
}
