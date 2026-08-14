use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Remove redundant `::`.";

/// `"::".len()`, which is all a `cbase` node ever spans.
const CBASE_LENGTH: usize = 2;

/// `on_cbase`: the `::` in front of a constant, when nothing it could be shadowed by encloses it.
///
/// A `class` whose *superclass* holds the `::` does not count as enclosing it, because the
/// superclass is resolved outside the class body.
///
/// Upstream has a second path, `provably_unshadowed?`, that consults a project-wide constant index.
/// The index is opt-in through `AllCops: UseProjectIndex` and is not built here, so that path is
/// never taken -- which is also what upstream does with the default configuration.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `lint_constant_resolution_cop_enabled?`: the two cops ask for opposite things.
    if context.cop_enabled("Lint/ConstantResolution") {
        return;
    }
    for node in context.nodes_of("scope_resolution") {
        // `(cbase)`: a `::Name` with nothing before the `::`.
        if node.field("scope").is_some() {
            continue;
        }
        if nesting_ancestors(node).next().is_some() {
            continue;
        }
        let range = node.start_byte()..node.start_byte() + CBASE_LENGTH;
        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// `module_nesting_ancestors_of`: the `class` and `module` nodes around the `::`, minus a `class`
/// whose superclass part is where the `::` was written.
fn nesting_ancestors<'tree>(node: Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
    let start = node.start_byte();
    std::iter::successors(node.parent(), |current| current.parent()).filter(move |ancestor| {
        match ancestor.kind_str() {
            "module" => true,
            "class" => ancestor.field("superclass").is_none_or(|superclass| {
                !(superclass.start_byte()..superclass.end_byte()).contains(&start)
            }),
            _ => false,
        }
    })
}
