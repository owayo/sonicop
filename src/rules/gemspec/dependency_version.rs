use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, is_string, send_range, string_text};
use crate::rules::support::{is_commit_reference, is_version_specification};

use super::support::first_specification_variable;

const METHODS: &[&str] = &[
    "add_dependency",
    "add_runtime_dependency",
    "add_development_dependency",
];

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
    // `(send (lvar #match_block_variable_name?) ...)`: the receiver has to be the parameter the
    // file's first specification block was opened with. Unlike the other `GemspecHelp` cops this one
    // does not accept `_1` or `it`.
    let Some(variable) = first_specification_variable(context) else {
        return;
    };
    let message = match required {
        true => "Dependency version specification is required.",
        false => "Dependency version specification is forbidden.",
    };
    for node in context.nodes_of("call") {
        if node
            .field("method")
            .is_none_or(|method| !METHODS.contains(&context.source.node_text(method)))
            || !is_plain_send(node, context)
        {
            continue;
        }
        if node.field("receiver").is_none_or(|receiver| {
            receiver.kind_str() != "identifier" || context.source.node_text(receiver) != variable
        }) {
            continue;
        }
        let arguments = arguments(node);
        // `allowed_gem?` reads `node.first_argument.str_content`, which takes the cop down on a call
        // written without arguments.
        let Some(first) = arguments.first() else {
            continue;
        };
        let name = is_string(first.first(), context).then(|| string_text(first.first(), context));
        if name.is_some_and(|name| allowed.iter().any(|gem| gem == name)) {
            continue;
        }
        let pinned = arguments.iter().any(|argument| {
            is_version_specification(argument, context) || is_commit_reference(argument, context)
        });
        if pinned == required {
            continue;
        }
        offenses.push(context.offense(message, send_range(node, context)));
    }
}
