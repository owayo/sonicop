use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// Parameter lists, whichever of the three shapes the grammar writes one in.
const PARAMETER_LISTS: &[&str] = &["method_parameters", "block_parameters", "lambda_parameters"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for list in context.nodes_of_any(PARAMETER_LISTS) {
        let parameters = super::nodes::children(list);
        let keywords: Vec<usize> = (0..parameters.len())
            .filter(|index| parameters[*index].kind() == "keyword_parameter")
            .collect();
        // The first optional keyword parameter is the only one whose offense corrects; the rest
        // report and leave the rewrite to the next pass.
        let first_optional = keywords
            .iter()
            .copied()
            .find(|index| is_optional(parameters[*index]));
        for index in keywords
            .iter()
            .copied()
            .filter(|i| is_optional(parameters[*i]))
        {
            let required: Vec<Node<'_>> = parameters[index + 1..]
                .iter()
                .copied()
                .filter(|node| node.kind() == "keyword_parameter" && !is_optional(*node))
                .collect();
            if required.is_empty() {
                continue;
            }
            let node = parameters[index];
            let offense = context.offense(
                "Place optional keyword parameters at the end of the parameters list.",
                node.byte_range(),
            );
            offenses.push(match first_optional == Some(index) {
                true => correct(context, list, &parameters, node, &required, offense),
                false => offense,
            });
        }
    }
}

/// Whether the keyword parameter carries a default, which is what upstream calls a `kwoptarg`.
fn is_optional(node: Node<'_>) -> bool {
    node.child_by_field_name("value").is_some()
}

/// `autocorrect`: move the required keyword parameters in front of the first optional one.
fn correct(
    context: &RuleContext<'_>,
    list: Node<'_>,
    parameters: &[Node<'_>],
    node: Node<'_>,
    required: &[Node<'_>],
    offense: Offense,
) -> Offense {
    let (Some(first), Some(last)) = (parameters.first(), parameters.last()) else {
        return offense;
    };
    if super::nodes::contains_comment(&(first.start_byte()..last.end_byte()), context) {
        return offense;
    }
    let moved: Vec<&str> = required
        .iter()
        .map(|parameter| context.source.node_text(*parameter))
        .collect();
    let mut edits = vec![Edit {
        start: node.start_byte(),
        end: node.start_byte(),
        replacement: format!("{}, ", moved.join(", ")),
        safe: true,
    }];
    // `append_newline_to_last_kwoptarg`: without parentheses the removal below takes the line
    // break at the end of the list with it, so one has to be put back.
    let mut anchor = None;
    if !context.source.node_text(list).starts_with('(')
        && list
            .parent()
            .is_some_and(|parent| matches!(parent.kind(), "method" | "singleton_method"))
        && last.kind() == "keyword_parameter"
        && !is_optional(*last)
        && let Some(final_optional) = parameters
            .iter()
            .rev()
            .find(|parameter| parameter.kind() == "keyword_parameter" && is_optional(**parameter))
    {
        if final_optional.id() != node.id() {
            anchor = Some(final_optional.byte_range());
        }
        edits.push(Edit {
            start: final_optional.end_byte(),
            end: final_optional.end_byte(),
            replacement: "\n".to_owned(),
            safe: true,
        });
    }
    for parameter in required {
        let removal = surrounding_comma_left(surrounding_space(parameter.byte_range(), context), context);
        edits.push(Edit {
            start: removal.start,
            end: removal.end,
            replacement: String::new(),
            safe: true,
        });
    }
    let offense = offense.corrected_by_all(edits);
    match anchor {
        Some(range) => offense.corrections_anchored_at(range),
        None => offense,
    }
}

/// `range_with_surrounding_space(range)`: the span plus the blanks, line continuations and line
/// breaks on either side of it.
fn surrounding_space(
    range: std::ops::Range<usize>,
    context: &RuleContext<'_>,
) -> std::ops::Range<usize> {
    let text = context.source.text().as_bytes();
    let mut start = range.start;
    while start > 0 && matches!(text[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    while start >= 2 && text[start - 1] == b'\n' && text[start - 2] == b'\\' {
        start -= 2;
    }
    while start > 0 && text[start - 1] == b'\n' {
        start -= 1;
    }
    let mut end = range.end;
    while end < text.len() && matches!(text[end], b' ' | b'\t') {
        end += 1;
    }
    while end + 1 < text.len() && text[end] == b'\\' && text[end + 1] == b'\n' {
        end += 2;
    }
    while end < text.len() && text[end] == b'\n' {
        end += 1;
    }
    start..end
}

/// `range_with_surrounding_comma(range, :left)`.
fn surrounding_comma_left(
    range: std::ops::Range<usize>,
    context: &RuleContext<'_>,
) -> std::ops::Range<usize> {
    let text = context.source.text().as_bytes();
    let mut start = range.start;
    while start > 0 && text[start - 1] == b',' {
        start -= 1;
    }
    start..range.end
}
