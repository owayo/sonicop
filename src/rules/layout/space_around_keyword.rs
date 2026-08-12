//! `Layout/SpaceAroundKeyword`.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::support::is_send_like;

/// `ACCEPT_LEFT_PAREN`: keywords a `(` may follow without a space.
const ACCEPT_LEFT_PAREN: [&str; 7] = [
    "break", "defined?", "next", "not", "rescue", "super", "yield",
];
/// `ACCEPT_LEFT_SQUARE_BRACKET`.
const ACCEPT_LEFT_SQUARE_BRACKET: [&str; 2] = ["super", "yield"];

/// Every node kind carrying one of the keywords this cop inspects.
const KINDS: [&str; 31] = [
    "begin",
    "begin_block",
    "binary",
    "block",
    "break",
    "case",
    "case_match",
    "do_block",
    "elsif",
    "end_block",
    "ensure",
    "for",
    "if",
    "if_guard",
    "if_modifier",
    "in_clause",
    "next",
    "rescue",
    "return",
    "super",
    "test_pattern",
    "unary",
    "unless",
    "unless_guard",
    "unless_modifier",
    "until",
    "until_modifier",
    "when",
    "while",
    "while_modifier",
    "yield",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut reporter = Reporter {
        context,
        reported: HashSet::new(),
    };
    // A second report on the same range is dropped by `add_offense`'s own set, so the nodes have to
    // be visited in the order upstream's callbacks fire, which is source order.
    for node in context.nodes_of_any(&KINDS) {
        reporter.inspect(node, offenses);
    }
}

struct Reporter<'a, 'b> {
    context: &'a RuleContext<'b>,
    reported: HashSet<(usize, usize)>,
}

impl Reporter<'_, '_> {
    fn inspect(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        match node.kind() {
            // `on_and` / `on_or`, which only fire for the word forms.
            "binary" => {
                if let Some(operator) = node
                    .child_by_field_name("operator")
                    .filter(|operator| matches!(operator.kind(), "and" | "or"))
                {
                    self.keyword(node, operator, offenses);
                }
            }
            // `on_send` for `prefix_not?`, and `on_defined?`.
            "unary" => {
                if let Some(operator) = node
                    .child_by_field_name("operator")
                    .filter(|operator| matches!(operator.kind(), "not" | "defined?"))
                {
                    self.keyword(node, operator, offenses);
                }
            }
            "block" | "do_block" => {
                let Some(open) = token(node, &["{", "do"]).filter(|open| open.kind() == "do")
                else {
                    return;
                };
                self.keyword(node, open, offenses);
                if let Some(end) = token(node, &["}", "end"]) {
                    self.end_keyword(end, offenses);
                }
            }
            "break" | "next" | "return" | "when" | "ensure" | "super" | "yield" => {
                if let Some(keyword) = node.child(0).filter(|child| !child.is_named()) {
                    self.keyword(node, keyword, offenses);
                }
            }
            "rescue" => {
                if let Some(keyword) = node.child(0).filter(|child| !child.is_named()) {
                    self.keyword(node, keyword, offenses);
                }
                // `on_rescue` checks the `else` upstream hangs off the rescue construct.
                if let Some(other) = rescue_else(node) {
                    self.keyword(node, other, offenses);
                }
            }
            "case" | "case_match" => {
                if let Some(keyword) = token(node, &["case"]) {
                    self.keyword(node, keyword, offenses);
                }
                if let Some(other) = case_else(node) {
                    self.keyword(node, other, offenses);
                }
            }
            // `on_for` looks at the delimiters only; `on_while` and `on_until` at the keyword too.
            "for" | "while" | "until" => {
                let body = node.child_by_field_name("body");
                // The grammar keeps both `do` and `end` inside the loop's body node.
                if let Some(open) = body.and_then(|body| token(body, &["do"])) {
                    self.keyword(node, open, offenses);
                    if let Some(end) = body.and_then(|body| token(body, &["end"])) {
                        self.end_keyword(end, offenses);
                    }
                }
                if node.kind() != "for" {
                    if let Some(keyword) = node.child(0).filter(|child| !child.is_named()) {
                        self.keyword(node, keyword, offenses);
                    }
                }
            }
            "while_modifier" | "until_modifier" | "if_modifier" | "unless_modifier"
            | "if_guard" | "unless_guard" => {
                if let Some(keyword) = token(node, &["while", "until", "if", "unless"]) {
                    self.keyword(node, keyword, offenses);
                }
            }
            "if" | "unless" | "elsif" => {
                if let Some(keyword) = token(node, &["if", "unless", "elsif"]) {
                    self.keyword(node, keyword, offenses);
                }
                if let Some(other) = node
                    .child_by_field_name("alternative")
                    .and_then(|other| token(other, &["else", "elsif"]))
                {
                    self.keyword(node, other, offenses);
                }
                if let Some(then) = node
                    .child_by_field_name("consequence")
                    .and_then(|consequence| token(consequence, &["then"]))
                {
                    self.keyword(node, then, offenses);
                }
                if let Some(end) = token(node, &["end"]) {
                    self.end_keyword(end, offenses);
                }
            }
            // `on_kwbegin`, whose `begin` is checked whatever it reads.
            "begin" => {
                if let Some(keyword) = token(node, &["begin"]) {
                    self.keyword(node, keyword, offenses);
                }
                if let Some(end) = token(node, &["end"]) {
                    self.end_keyword(end, offenses);
                }
            }
            "in_clause" | "test_pattern" => {
                if let Some(keyword) = token(node, &["in"]) {
                    self.keyword(node, keyword, offenses);
                }
            }
            "begin_block" | "end_block" => {
                if let Some(keyword) = token(node, &["BEGIN", "END"]) {
                    self.keyword(node, keyword, offenses);
                }
            }
            _ => {}
        }
    }

