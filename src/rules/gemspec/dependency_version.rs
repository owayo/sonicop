use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{
    Argument, arguments, is_plain_send, is_string, named_children, pair_key_symbol, send_range,
    string_text,
};

use super::support::first_specification_variable;

const METHODS: &[&str] = &[
    "add_dependency",
    "add_runtime_dependency",
    "add_development_dependency",
];

/// `VERSION_SPECIFICATION_REGEX`. Ruby anchors `^` at the start of a *line*, which this engine only
/// does under `(?m)`.
static VERSION_SPECIFICATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*[~<>=]*\s*[0-9.]+").expect("the version requirement pattern compiles")
});

/// The keys that pin a dependency to a commit rather than to a version.
const COMMIT_KEYS: &[&str] = &["branch", "ref", "tag"];

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

/// `<(str #version_specification?) ...>`: one of the arguments is a string that opens with a version
/// requirement.
fn is_version_specification(argument: &Argument<'_>, context: &RuleContext<'_>) -> bool {
    let node = argument.first();
    argument.parts().len() == 1
        && is_string(node, context)
        && VERSION_SPECIFICATION.is_match(string_text(node, context))
}

/// `<(hash <(pair (sym {:branch :ref :tag}) (str _)) ...>) ...>`: one of the arguments is a hash that
/// pins the dependency to a commit.
fn is_commit_reference(argument: &Argument<'_>, context: &RuleContext<'_>) -> bool {
    // A trailing run of `key: value` pairs is one `hash` argument upstream even though it was
    // written without braces, so both spellings have to be looked into.
    let pairs: Vec<Node<'_>> = match argument.first().kind_str() {
        "hash" if argument.parts().len() == 1 => named_children(argument.first()),
        _ => argument.parts().to_vec(),
    };
    pairs.iter().any(|pair| {
        pair.kind_str() == "pair"
            && pair_key_symbol(*pair, context).is_some_and(|key| COMMIT_KEYS.contains(&key))
            && pair
                .field("value")
                .is_some_and(|value| is_string(value, context))
    })
}
