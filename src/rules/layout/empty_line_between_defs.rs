//! `Layout/EmptyLineBetweenDefs`.

use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::support::{heredoc_terminators, statement_groups};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let counts: Vec<i64> = context
        .setting::<Vec<i64>>("NumberOfEmptyLines")
        .or_else(|| {
            context
                .setting::<i64>("NumberOfEmptyLines")
                .map(|n| vec![n])
        })
        .filter(|counts| !counts.is_empty())
        .unwrap_or_else(|| vec![1]);
    let checker = Checker {
        context,
        minimum: counts[0],
        maximum: counts[counts.len() - 1],
        methods: context
            .setting::<bool>("EmptyLineBetweenMethodDefs")
            .unwrap_or(true),
        classes: context
            .setting::<bool>("EmptyLineBetweenClassDefs")
            .unwrap_or(true),
        modules: context
            .setting::<bool>("EmptyLineBetweenModuleDefs")
            .unwrap_or(true),
        adjacent_one_liners: context
            .setting::<bool>("AllowAdjacentOneLineDefs")
            .unwrap_or(true),
        macros: context
            .setting::<Vec<String>>("DefLikeMacros")
            .unwrap_or_default(),
        heredocs: heredoc_terminators(context),
    };
    let mut reported = HashSet::new();
    for group in statement_groups(context) {
        for pair in group.statements.windows(2) {
            if checker.is_candidate(pair[0]) && checker.is_candidate(pair[1]) {
                checker.check_defs(pair[0], pair[1], &mut reported, offenses);
            }
        }
    }
}

struct Checker<'a, 'b> {
    context: &'a RuleContext<'b>,
    minimum: i64,
    maximum: i64,
    methods: bool,
    classes: bool,
    modules: bool,
    adjacent_one_liners: bool,
    macros: Vec<String>,
    /// Every heredoc of the file as `(where its opener sits, the range of its terminator)`.
    heredocs: Vec<(usize, Range<usize>)>,
}

impl Checker<'_, '_> {
    fn check_defs(
        &self,
        previous: Node<'_>,
        node: Node<'_>,
        reported: &mut HashSet<(usize, usize)>,
        offenses: &mut Vec<Offense>,
    ) {
        let between = self.lines_between_defs(previous, node);
        let count = between.iter().filter(|blank| **blank).count() as i64;
        if (self.minimum..=self.maximum).contains(&count) {
            return;
        }
        if multiple_blank_lines_groups(&between) {
            return;
        }
        if self.adjacent_one_liners && is_single_line(previous) && is_single_line(node) {
            return;
        }
        let range = self.def_location(node);
        if !reported.insert((range.start, range.end)) {
            return;
        }
        let message = format!(
            "Expected {} between {} definitions; found {count}.",
            self.expected_lines(),
            node_type(node),
        );
        let mut offense = self.context.offense(message, range);
        if let Some((edit, anchor)) = self.correction(previous, node, count) {
            offense = offense.corrected_by(edit);
            if let Some(anchor) = anchor {
                offense = offense.corrections_anchored_at(anchor);
            }
        }
        offenses.push(offense);
    }

    /// `autocorrect`: the blank lines are added or dropped at the first newline after the previous
    /// definition ends, unless the two share a line, where the newline sought lies past the second
    /// definition and the insertion moves back in front of it.
    fn correction(
        &self,
        previous: Node<'_>,
        node: Node<'_>,
        count: i64,
    ) -> Option<(Edit, Option<Range<usize>>)> {
        let text = self.context.source.text();
        let (_, end_pos) = self.end_loc(previous);
        let mut newline = text[end_pos..].find('\n')? + end_pos;
        let begin = node.start_byte();
        if newline > begin {
            newline = begin.checked_sub(1)?;
        }
        if count > self.maximum {
            let width = usize::try_from(count - self.maximum).unwrap_or(0);
            return Some((
                Edit {
                    start: newline,
                    end: (newline + width).min(text.len()),
                    replacement: String::new(),
                    safe: true,
                },
                None,
            ));
        }
        let width = usize::try_from(self.minimum - count).unwrap_or(0);
        let anchor = newline..(newline + 1).min(text.len());
        Some((
            Edit {
                start: anchor.end,
                end: anchor.end,
                replacement: "\n".repeat(width),
                safe: true,
            },
            Some(anchor),
        ))
    }