    /// `check_keyword`: the blank on each side of the keyword. Both halves report the same range,
    /// so the second one only ever reaches the set that drops it.
    fn keyword(&mut self, node: Node<'_>, range: Node<'_>, offenses: &mut Vec<Offense>) {
        let mut recorded = false;
        if self.space_before_missing(range) && !self.preceded_by_operator(node) {
            recorded = self.report(range, true, offenses);
        }
        if !recorded && self.space_after_missing(range) {
            self.report(range, false, offenses);
        }
    }

    /// `check_end`, which asks about the blank before the keyword only and skips the operator test.
    fn end_keyword(&mut self, range: Node<'_>, offenses: &mut Vec<Offense>) {
        if self.space_before_missing(range) {
            self.report(range, true, offenses);
        }
    }

    /// Whether the range now counts as reported, whether by this call or an earlier one.
    fn report(&mut self, range: Node<'_>, before: bool, offenses: &mut Vec<Offense>) -> bool {
        let span = range.byte_range();
        if !self.reported.insert((span.start, span.end)) {
            return true;
        }
        let source = self.context.source.node_text(range);
        let message = if before {
            format!("Space before keyword `{source}` is missing.")
        } else {
            format!("Space after keyword `{source}` is missing.")
        };
        let offset = if before { span.start } else { span.end };
        offenses.push(self.context.offense(message, span).corrected_by(Edit {
            start: offset,
            end: offset,
            replacement: " ".to_owned(),
            safe: true,
        }));
        true
    }

    /// `space_before_missing?`
    fn space_before_missing(&self, range: Node<'_>) -> bool {
        let start = range.start_byte();
        if start == 0 {
            return false;
        }
        let byte = self.context.source.text().as_bytes()[start - 1];
        !(byte.is_ascii_whitespace() || b"(|{[;,*=".contains(&byte))
    }

    /// `space_after_missing?`
    fn space_after_missing(&self, range: Node<'_>) -> bool {
        let text = self.context.source.text();
        let source = self.context.source.node_text(range);
        let Some(&byte) = text.as_bytes().get(range.end_byte()) else {
            return false;
        };
        if (byte == b'[' && ACCEPT_LEFT_SQUARE_BRACKET.contains(&source))
            || (byte == b'(' && ACCEPT_LEFT_PAREN.contains(&source))
        {
            return false;
        }
        let rest = &text[range.end_byte()..];
        if rest.starts_with("&.") || (source == "super" && rest.starts_with("::")) {
            return false;
        }
        !(byte.is_ascii_whitespace() || b";,#\\)}].".contains(&byte))
    }

    /// `preceded_by_operator?`: an operator binds the keyword tightly enough that the blank in
    /// front of it belongs to the operator cop.
    fn preceded_by_operator(&self, node: Node<'_>) -> bool {
        let mut current = node;
        while let Some(parent) = current.parent() {
            if parent.kind() == "range" {
                return true;
            }
            if parent.kind() == "binary"
                && parent
                    .child_by_field_name("operator")
                    .is_some_and(|operator| matches!(operator.kind(), "and" | "or" | "&&" | "||"))
            {
                return true;
            }
            if !is_send_like(self.context, parent) {
                return false;
            }
            if operator_method(self.context, parent) {
                return true;
            }
            current = parent;
        }
        false
    }
}

/// `node.operator_method?`
fn operator_method(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "binary" | "unary" | "element_reference" => true,
        _ => node.child_by_field_name("method").is_some_and(|method| {
            !context
                .source
                .node_text(method)
                .starts_with(|character: char| character.is_alphabetic() || character == '_')
        }),
    }
}

/// `case.loc.else`. The grammar names the field for a pattern-matching `case` and not for the
/// ordinary one.
fn case_else<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let branch = node.child_by_field_name("else").or_else(|| {
        node.named_children(&mut cursor)
            .find(|child| child.kind() == "else")
    })?;
    token(branch, &["else"])
}

/// The `else` of a `begin ... rescue ... else ... end`, which upstream hangs off the rescue node.
fn rescue_else<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut sibling = node.next_named_sibling();
    while let Some(candidate) = sibling {
        match candidate.kind() {
            "else" => return token(candidate, &["else"]),
            "rescue" => sibling = candidate.next_named_sibling(),
            _ => return None,
        }
    }
    None
}

/// A keyword token written directly under `node`. Only unnamed children qualify: a nested `if` is a
/// node of that name and never the keyword itself.
fn token<'tree>(node: Node<'tree>, kinds: &[&str]) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && kinds.contains(&child.kind()))
}
