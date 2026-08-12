use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "More than one disable comment on one line.";
const DIRECTIVES: [&str; 2] = ["# rubocop:disable", "# rubocop:todo"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("comment") {
        let text = context.source.node_text(node);
        if count_directives(text) < 2 {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: joined(text),
            safe: true,
        }));
    }
}

/// `text.scan(/# rubocop:(?:disable|todo)/).size`: the matches do not overlap, so a scan from left
/// to right that skips past each hit counts them the same way.
fn count_directives(text: &str) -> usize {
    let mut count = 0;
    let mut rest = text;
    while let Some((offset, length)) = first_directive(rest) {
        count += 1;
        rest = &rest[offset + length..];
    }
    count
}

fn first_directive(text: &str) -> Option<(usize, usize)> {
    DIRECTIVES
        .iter()
        .filter_map(|directive| text.find(directive).map(|offset| (offset, directive.len())))
        .min()
}

/// `text.gsub(%r{ # rubocop:(disable|todo)}, ',')`: each directive after the first becomes a comma
/// continuing the list the one before it opened.
fn joined(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some((offset, length)) = first_spaced_directive(rest) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..offset]);
        out.push(',');
        rest = &rest[offset + length..];
    }
}

fn first_spaced_directive(text: &str) -> Option<(usize, usize)> {
    DIRECTIVES
        .iter()
        .filter_map(|directive| {
            let needle = format!(" {directive}");
            text.find(&needle).map(|offset| (offset, needle.len()))
        })
        .min()
}
