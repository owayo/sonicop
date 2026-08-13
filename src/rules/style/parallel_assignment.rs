//! `Style/ParallelAssignment`: one assignment per line, unless the values genuinely have to move
//! at once.

use std::ops::Range;

use tree_sitter::Node;

use super::conditional::descendants;
use super::literal::{Quoting, decode};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not use parallel assignment.";

/// The right-hand sides upstream reads as an `array`: the bare list of a parallel assignment and
/// every spelling of an array literal.
const RHS_LISTS: &[&str] = &[
    "right_assignment_list",
    "array",
    "string_array",
    "symbol_array",
];

/// Node kinds standing for a splat, which makes the assignment genuinely parallel.
const SPLAT_KINDS: &[&str] = &["splat_argument", "rest_assignment", "splat_parameter"];

/// Nodes whose modifier form wraps the assignment, which the correction has to open into a block.
const MODIFIER_KINDS: &[&str] = &[
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        indentation_width: context
            .setting_of("Layout/IndentationWidth", "Width")
            .unwrap_or(2),
    };
    for node in context.nodes_of("assignment") {
        cop.on_masgn(node, offenses);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    indentation_width: usize,
}

impl Cop<'_, '_> {
    fn source(&self, node: Node<'_>) -> &str {
        self.context.source.node_text(node)
    }

    fn on_masgn(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let Some(left) = node.field("left") else {
            return;
        };
        if left.kind_str() != "left_assignment_list" {
            return;
        }
        let Some(rhs) = node.field("right") else {
            return;
        };
        // A `rescue` modifier binds tighter than the assignment, so the values are its body.
        let (rhs, rescue_result) = match rhs.kind_str() {
            "rescue_modifier" => (
                match rhs.field("body") {
                    Some(body) => body,
                    None => return,
                },
                rhs.field("handler"),
            ),
            _ => (rhs, None),
        };
        let lhs_elements = assignments(left);
        let rhs_elements = match RHS_LISTS.contains(&rhs.kind_str()) {
            true => super::nodes::children(rhs),
            false => Vec::new(),
        };

        // `allowed_lhs?`, `allowed_rhs?`.
        if lhs_elements.len() <= 1
            || lhs_elements
                .iter()
                .any(|element| SPLAT_KINDS.contains(&element.kind_str()))
            || !RHS_LISTS.contains(&rhs.kind_str())
            || rhs_elements
                .iter()
                .any(|element| SPLAT_KINDS.contains(&element.kind_str()))
        {
            return;
        }
        // `allowed_masign?`.
        if lhs_elements.len() != rhs_elements.len() {
            return;
        }
        let Some(order) = self.valid_order(&lhs_elements, &rhs_elements) else {
            return;
        };
        // `contains_heredoc?`: splitting the assignment would drop the following lines into the
        // heredoc's body.
        if descendants(rhs)
            .into_iter()
            .any(|inner| inner.kind_str() == "heredoc_beginning")
        {
            return;
        }

        let offense = self.context.offense(MSG, node.start_byte()..rhs.end_byte());
        offenses.push(offense.corrected_by_all(self.autocorrect(node, &order, rescue_result)));
    }

    /// `find_valid_order`: the assignments arranged so that none reads a name a later one writes.
    fn valid_order<'t>(
        &self,
        lhs_elements: &[Node<'t>],
        rhs_elements: &[Node<'t>],
    ) -> Option<Vec<(Node<'t>, Node<'t>)>> {
        let pairs: Vec<(Node<'t>, Node<'t>)> = lhs_elements
            .iter()
            .copied()
            .zip(rhs_elements.iter().copied())
            .collect();
        // `dependencies_for_assignment`: everything that has to be written after this one.
        let mut dependencies: Vec<(usize, Vec<usize>)> = pairs
            .iter()
            .enumerate()
            .map(|(index, (lhs, _))| {
                let edges = pairs
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != index)
                    .filter(|(_, (_, other_rhs))| self.depends_on(*lhs, *other_rhs))
                    .map(|(other, _)| other)
                    .collect();
                (index, edges)
            })
            .collect();
        let mut result = Vec::new();
        while let Some(position) = dependencies.iter().position(|(_, edges)| edges.is_empty()) {
            let (matched, _) = dependencies.remove(position);
            result.push(pairs[matched]);
            for (_, edges) in &mut dependencies {
                edges.retain(|edge| *edge != matched);
            }
        }
        // A cyclic dependency is a real swap, which only parallel assignment can express.
        dependencies.is_empty().then_some(result)
    }