    fn expected_lines(&self) -> String {
        if self.minimum != self.maximum {
            return format!("{}..{} empty lines", self.minimum, self.maximum);
        }
        let unit = if self.maximum == 1 { "line" } else { "lines" };
        format!("{} empty {unit}", self.maximum)
    }

    /// `lines_between_defs`, reduced to whether each line is blank.
    fn lines_between_defs(&self, previous: Node<'_>, node: Node<'_>) -> Vec<bool> {
        let first = self.end_loc(previous).0 + 1;
        let last = self.context.source.line_column(node.start_byte()).0;
        if last < 2 {
            return Vec::new();
        }
        (first..last)
            .map(|line| self.context.source.line(line).trim().is_empty())
            .collect()
    }

    /// `def_location`: a definition is named by its keyword and its name, while a macro is named
    /// whole.
    fn def_location(&self, node: Node<'_>) -> Range<usize> {
        match node.kind_str() {
            "method" | "singleton_method" | "class" | "module" => match node.field("name") {
                Some(name) => node.start_byte()..name.end_byte(),
                None => node.byte_range(),
            },
            _ => node.byte_range(),
        }
    }

    /// `end_loc`: the line and offset the definition ends at, which for one trailed by a heredoc is
    /// the heredoc's terminator rather than the definition's own end.
    fn end_loc(&self, node: Node<'_>) -> (usize, usize) {
        let terminator = self
            .heredocs
            .iter()
            .filter(|(opener, _)| *opener >= node.start_byte() && *opener < node.end_byte())
            .map(|(_, terminator)| terminator)
            .max_by_key(|terminator| terminator.end);
        match terminator {
            Some(terminator) if terminator.end > node.end_byte() => (
                self.context.source.line_column(terminator.start).0,
                terminator.end,
            ),
            _ => (
                self.context.source.line_column(node.end_byte()).0,
                node.end_byte(),
            ),
        }
    }

    fn is_candidate(&self, node: Node<'_>) -> bool {
        (self.methods && matches!(node.kind_str(), "method" | "singleton_method"))
            || (self.classes && node.kind_str() == "class")
            || (self.modules && node.kind_str() == "module")
            || self.is_macro_candidate(node)
    }

    /// `macro_candidate?`: a call named by `DefLikeMacros` written where a definition would go.
    fn is_macro_candidate(&self, node: Node<'_>) -> bool {
        if self.macros.is_empty() {
            return false;
        }
        let call = match node.kind_str() {
            "call" | "method_call" | "identifier" => node,
            _ => return false,
        };
        if call.field("receiver").is_some() {
            return false;
        }
        let name = match call.kind_str() {
            "identifier" => &self.context.source.text()[call.byte_range()],
            _ => match call.field("method") {
                Some(method) => &self.context.source.text()[method.byte_range()],
                None => return false,
            },
        };
        self.macros.iter().any(|macro_name| macro_name == name)
    }
}

/// `multiple_blank_lines_groups?`: a blank line after something that is not blank means the gap
/// holds more than one run, which this cop leaves alone.
fn multiple_blank_lines_groups(between: &[bool]) -> bool {
    let last_blank = between.iter().rposition(|blank| *blank);
    let first_filled = between.iter().position(|blank| !*blank);
    match (last_blank, first_filled) {
        (Some(last_blank), Some(first_filled)) => last_blank > first_filled,
        _ => false,
    }
}

fn is_single_line(node: Node<'_>) -> bool {
    node.start_position().row == node.end_position().row
}

/// `node_type`, which names a definition after what it defines rather than after its node.
fn node_type(node: Node<'_>) -> &'static str {
    match node.kind_str() {
        "method" | "singleton_method" => "method",
        "class" => "class",
        "module" => "module",
        // A `DefLikeMacros` entry reaches upstream as the `block` wrapped around the call, and
        // `node_type` folds `numblock` / `itblock` into `block` as well. The grammar keeps the
        // block on the call, so the name has to come from whether one is written.
        "call" if node.field("block").is_some() => "block",
        _ => "send",
    }
}
