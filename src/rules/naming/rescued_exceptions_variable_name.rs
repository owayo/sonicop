use std::ops::Range;

use tree_sitter::Node;

use super::support::last_named_child;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::variable_force::Analysis;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::spurious_assignment_list;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let configured: String = context
        .setting("PreferredName")
        .unwrap_or_else(|| "e".to_owned());
    let mut variables = None;

    for node in context.nodes_of("rescue") {
        // A nested rescue keeps its own name: renaming it could shadow the variable the outer one
        // bound.
        if ancestors(node).any(|ancestor| ancestor.kind_str() == "rescue") {
            continue;
        }
        let Some((name_node, name)) = exception_variable(context, node) else {
            continue;
        };
        // A name already marked unused keeps that mark, so `_err` is asked to become `_e`.
        let preferred = if name.starts_with('_') {
            format!("_{configured}")
        } else {
            configured.clone()
        };
        if name == preferred {
            continue;
        }
        let variables = variables.get_or_insert_with(|| context.variable_analysis());
        // `shadowed_variable_name?` asks whether the *configured* name is already read inside the
        // handler. Upstream passes a node where a name is expected, so the underscore prefix never
        // reaches this test.
        if reads_name(context, variables, node, &configured) {
            continue;
        }
        let range = name_node.byte_range();
        offenses.push(
            context
                .offense(
                    format!("Use `{preferred}` instead of `{name}`."),
                    range.clone(),
                )
                .corrected_by_all(rename(context, variables, node, range, &name, &preferred)),
        );
    }
}

/// The exception variable's node and the name the parser reads off it. A `send` target --
/// `rescue => obj.attr` -- answers to no name and is left alone.
fn exception_variable<'tree>(
    context: &RuleContext<'tree>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, String)> {
    let variable = node.field("variable")?;
    let mut cursor = variable.walk();
    let target = variable.named_children(&mut cursor).next()?;
    let name = match target.kind_str() {
        "identifier" | "instance_variable" | "class_variable" | "global_variable" | "constant" => {
            context.source.node_text(target)
        }
        // `rescue => Foo::Bar` is a `casgn` whose name is only the last part, though the offense
        // still covers the whole path.
        "scope_resolution" => context.source.node_text(target.field("name")?),
        _ => return None,
    };
    Some((target, name.to_owned()))
}

fn reads_name(
    context: &RuleContext<'_>,
    variables: &Analysis<'_>,
    node: Node<'_>,
    name: &str,
) -> bool {
    let mut found = false;
    crate::rules::walk_named(node, &mut |current| {
        found = found
            || (current.kind_str() == "identifier"
                && context.source.node_text(current) == name
                && variables.is_reference(current));
    });
    found
}

/// The replacements upstream makes, one edit each: the variable itself, its reads
/// inside the handler, and -- when the handler never reassigns it -- its reads in the statements
/// that follow the `begin`/`end` it belongs to.
fn rename(
    context: &RuleContext<'_>,
    variables: &Analysis<'_>,
    node: Node<'_>,
    variable: Range<usize>,
    name: &str,
    preferred: &str,
) -> Vec<Edit> {
    let mut rewrite = Rewrite {
        context,
        variables,
        name,
        preferred,
        sites: vec![(variable.clone(), preferred.to_owned())],
    };
    let stopped = node
        .field("body")
        .is_some_and(|body| rewrite.walk_children(body));
    if !stopped && let Some(block) = enclosing_begin(node) {
        // Once the handler ends the variable is still in scope, so the reads after the block are
        // renamed too -- again only up to a reassignment.
        for sibling in right_siblings(block) {
            if rewrite.walk(sibling) {
                break;
            }
        }
    }
    // One edit per site, which is what `corrector.replace` is called for upstream. Collapsing them
    // into a single edit spanning the first site to the last swallows everything written between
    // them, and that span reaches well past this handler: the reads after the `begin`/`end` are
    // renamed too, so a second `rescue` further down the file ends up inside it. Its own offence
    // then clobbers against this one and is put off to the next pass, which leaves a handler whose
    // body reads the new name while the variable still carries the old one -- and
    // `Lint/UselessAssignment` deletes the `=> error` it now believes nothing reads.
    let safe = context.setting("Safe").unwrap_or(true);
    rewrite
        .sites
        .into_iter()
        .map(|(range, replacement)| Edit {
            start: range.start,
            end: range.end,
            replacement,
            safe,
        })
        .collect()
}

