use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::{Argument, arguments, is_plain_send, top_level_constant};
use crate::rules::node_ext::NodeExt;

const MESSAGE_SAFE_LEVEL: &str = "Passing safe_level with the 2nd argument of `ERB.new` is \
     deprecated. Do not use it, and specify other arguments as keyword arguments.";

/// `minimum_target_ruby_version 2.6`: the keyword form the cop asks for did not exist before.
const MINIMUM_VERSION: RubyVersion = RubyVersion::new(2, 6);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM_VERSION {
        return;
    }
    for node in context.nodes_of("call") {
        // `(send (const {nil? cbase} :ERB) :new $...)`.
        if !is_plain_send(node, context)
            || node
                .field("method")
                .is_none_or(|method| context.source.node_text(method) != "new")
            || node
                .field("receiver")
                .is_none_or(|receiver| !top_level_constant(receiver, "ERB", context))
        {
            continue;
        }
        let given = arguments(node);
        if given.is_empty() || correct_arguments(&given) {
            continue;
        }
        for (index, argument) in given.iter().enumerate().skip(1).take(3) {
            if is_hash(argument) {
                continue;
            }
            let message = message(index - 1, context.source.slice(argument.range()));
            offenses.push(
                context
                    .offense(message, argument.range())
                    .corrected_by(autocorrect(node, &given, context)),
            );
        }
    }
}

/// `arguments.size == 1 || (arguments.size == 2 && arguments[1].hash_type?)`.
fn correct_arguments(given: &[Argument<'_>]) -> bool {
    given.len() == 1 || (given.len() == 2 && is_hash(&given[1]))
}

/// Whether upstream's parser would have folded the argument into a `hash` node, which is what a
/// brace-less run of `key: value` pairs becomes as much as a written `{ }` does.
fn is_hash(argument: &Argument<'_>) -> bool {
    matches!(
        argument.first().kind_str(),
        "pair" | "hash_splat_argument" | "hash"
    )
}

fn message(position: usize, value: &str) -> String {
    match position {
        0 => MESSAGE_SAFE_LEVEL.to_owned(),
        1 => format!(
            "Passing trim_mode with the 3rd argument of `ERB.new` is deprecated. Use keyword \
             argument like `ERB.new(str, trim_mode: {value})` instead."
        ),
        _ => format!(
            "Passing eoutvar with the 4th argument of `ERB.new` is deprecated. Use keyword \
             argument like `ERB.new(str, eoutvar: {value})` instead."
        ),
    }
}

/// Rewrites the whole argument list as the string and the two keywords the legacy positions stood
/// for, which is what upstream replaces `arguments_range` with.
fn autocorrect(node: Node<'_>, given: &[Argument<'_>], context: &RuleContext<'_>) -> Edit {
    let mut keywords = build_keywords(given, context);
    override_by_legacy_arguments(&mut keywords, given, context);
    let mut parts = vec![context.source.slice(given[0].range()).to_owned()];
    parts.extend(keywords.into_iter().flatten());
    let start = given[0].range().start;
    let end = given[given.len() - 1].range().end;
    let _ = node;
    Edit {
        start,
        end,
        replacement: parts.join(", "),
        safe: true,
    }
}

/// The `trim_mode:` and `eoutvar:` the call already passes as keywords.
fn build_keywords(given: &[Argument<'_>], context: &RuleContext<'_>) -> [Option<String>; 2] {
    let mut keywords = [None, None];
    let last = &given[given.len() - 1];
    if !is_hash(last) {
        return keywords;
    }
    for pair in pairs(last) {
        let (Some(key), Some(value)) = (
            pair.field("key"),
            pair.field("value"),
        ) else {
            continue;
        };
        let value = context.source.node_text(value);
        match context.source.node_text(key) {
            "trim_mode" => keywords[0] = Some(format!("trim_mode: {value}")),
            "eoutvar" => keywords[1] = Some(format!("eoutvar: {value}")),
            _ => {}
        }
    }
    keywords
}

/// `pairs` of the hash argument, which is either a written `{ }` or the pairs themselves.
fn pairs<'tree>(argument: &Argument<'tree>) -> Vec<Node<'tree>> {
    if argument.first().kind_str() == "hash" {
        return crate::rules::send_node::named_children(argument.first())
            .into_iter()
            .filter(|child| child.kind_str() == "pair")
            .collect();
    }
    argument
        .parts()
        .iter()
        .copied()
        .filter(|part| part.kind_str() == "pair")
        .collect()
}

/// The legacy 3rd and 4th positions win over the keywords of the same name.
fn override_by_legacy_arguments(
    keywords: &mut [Option<String>; 2],
    given: &[Argument<'_>],
    context: &RuleContext<'_>,
) {
    if let Some(argument) = given.get(2)
        && !is_hash(argument)
    {
        keywords[0] = Some(format!(
            "trim_mode: {}",
            context.source.slice(argument.range())
        ));
    }
    if let Some(argument) = given.get(3)
        && !is_hash(argument)
    {
        keywords[1] = Some(format!(
            "eoutvar: {}",
            context.source.slice(argument.range())
        ));
    }
}
