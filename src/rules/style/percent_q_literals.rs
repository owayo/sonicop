//! `Style/PercentQLiterals`: `%Q` is only needed where `%q` would read differently.

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const LOWER_CASE_Q_MSG: &str = "Do not use `%Q` unless interpolation is needed. Use `%q`.";
const UPPER_CASE_Q_MSG: &str = "Use `%Q` instead of `%q`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let lower_case = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "lower_case_q");

    // `on_str` only: a literal that interpolates is a `dstr` upstream and never reaches this cop.
    for node in context.nodes_of("string") {
        let Some(literal) = super::percent::PercentLiteral::new(node, context) else {
            continue;
        };
        let wanted = if lower_case { "%q" } else { "%Q" };
        // `process(node, '%Q', '%q')` then `correct_literal_style?`.
        if !matches!(literal.percent_type.as_str(), "%q" | "%Q")
            || literal.percent_type == wanted
            || !is_str(node)
        {
            continue;
        }
        // Upstream reparses the swapped source and keeps the offense only when the value is the
        // same, which is what stops `%Q(a\nb)` from becoming a two-character `\n`.
        let body = &context.source.text()[literal.begin.end..literal.close.start];
        let delimiters = [literal.opening, closing_of(context, &literal)];
        let single = super::literal::decode(body, super::literal::Quoting::Single, &delimiters);
        let double = super::literal::decode(body, super::literal::Quoting::Double, &delimiters);
        if single.value != double.value {
            continue;
        }
        let text = context.source.node_text(node);
        let mut swapped = String::with_capacity(text.len());
        swapped.push('%');
        swapped.push_str(wanted.trim_start_matches('%'));
        swapped.push_str(&text[2..]);
        offenses.push(
            context
                .offense(
                    if lower_case {
                        LOWER_CASE_Q_MSG
                    } else {
                        UPPER_CASE_Q_MSG
                    },
                    literal.begin.clone(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: swapped,
                    safe: true,
                })
                .corrections_anchored_at(node.byte_range()),
        );
    }
}

fn closing_of(context: &RuleContext<'_>, literal: &super::percent::PercentLiteral) -> char {
    context
        .source
        .slice(literal.close.clone())
        .chars()
        .next()
        .unwrap_or(literal.opening)
}

/// `node.str_type?`: a literal is a `dstr` upstream both when it interpolates and when its text
/// does not fit on one line.
fn is_str(node: tree_sitter::Node<'_>) -> bool {
    let mut cursor = node.walk();
    !node
        .named_children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
        && node.start_position().row == node.end_position().row
}
