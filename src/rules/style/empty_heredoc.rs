//! `Style/EmptyHeredoc`: a heredoc with nothing in it is a string literal written the long way.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::heredoc_body;

const MSG: &str = "Use an empty string literal instead of heredoc.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("heredoc_beginning") {
        // `node.xstr_type?`: a backtick heredoc runs a command, which no string literal replaces.
        if context.source.node_text(node).contains('`') {
            continue;
        }
        let Some(body) = heredoc_body(node, context) else {
            continue;
        };
        let Some(terminator) = super::nodes::children(body)
            .into_iter()
            .find(|child| child.kind_str() == "heredoc_end")
        else {
            continue;
        };
        if !is_empty(body, terminator, context) {
            continue;
        }
        // `range_by_whole_lines(heredoc_end, include_final_newline: true)`. Upstream removes the
        // body's own whole lines as well, but an empty body sits at the start of the terminator's
        // line, so the two ranges are the same one.
        let (line, _) = context.source.line_column(terminator.start_byte());
        let terminator_line = context.source.line_range(line);
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by_all([
            Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: preferred_string_literal(context),
                safe: true,
            },
            Edit {
                start: terminator_line.start,
                end: terminator_line.end,
                replacement: String::new(),
                safe: true,
            },
        ]));
    }
}

/// `node.loc.heredoc_body.source.empty?`.
///
/// The grammar's body starts where the opener was written rather than on the line below, and it runs
/// up to the terminator's own text rather than to the start of its line. What upstream calls the
/// body is therefore what lies between the first newline and the indentation a squiggly terminator
/// was written with -- everything the body holds, not just the run of text it opens with. A body
/// that begins with an interpolation opens with a `heredoc_content` of nothing but the line break
/// and the indentation, so reading that alone reports a heredoc that has something in it.
fn is_empty(body: Node<'_>, terminator: Node<'_>, context: &RuleContext<'_>) -> bool {
    let text = context
        .source
        .slice(body.start_byte()..terminator.start_byte());
    text.split_once('\n').is_some_and(|(_, rest)| {
        !rest.contains('\n') && rest.bytes().all(|byte| byte == b' ' || byte == b'\t')
    })
}

/// `StringLiteralsHelp#preferred_string_literal`, which reads `Style/StringLiterals`.
fn preferred_string_literal(context: &RuleContext<'_>) -> String {
    let double = context
        .setting_of::<String>("Style/StringLiterals", "EnforcedStyle")
        .is_some_and(|style| style == "double_quotes");
    if double { "\"\"" } else { "''" }.to_owned()
}
