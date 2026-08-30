use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let prefixes: Vec<String> = context.setting("NamePrefix").unwrap_or_default();
    let forbidden: Vec<String> = context.setting("ForbiddenPrefixes").unwrap_or_default();
    let allowed_methods: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    let macros: Vec<String> = context
        .setting("MethodDefinitionMacros")
        .unwrap_or_default();
    // `UseSorbetSigs`: only a `def` whose preceding `sig` declares `returns(T::Boolean)` is a
    // predicate candidate. A dynamic definition carries no signature, so the macros are skipped
    // entirely under this setting -- "Dynamic methods are not supported with this configuration".
    let use_sorbet_sigs: bool = context.setting("UseSorbetSigs").unwrap_or(false);
    // Every prefix is tried, but `add_offense` keeps only the first report at a range.
    let mut reported: HashSet<Range<usize>> = HashSet::new();
    let mut report = |offenses: &mut Vec<Offense>, name: &str, range: Range<usize>| {
        for prefix in &prefixes {
            if allowed_method_name(name, prefix, &forbidden, &allowed_methods) {
                continue;
            }
            let expected = expected_name(name, prefix, &forbidden);
            if reported.insert(range.clone()) {
                offenses.push(
                    context.offense(format!("Rename `{name}` to `{expected}`."), range.clone()),
                );
            }
        }
    };

    for node in context.nodes_of_any(&["method", "singleton_method", "call"]) {
        if node.kind_str() == "call" {
            if use_sorbet_sigs {
                continue;
            }
            let Some((name, range)) = dynamic_definition(context, node, &macros) else {
                continue;
            };
            report(offenses, &name, range);
            continue;
        }
        if use_sorbet_sigs && !sorbet_boolean_sig(context, node) {
            continue;
        }
        let Some(name_node) = node.field("name") else {
            continue;
        };
        let name = context.source.node_text(name_node).to_owned();
        report(offenses, &name, name_node.byte_range());
    }
}

/// `dynamic_method_define`: a receiverless call to one of the configured macros whose first
/// argument is a plain symbol. A string names nothing here, and neither does an interpolated
/// symbol.
fn dynamic_definition(
    context: &RuleContext<'_>,
    node: Node<'_>,
    macros: &[String],
) -> Option<(String, Range<usize>)> {
    if node.field("receiver").is_some() {
        return None;
    }
    let method = node.field("method")?;
    let name = context.source.node_text(method);
    if !macros.iter().any(|macro_name| macro_name == name) {
        return None;
    }
    let arguments = node.field("arguments")?;
    let mut cursor = arguments.walk();
    let first = arguments.named_children(&mut cursor).next()?;
    Some((symbol_name(context, first)?, first.byte_range()))
}

/// The name a `sym` node spells. `:"a b"` is one too, but `:"a#{b}"` is a `dsym` and names
/// nothing.
fn symbol_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    match node.kind_str() {
        "simple_symbol" => Some(
            context
                .source
                .node_text(node)
                .trim_start_matches(':')
                .to_owned(),
        ),
        "delimited_symbol" => {
            let mut cursor = node.walk();
            let mut value = String::new();
            for child in node.named_children(&mut cursor) {
                if child.kind_str() != "string_content" {
                    return None;
                }
                value.push_str(context.source.node_text(child));
            }
            Some(value)
        }
        _ => None,
    }
}

/// `allowed_method_name?`: a name is left alone unless it opens with the prefix followed by a
/// non-digit, and even then a name that already reads as the corrected one, a setter, or one
/// listed in `AllowedMethods` passes.
fn allowed_method_name(
    name: &str,
    prefix: &str,
    forbidden: &[String],
    allowed_methods: &[String],
) -> bool {
    let matches_prefix = name.strip_prefix(prefix).is_some_and(|rest| {
        rest.chars()
            .next()
            .is_some_and(|first| !first.is_ascii_digit())
    });
    !matches_prefix
        || name == expected_name(name, prefix, forbidden)
        || name.ends_with('=')
        || allowed_methods.iter().any(|allowed| allowed == name)
}

/// `expected_name`: a forbidden prefix is dropped, and the name gains the question mark it is
/// missing.
fn expected_name(name: &str, prefix: &str, forbidden: &[String]) -> String {
    let mut expected = if forbidden.iter().any(|entry| entry == prefix) {
        name.replacen(prefix, "", 1)
    } else {
        name.to_owned()
    };
    if !name.ends_with('?') {
        expected.push('?');
    }
    expected
}

/// `sorbet_sig?(node, return_type: 'T::Boolean')`: the `sig { returns(...) }` block written
/// immediately before the definition, whose return type is spelled exactly `T::Boolean`.
fn sorbet_boolean_sig(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(previous) = previous_statement(node) else {
        return false;
    };
    // `(block (send nil? :sig) args (send _ :returns $_type))`, which the grammar writes as a
    // `call` of `sig` carrying the block rather than as a block wrapped around the call.
    if previous.kind_str() != "call" {
        return false;
    }
    if previous
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "sig")
    {
        return false;
    }
    let Some(block) = previous.field("block") else {
        return false;
    };
    let Some(body) = block.field("body") else {
        return false;
    };
    let mut found = false;
    crate::rules::walk_named(body, context, &mut |inner| {
        if found || inner.kind_str() != "call" {
            return;
        }
        let names_returns = inner
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "returns");
        if !names_returns {
            return;
        }
        found = inner.field("arguments").is_some_and(|arguments| {
            let mut cursor = arguments.walk();
            arguments
                .named_children(&mut cursor)
                .next()
                .is_some_and(|type_node| context.source.node_text(type_node) == "T::Boolean")
        });
    });
    found
}

/// `node.left_sibling`, skipping the comments the grammar keeps and upstream's AST does not.
fn previous_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut sibling = node.prev_named_sibling();
    while sibling.is_some_and(|node| node.kind_str() == "comment") {
        sibling = sibling.and_then(|node| node.prev_named_sibling());
    }
    sibling
}
