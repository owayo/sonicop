//! `Style/RedundantPercentQ`: `%q` and `%Q` earn their delimiters only when both quotes appear.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("string") {
        let Some(literal) = super::percent::PercentLiteral::new(node, context) else {
            continue;
        };
        if !matches!(literal.percent_type.as_str(), "%q" | "%Q") {
            continue;
        }
        let source = context.source.node_text(node);
        // `interpolated_quotes?`: a literal holding both quotes is what `%q` is for.
        if source.contains('\'') && source.contains('"') {
            continue;
        }
        let capital = literal.percent_type == "%Q";
        let allowed = if capital {
            acceptable_capital_q(source, node)
        } else {
            acceptable_q(source)
        };
        if allowed {
            continue;
        }
        let delimiter = if capital && !source.contains('"') || source.contains('\'') {
            '"'
        } else {
            '\''
        };
        let extra = if capital {
            ", or for dynamic strings that contain double quotes"
        } else {
            ""
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{}` only for strings that contain both single quotes and double \
                         quotes{extra}.",
                        &source[..2]
                    ),
                    node.byte_range(),
                )
                .corrected_by_all([
                    Edit {
                        start: literal.begin.start,
                        end: literal.begin.end,
                        replacement: delimiter.to_string(),
                        safe: true,
                    },
                    Edit {
                        start: literal.close.start,
                        end: literal.close.end,
                        replacement: delimiter.to_string(),
                        safe: true,
                    },
                ]),
        );
    }
}

/// `acceptable_q?`: `%q` is kept for an escape that would change meaning, and for a source that
/// both interpolates and holds a single quote.
fn acceptable_q(source: &str) -> bool {
    if holds_interpolation_text(source) && source.contains('\'') {
        return true;
    }
    // `src.scan(/\\./).any?(ESCAPED_NON_BACKSLASH)`: an escape of anything but a backslash.
    // `.` does not match a newline, so a backslash ending a line is not an escape at all and the
    // scan resumes one character later rather than two.
    let characters: Vec<char> = source.chars().collect();
    let mut index = 0;
    while index + 1 < characters.len() {
        if characters[index] != '\\' || characters[index + 1] == '\n' {
            index += 1;
            continue;
        }
        if characters[index + 1] != '\\' {
            return true;
        }
        index += 2;
    }
    false
}

/// `acceptable_capital_q?`.
fn acceptable_capital_q(source: &str, node: Node<'_>) -> bool {
    source.contains('"')
        && (holds_interpolation_text(source)
            || (is_str(node) && super::literal::double_quotes_required(source)))
}

/// `node.str_type?`: a literal is a `dstr` upstream both when it interpolates and when its text
/// does not fit on one line.
fn is_str(node: Node<'_>) -> bool {
    !holds_interpolation(node) && node.start_position().row == node.end_position().row
}

/// `/#\{.+\}/`, which upstream matches against the source rather than against the tree.
fn holds_interpolation_text(source: &str) -> bool {
    let Some(open) = source.find("#{") else {
        return false;
    };
    source[open + 2..].find('}').is_some_and(|close| close > 0)
}

fn holds_interpolation(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
}
