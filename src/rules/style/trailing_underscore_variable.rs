//! `Style/TrailingUnderscoreVariable`: a parallel assignment does not have to name what it drops.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// `DISALLOW`: the assignment targets that may be dropped, which are plain local variables and the
/// splat that collects the rest.
const DISALLOW: &[&str] = &["identifier", "rest_assignment"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_named: bool = context
        .setting("AllowNamedUnderscoreVariables")
        .unwrap_or(true);
    for node in context.nodes_of("assignment") {
        let Some(left) = node.child_by_field_name("left") else {
            continue;
        };
        if left.kind() != "left_assignment_list" {
            continue;
        }
        let cop = Cop {
            context,
            allow_named,
        };
        let mut ranges = Vec::new();
        cop.unneeded_ranges(Some(node), left, &mut ranges);
        let source = context.source.node_text(node);
        for range in ranges {
            let offset = range.start - node.start_byte();
            let mut good = source[..offset].to_owned();
            good.push_str(&source[offset + (range.end - range.start)..]);
            offenses.push(
                context
                    .offense(
                        format!(
                            "Do not use trailing `_`s in parallel assignment. Prefer `{good}`."
                        ),
                        range.clone(),
                    )
                    .corrected_by(Edit {
                        start: range.start,
                        end: range.end,
                        replacement: String::new(),
                        safe: true,
                    }),
            );
        }
    }
}

struct Cop<'a> {
    context: &'a RuleContext<'a>,
    allow_named: bool,
}

impl Cop<'_> {
    /// `unneeded_ranges`: what a nested destructuring group drops, then what this level drops.
    ///
    /// `assignment` is the whole parallel assignment while `mlhs` is the group being looked at; a
    /// nested group has no right-hand side of its own to anchor against.
    fn unneeded_ranges(
        &self,
        assignment: Option<Node<'_>>,
        mlhs: Node<'_>,
        ranges: &mut Vec<Range<usize>>,
    ) {
        let variables = super::nodes::children(mlhs);
        for variable in &variables {
            if variable.kind() == "destructured_left_assignment" {
                self.unneeded_ranges(None, *variable, ranges);
            }
        }
        if let Some(main) = self.main_offense(assignment, mlhs, &variables) {
            ranges.push(main);
        }
    }

    fn main_offense(
        &self,
        assignment: Option<Node<'_>>,
        mlhs: Node<'_>,
        variables: &[Node<'_>],
    ) -> Option<Range<usize>> {
        let first = self.first_offense(variables)?;
        // Every target was dropped, so the whole left-hand side goes.
        if first.byte_range() == variables[0].byte_range() {
            let end = match assignment {
                Some(assignment) => assignment.child_by_field_name("right")?.start_byte(),
                None => mlhs.end_byte(),
            };
            return Some(mlhs.start_byte()..end);
        }
        // A parenthesized group keeps its closing parenthesis, so the comma before the first
        // dropped name goes instead of the space before the `=`.
        if mlhs.kind() == "destructured_left_assignment" {
            return Some(first.start_byte() - 1..mlhs.end_byte() - 1);
        }
        let operator = operator(assignment?)?;
        Some(first.start_byte()..operator)
    }

    /// `find_first_offense`: the earliest of the run of droppable targets that ends the list.
    fn first_offense<'t>(&self, variables: &[Node<'t>]) -> Option<Node<'t>> {
        let mut found: Option<Node<'t>> = None;
        for variable in variables.iter().rev() {
            if !DISALLOW.contains(&variable.kind()) {
                break;
            }
            let Some(name) = self.name(*variable) else {
                break;
            };
            if (self.allow_named && name != "_") || !name.starts_with('_') {
                break;
            }
            found = Some(*variable);
        }
        let found = found?;
        // `splat_variable_before?`: `_, *rest, _` drops nothing, because what the trailing `_`
        // stands for depends on how much the splat took.
        let position = variables
            .iter()
            .position(|variable| variable.byte_range() == found.byte_range())?;
        variables[..position]
            .iter()
            .all(|variable| variable.kind() != "rest_assignment")
            .then_some(found)
    }

    /// The name a target binds, unwrapping the splat that collects the rest.
    fn name(&self, variable: Node<'_>) -> Option<&str> {
        let named = match variable.kind() {
            "rest_assignment" => super::nodes::children(variable).first().copied()?,
            _ => variable,
        };
        Some(self.context.source.node_text(named))
    }
}

/// The `=` of the assignment, which is where the left-hand side stops.
fn operator(assignment: Node<'_>) -> Option<usize> {
    let mut cursor = assignment.walk();
    assignment
        .children(&mut cursor)
        .find(|child| !child.is_named() && child.kind() == "=")
        .map(|child| child.start_byte())
}
