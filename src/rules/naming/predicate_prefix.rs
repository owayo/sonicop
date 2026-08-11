use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let prefixes: Vec<String> = context.setting("NamePrefix").unwrap_or_default();
    let forbidden: Vec<String> = context.setting("ForbiddenPrefixes").unwrap_or_default();
    let allowed_methods: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    let macros: Vec<String> = context
        .setting("MethodDefinitionMacros")
        .unwrap_or_default();
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
        if node.kind() == "call" {
            let Some((name, range)) = dynamic_definition(context, node, &macros) else {
                continue;
            };
            report(offenses, &name, range);
            continue;
        }
        let Some(name_node) = node.child_by_field_name("name") else {
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
    if node.child_by_field_name("receiver").is_some() {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    let name = context.source.node_text(method);
    if !macros.iter().any(|macro_name| macro_name == name) {
        return None;
    }
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let first = arguments.named_children(&mut cursor).next()?;
    Some((symbol_name(context, first)?, first.byte_range()))
}

/// The name a `sym` node spells. `:"a b"` is one too, but `:"a#{b}"` is a `dsym` and names
/// nothing.
fn symbol_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    match node.kind() {
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
                if child.kind() != "string_content" {
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