    /// `dependency?`: whether `rhs` reads the value `lhs` overwrites.
    fn depends_on(&self, lhs: Node<'_>, rhs: Node<'_>) -> bool {
        match lhs.kind_str() {
            "identifier" | "instance_variable" | "global_variable" | "class_variable"
            | "constant" => {
                let name = self.source(lhs);
                descendants(rhs)
                    .into_iter()
                    .any(|inner| self.reads_name(inner, name))
            }
            // `lhs.send_type? && lhs.assignment_method?`: `obj.attr` and `ary[idx]` are writes
            // through a call, so the read they shadow is the matching getter.
            "call" | "method_call" => self.accesses_getter(lhs, rhs),
            "element_reference" => self.accesses_index(lhs, rhs),
            _ => false,
        }
    }

    fn reads_name(&self, node: Node<'_>, name: &str) -> bool {
        match node.kind_str() {
            "identifier" => reads_a_variable(node) && self.source(node) == name,
            "instance_variable" | "global_variable" | "class_variable" | "constant" => {
                self.source(node) == name
            }
            _ => false,
        }
    }

    /// `accesses?` for `obj.attr=`: the reader `obj.attr` written anywhere on the right.
    fn accesses_getter(&self, lhs: Node<'_>, rhs: Node<'_>) -> bool {
        let (Some(receiver), Some(method)) = (
            lhs.field("receiver"),
            lhs.field("method"),
        ) else {
            return false;
        };
        let receiver = self.source(receiver);
        let method = self.source(method);
        descendants(rhs).into_iter().any(|inner| {
            matches!(inner.kind_str(), "call" | "method_call")
                && inner
                    .field("receiver")
                    .is_some_and(|other| self.source(other) == receiver)
                && inner
                    .field("method")
                    .is_some_and(|other| self.source(other) == method)
        })
    }

    /// `accesses?` for `ary[idx]=`: the same index read written anywhere on the right.
    fn accesses_index(&self, lhs: Node<'_>, rhs: Node<'_>) -> bool {
        let Some(object) = lhs.field("object") else {
            return false;
        };
        let object = self.source(object);
        let arguments = index_arguments(self, lhs);
        descendants(rhs).into_iter().any(|inner| {
            inner.kind_str() == "element_reference"
                && inner
                    .field("object")
                    .is_some_and(|other| self.source(other) == object)
                && index_arguments(self, inner) == arguments
        })
    }

