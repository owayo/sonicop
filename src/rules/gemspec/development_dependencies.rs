use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send, is_string, send_range, string_text};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "Gemfile".to_owned());
    // `add_development_dependency 'gem', 'version'` belongs in the manifest under either manifest
    // style, and `gem 'name'` belongs in the gemspec under the third. Only one method is looked at
    // per style, and how many arguments it may carry differs between the two.
    let (method, extra_arguments) = match style.as_str() {
        "Gemfile" | "gems.rb" => ("add_development_dependency", 2),
        "gemspec" => ("gem", 0),
        _ => return,
    };
    let allowed: Vec<String> = context.setting("AllowedGems").unwrap_or_default();
    let message = format!("Specify development dependencies in {style}.");
    for node in context.nodes_of("call") {
        if node
            .field("method")
            .is_none_or(|name| context.source.node_text(name) != method)
            || !is_plain_send(node, context)
        {
            continue;
        }
        let arguments = arguments(node);
        // `(str #forbidden_gem? ...) _? _?`: the name is a plain string literal, and at most that
        // many further arguments may follow it.
        let [name, rest @ ..] = arguments.as_slice() else {
            continue;
        };
        if rest.len() > extra_arguments {
            continue;
        }
        let name = name.first();
        if !is_string(name, context) || allowed.iter().any(|gem| gem == string_text(name, context))
        {
            continue;
        }
        offenses.push(context.offense(message.clone(), send_range(node, context)));
    }
}
