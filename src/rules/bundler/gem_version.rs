use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, send_range, string_text};
use crate::rules::support::{is_commit_reference, is_version_specification};

use super::support::gem_declarations;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "required".to_owned());
    let required = match style.as_str() {
        "required" => true,
        "forbidden" => false,
        // Upstream's `message` answers `nil` for any other style, which `add_offense` would refuse.
        _ => return,
    };
    let allowed: Vec<String> = context.setting("AllowedGems").unwrap_or_default();
    let message = match required {
        true => "Gem version specification is required.",
        false => "Gem version specification is forbidden.",
    };
    for (node, name) in gem_declarations(context) {
        if allowed.iter().any(|gem| gem == string_text(name, context)) {
            continue;
        }
        let pinned = arguments(node).iter().any(|argument| {
            is_version_specification(argument, context) || is_commit_reference(argument, context)
        });
        if pinned == required {
            continue;
        }
        offenses.push(context.offense(message, send_range(node, context)));
    }
}