    fn autocorrect(
        &self,
        node: Node<'_>,
        order: &[(Node<'_>, Node<'_>)],
        rescue_result: Option<Node<'_>>,
    ) -> Vec<Edit> {
        let column = node.start_position().column;
        let offset = " ".repeat(column);
        let indentation = " ".repeat(column + self.indentation_width);
        let assignments: Vec<String> = order
            .iter()
            .map(|(lhs, rhs)| format!("{} = {}", self.source(*lhs), self.element_source(*rhs)))
            .collect();

        // `ModifierCorrector`: the modifier keyword opens a block the assignments live in.
        if let Some(modifier) = node.parent().filter(|parent| {
            MODIFIER_KINDS.contains(&parent.kind_str())
                && parent
                    .field("body")
                    .is_some_and(|body| body.id() == node.id())
        }) {
            let keyword = modifier_range(modifier);
            return vec![Edit {
                start: modifier.start_byte(),
                end: modifier.end_byte(),
                replacement: format!(
                    "{}\n{indentation}{}\n{offset}end",
                    &self.context.source.text()[keyword],
                    assignments.join(&format!("\n{indentation}"))
                ),
                safe: true,
            }];
        }
        // `RescueCorrector`: the values were guarded, so the assignments need a `begin` to share.
        if let Some(result) = rescue_result {
            let Some(rescue) = node.field("right") else {
                return Vec::new();
            };
            return vec![Edit {
                start: node.start_byte(),
                end: rescue.end_byte(),
                replacement: format!(
                    "begin\n{indentation}{}\n{offset}rescue\n{indentation}{}\n{offset}end",
                    assignments.join(&format!("\n{indentation}")),
                    self.source(result)
                ),
                safe: true,
            }];
        }
        vec![Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: assignments.join(&format!("\n{offset}")),
            safe: true,
        }]
    }

    /// `GenericCorrector#source`: an element of a `%w`/`%i` literal carries no delimiter of its
    /// own, so it has to be written back out as a quoted string or an inspected symbol.
    fn element_source(&self, node: Node<'_>) -> String {
        let quoting = |array: Node<'_>| match self
            .source(array)
            .chars()
            .nth(1)
        {
            Some('W' | 'I') => Quoting::Double,
            _ => Quoting::Word,
        };
        match (node.kind_str(), node.parent()) {
            ("bare_string", Some(array)) => {
                quote(&decode(self.source(node), quoting(array), &[]).value)
            }
            ("bare_symbol", Some(array)) => {
                inspect_symbol(&decode(self.source(node), quoting(array), &[]).value)
            }
            _ => self.source(node).to_owned(),
        }
    }
}

fn index_arguments(cop: &Cop<'_, '_>, node: Node<'_>) -> Vec<String> {
    let mut arguments = super::nodes::children(node);
    if node.field("object").is_some() && !arguments.is_empty() {
        arguments.remove(0);
    }
    arguments
        .into_iter()
        .map(|argument| cop.source(argument).to_owned())
        .collect()
}

/// `modifier_range`: the keyword and the condition behind it, which becomes the head of the block.
fn modifier_range(modifier: Node<'_>) -> Range<usize> {
    let mut cursor = modifier.walk();
    let keyword = modifier
        .children(&mut cursor)
        .find(|child| !child.is_named())
        .map_or(modifier.start_byte(), |child| child.start_byte());
    keyword..modifier.end_byte()
}

/// `MlhsNode#assignments`: the names a left-hand side writes, with a splat standing for the target
/// it holds and a nested list flattened into the one around it.
fn assignments<'t>(left: Node<'t>) -> Vec<Node<'t>> {
    super::nodes::children(left)
        .into_iter()
        .flat_map(|node| match node.kind_str() {
            // An anonymous splat has nothing beneath it and stands for itself.
            kind if SPLAT_KINDS.contains(&kind) => {
                vec![
                    super::nodes::children(node)
                        .first()
                        .copied()
                        .unwrap_or(node),
                ]
            }
            "destructured_left_assignment" => assignments(node),
            _ => vec![node],
        })
        .collect()
}

/// Whether the identifier stands for a value rather than for the name of a call.
fn reads_a_variable(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    parent
        .field("method")
        .is_none_or(|method| method.id() != node.id())
}

/// `quote`: a single-quoted string with the two characters that stay special escaped.
fn quote(value: &str) -> String {
    let escaped: String = value
        .chars()
        .flat_map(|character| match character {
            '\\' | '\'' => vec!['\\', character],
            other => vec![other],
        })
        .collect();
    format!("'{escaped}'")
}

/// `Symbol#inspect`: a plain name stays bare, anything else is quoted.
fn inspect_symbol(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_')
        && !value.starts_with(|character: char| character.is_ascii_digit());
    match plain {
        true => format!(":{value}"),
        false => format!(":{}", super::literal::to_string_literal(value)),
    }
}
