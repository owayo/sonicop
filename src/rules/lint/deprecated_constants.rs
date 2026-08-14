use std::collections::BTreeMap;

use serde::Deserialize;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// One entry of the `DeprecatedConstants` table.
#[derive(Deserialize)]
struct Deprecation {
    #[serde(rename = "Alternative")]
    alternative: Option<String>,
    #[serde(rename = "DeprecatedVersion")]
    deprecated_version: Option<String>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(table) = context.setting::<BTreeMap<String, Deprecation>>("DeprecatedConstants")
    else {
        return;
    };
    for node in context.nodes_of_any(&["constant", "scope_resolution"]) {
        if !is_constant_read(node, context) {
            continue;
        }
        let source = context.source.node_text(node);
        let Some(deprecation) = table.get(source.strip_prefix("::").unwrap_or(source)) else {
            continue;
        };
        // The version the constant went out of use in, which a run targeting an older Ruby has
        // not reached yet.
        let version = deprecation.deprecated_version.as_deref();
        if version.is_some_and(|version| below_target(context, version)) {
            continue;
        }
        let since = version.map(|version| format!(", deprecated since Ruby {version}"));
        let since = since.as_deref().unwrap_or_default();
        let message = match &deprecation.alternative {
            Some(good) => format!("Use `{good}` instead of `{source}`{since}."),
            None => format!("Do not use `{source}`{since}."),
        };
        let range = node.byte_range();
        let offense = context.offense(message, range.clone());
        offenses.push(match &deprecation.alternative {
            Some(good) => offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: good.clone(),
                safe: true,
            }),
            None => offense,
        });
    }
}

/// Whether upstream's parser would have built a `const` node here.
///
/// The name written after `::` is no node of its own there, and the target of a constant
/// assignment is a `casgn` rather than a `const` -- but the scope written in front of that target
/// still is one, which is why only the target itself is skipped.
fn is_constant_read(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return true;
    };
    let is_field = |field: &str| {
        parent
            .field(field)
            .is_some_and(|child| child.id() == node.id())
    };
    match parent.kind_str() {
        "scope_resolution" => !is_field("name"),
        "assignment" | "operator_assignment" => !is_field("left"),
        _ => true,
    }
}

/// `target_ruby_version < version.to_f`, which is the comparison the cop makes on a bare float.
fn below_target(context: &RuleContext<'_>, version: &str) -> bool {
    let mut parts = version.split('.');
    let major: u16 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor: u16 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    context.target_ruby_version() < crate::ruby_version::RubyVersion::new(major, minor)
}