struct Rewrite<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    variables: &'a Analysis<'a>,
    name: &'a str,
    preferred: &'a str,
    sites: Vec<(Range<usize>, String)>,
}

impl Rewrite<'_, '_> {
    /// `correct_node`: renames every read of the variable until it is reassigned, and reports
    /// whether that reassignment was found.
    fn walk(&mut self, node: Node<'_>) -> bool {
        if let Some(targets) = assignment_targets(node) {
            let matched = targets
                .iter()
                .any(|target| self.context.source.node_text(*target) == self.name);
            if matched {
                // The assignment's own target keeps the old name -- from here on it is a
                // different variable -- but its value is still the old one.
                if let Some(right) = node.field("right") {
                    self.walk(right);
                }
                return true;
            }
        }
        if node.kind_str() == "identifier"
            && self.context.source.node_text(node) == self.name
            && self.variables.is_reference(node)
        {
            self.sites
                .push((node.byte_range(), self.preferred.to_owned()));
            return false;
        }
        // `{ err: }` reads the variable without writing its name a second time, so the value has
        // to be spelled out rather than replaced.
        if node.kind_str() == "pair"
            && node.field("value").is_none()
            && let Some(key) = node.field("key")
            && self.context.source.node_text(key) == self.name
        {
            let mut cursor = node.walk();
            if let Some(colon) = node
                .children(&mut cursor)
                .find(|child| child.kind_str() == ":")
            {
                let at = colon.end_byte();
                self.sites.push((at..at, format!(" {}", self.preferred)));
            }
            return false;
        }
        self.walk_children(node)
    }

    fn walk_children(&mut self, node: Node<'_>) -> bool {
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
        children.into_iter().any(|child| self.walk(child))
    }
}

/// The names an assignment binds, when the parser would build an `lvasgn` or `masgn` for it.
fn assignment_targets<'tree>(node: Node<'tree>) -> Option<Vec<Node<'tree>>> {
    if !matches!(node.kind_str(), "assignment" | "operator_assignment") {
        return None;
    }
    let left = node.field("left")?;
    match left.kind_str() {
        "identifier" => Some(vec![left]),
        // A list the grammar invented is not a multiple assignment: only the last name is
        // assigned to.
        "left_assignment_list" if spurious_assignment_list(left) => {
            Some(last_named_child(left).into_iter().collect())
        }
        "left_assignment_list" | "destructured_left_assignment" => {
            let mut targets = Vec::new();
            collect_targets(left, &mut targets);
            Some(targets)
        }
        _ => None,
    }
}

fn collect_targets<'tree>(node: Node<'tree>, out: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind_str() {
            "identifier" => out.push(child),
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                collect_targets(child, out);
            }
            _ => {}
        }
    }
}

/// The `begin`/`end` the handler belongs to, which is the only shape whose following statements
/// upstream reaches for.
fn enclosing_begin(node: Node<'_>) -> Option<Node<'_>> {
    ancestors(node).find(|ancestor| ancestor.kind_str() == "begin")
}

/// What follows the block in the node upstream would have made its parent.
///
/// tree-sitter writes a `rescue`, `else` or `ensure` clause beside the statements it guards, while
/// the parser wraps them around it. Which of the two a sibling belongs to depends on how many
/// statements there are: two or more get a `begin` node of their own, whose siblings are the
/// statements alone, and a single one is the clause's own child, whose siblings are the clauses.
fn right_siblings(node: Node<'_>) -> Vec<Node<'_>> {
    let Some(parent) = node.parent() else {
        return Vec::new();
    };
    let mut cursor = parent.walk();
    let children: Vec<Node<'_>> = parent.named_children(&mut cursor).collect();
    let is_clause = |node: &Node<'_>| matches!(node.kind_str(), "rescue" | "else" | "ensure");
    let statements = children.iter().filter(|child| !is_clause(child)).count();
    let following = children
        .into_iter()
        .skip_while(|child| child.id() != node.id())
        .skip(1);
    if statements > 1 {
        return following.filter(|child| !is_clause(child)).collect();
    }
    // A lone statement sits directly under the clause, so the other clauses are its siblings --
    // except that a `rescue` takes the `ensure` for itself.
    let rescued = following.clone().any(|child| child.kind_str() == "rescue");
    following
        .filter(|child| !(rescued && child.kind_str() == "ensure"))
        .collect()
}

fn ancestors(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut current = node;
    std::iter::from_fn(move || {
        current = current.parent()?;
        Some(current)
    })
}
