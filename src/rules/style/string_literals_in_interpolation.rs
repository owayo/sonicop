use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::string_literals::{
    corrected_literal, has_interpolation, inside_interpolation, is_dstr, quoted_label_key,
    wrong_quotes,
};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.nodes_of("interpolation").next().is_none() {
        return;
    }
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "single_quotes".to_owned());
    let single_quotes = style != "double_quotes";
    // `style.to_s.sub(/_(.*)s/, '-\1d')`: `single_quotes` names `single-quoted` strings.
    let message = match single_quotes {
        true => "Prefer single-quoted strings inside interpolations.",
        false => "Prefer double-quoted strings inside interpolations.",
    };

    for node in context.nodes_of("string") {
        let source = context.source.node_text(node);
        // `on_str` reaches only a plain `str` carrying its own quotes, and this cop only wants the
        // ones written inside a `#{}`. Unlike `Style/StringLiterals` it leaves `on_regexp` empty,
        // so a literal interpolated into a regexp is checked rather than ignored.
        if source.len() < 2
            || has_interpolation(node)
            || is_dstr(source)
            || quoted_label_key(node, context)
            || !inside_interpolation(node)
            || !wrong_quotes(source, single_quotes)
        {
            continue;
        }
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: corrected_literal(source, single_quotes),
                    safe: true,
                }),
        );
    }
}
