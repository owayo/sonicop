use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not define multiple classes/modules at the top level in a single file.";

/// Every top-level `class` or `module` after the first.
///
/// `top_level_definition?` asks whether the node is the root or sits directly in the root's
/// statement list, which is the same as having `program` for a parent here. `class << self` is an
/// `sclass` upstream and never reaches the cop.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = context
        .setting::<Vec<String>>("AllowedClasses")
        .unwrap_or_default();
    let mut seen = 0_usize;
    for node in context.nodes_of_any(&["class", "module"]) {
        if !node
            .parent()
            .is_some_and(|parent| parent.kind_str() == "program")
        {
            continue;
        }
        let Some(name) = node.field("name") else {
            continue;
        };
        // `node.identifier.short_name`: the last segment, so `class Foo::Bar` is named `Bar`.
        let short = match name.kind_str() {
            "scope_resolution" => match name.field("name") {
                Some(inner) => context.source.node_text(inner),
                None => continue,
            },
            _ => context.source.node_text(name),
        };
        if allowed.iter().any(|entry| entry == short) {
            continue;
        }
        seen += 1;
        if seen > 1 {
            // `range_between(node.source_range.begin_pos, node.loc.name.end_pos)`.
            offenses.push(context.offense(MSG, node.start_byte()..name.end_byte()));
        }
    }
}
