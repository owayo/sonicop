use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, has_interpolation, is_string, string_text};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let path = context.source.path();
    if path.extension().and_then(|extension| extension.to_str()) != Some("rb") {
        return;
    }
    let Some(basename) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return;
    };
    for node in context.nodes_of("call") {
        let Some(method) = node.field("method") else {
            continue;
        };
        if context.source.node_text(method) != "require_relative" {
            continue;
        }
        let arguments = arguments(node);
        let Some(first) = arguments.first().map(|argument| argument.first()) else {
            continue;
        };
        // `respond_to?(:value)`: only a literal has one, and an interpolated string is a `dstr`.
        if !is_string(first, context) || has_interpolation(first) {
            continue;
        }
        let required = string_text(first, context);
        if required != basename && required != format!("{basename}.rb") {
            continue;
        }
        let range = node.byte_range();
        offenses.push(
            context
                .offense(
                    "Remove the `require_relative` that requires itself.",
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: line_start(context, range.start),
                    end: line_end(context, range.end),
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `range_by_whole_lines`: the offense grows to the whole lines it lies on, and
/// `include_final_newline` takes the line break that ends the last of them.
fn line_start(context: &RuleContext<'_>, position: usize) -> usize {
    let text = context.source.text();
    text[..position]
        .rfind('\n')
        .map_or(0, |offset| offset + 1)
}

fn line_end(context: &RuleContext<'_>, position: usize) -> usize {
    let text = context.source.text();
    text[position..]
        .find('\n')
        .map_or(text.len(), |offset| (position + offset + 1).min(text.len()))
}
