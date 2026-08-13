use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("global_variable") {
        let name = context.source.node_text(node);
        let Some(preferred) = preferred_expression(name) else {
            continue;
        };
        let preferred = format!("{}{preferred}", constant_prefix(node));
        let replacement = match derived_from_braceless_interpolation(context, node) {
            true => format!("{{{preferred}}}"),
            false => preferred.clone(),
        };
        offenses.push(
            context
                .offense(
                    format!("Prefer `{preferred}` over `{name}`."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// `preferred_expression_to`. `$+` / `$LAST_PAREN_MATCH` is deliberately left out upstream.
fn preferred_expression(name: &str) -> Option<String> {
    match name {
        "$&" | "$MATCH" => Some("Regexp.last_match(0)".to_owned()),
        "$`" | "$PREMATCH" => Some("Regexp.last_match.pre_match".to_owned()),
        "$'" | "$POSTMATCH" => Some("Regexp.last_match.post_match".to_owned()),
        _ => {
            let digits = name.strip_prefix('$')?;
            // `$0` is the program name, not a capture: the parser spells the numbered references
            // `$1` upwards, which never carry a leading zero.
            let numbered = digits.starts_with(|first: char| first.is_ascii_digit() && first != '0')
                && digits.chars().all(|digit| digit.is_ascii_digit());
            numbered.then(|| format!("Regexp.last_match({digits})"))
        }
    }
}

/// `constant_prefix`: inside a class or module body the constant is spelled from the root.
fn constant_prefix(node: Node<'_>) -> &'static str {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(parent.kind_str(), "class" | "module") {
            return "::";
        }
        current = parent.parent();
    }
    ""
}

/// `derived_from_braceless_interpolation?`: upstream sees the reference as a direct child of the
/// `dstr` / `regexp` / `xstr` when it was written as `#$1`, so the correction has to supply the
/// braces the source did without.
fn derived_from_braceless_interpolation(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind_str() == "interpolation" && !context.source.node_text(parent).starts_with("#{")
    })
}
