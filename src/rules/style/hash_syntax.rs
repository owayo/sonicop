use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

static HASH_ROCKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^:([A-Za-z_][A-Za-z0-9_]*)\s*=>").unwrap());

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "ruby19".to_owned());
    if style != "ruby19" && style != "ruby19_no_mixed_keys" {
        return;
    }
    for node in context.nodes_of("pair") {
        let text = context.source.node_text(node);
        let Some(captures) = HASH_ROCKET.captures(text) else {
            continue;
        };
        // `ruby19` leaves a hash alone unless every key can take the new syntax, so that one
        // rocket that has to stay does not leave the hash in two styles at once.
        if style == "ruby19" && !all_hash_keys_are_symbols(node, context) {
            continue;
        }
        let whole = captures.get(0).unwrap();
        let name = captures.get(1).unwrap().as_str();
        let start = node.start_byte() + whole.start();
        let end = node.start_byte() + whole.end();
        offenses.push(
            context
                .offense("Use the new Ruby 1.9 hash syntax.", start..end)
                .corrected_by(Edit {
                    start,
                    end,
                    replacement: format!("{name}:"),
                    safe: true,
                }),
        );
    }
}

fn all_hash_keys_are_symbols(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(container) = node.parent() else {
        return false;
    };
    let mut cursor = container.walk();
    let pairs = container
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "pair")
        .collect::<Vec<_>>();
    !pairs.is_empty()
        && pairs.iter().all(|pair| {
            let Some(key) = pair.child_by_field_name("key") else {
                return false;
            };
            context.source.node_text(key).starts_with(':')
                || context.source.text().as_bytes().get(key.end_byte()) == Some(&b':')
        })
}
