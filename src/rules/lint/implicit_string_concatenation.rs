use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{is_plain_send, string_text};
use crate::rules::send_node::named_children_of;

const FOR_ARRAY: &str =
    " Or, if they were intended to be separate array elements, separate them with a comma.";
const FOR_METHOD: &str =
    " Or, if they were intended to be separate method arguments, separate them with a comma.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // The only `dstr` whose consecutive children are separate literals in the source is the one
    // adjacent literals build: the parts of `"a\nb"` are fragments of a single literal, and the
    // delimiter test upstream applies is what tells the two apart.
    for node in context.nodes_of("chained_string") {
        let parts = named_children_of(node, context);
        let suffix = suffix(node, context);
        for pair in parts.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            if left.kind_str() != "string" || right.kind_str() != "string" {
                continue;
            }
            if context.source.line_column(left.end_byte()).0
                != context.source.line_column(right.start_byte()).0
            {
                continue;
            }
            let text = context.source.node_text(left);
            if !ends_with_its_own_delimiter(text) {
                continue;
            }
            // `"a"%[b]` is the format operator applied to an array, which the grammar reads as two
            // adjacent literals instead: after a value, a `%` never opens a percent literal.
            if context.source.node_text(right).starts_with('%') {
                continue;
            }
            let message = format!(
                "Combine {} and {} into a single string literal, rather than using implicit string concatenation.{suffix}",
                display(left, context),
                display(right, context)
            );
            offenses.push(
                context
                    .offense(message, left.start_byte()..right.end_byte())
                    .corrected_by(edit(left, right, context)),
            );
        }
    }
}

/// The rewrite: an empty literal is dropped, and otherwise the two are joined with `+`.
fn edit(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> Edit {
    if string_text(left, context).is_empty() {
        return Edit {
            start: left.start_byte(),
            end: left.end_byte(),
            replacement: String::new(),
            safe: true,
        };
    }
    if string_text(right, context).is_empty() {
        return Edit {
            start: right.start_byte(),
            end: right.end_byte(),
            replacement: String::new(),
            safe: true,
        };
    }
    Edit {
        start: left.end_byte(),
        end: right.start_byte(),
        replacement: " + ".to_owned(),
        safe: true,
    }
}

/// `display_str`: the source, unless it spans lines, in which case the value is shown inspected.
fn display(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let source = context.source.node_text(node);
    if !source.contains('\n') {
        return source.to_owned();
    }
    format!("{:?}", str_content(node, context))
}

/// `str_content`: the text of a literal with every interpolation dropped.
///
/// Upstream reaches the interpolation as a `begin` node holding a `send`, whose children answer
/// nothing to `str_type?` -- so the recursion joins an empty string for it. Showing the source
/// instead puts `#{...}` in a message that upstream writes without it.
fn str_content(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let mut cursor = node.walk();
    let parts: Vec<String> = node
        .children(&mut cursor)
        .filter(|child| child.is_named() && child.kind_str() != "interpolation")
        .map(|child| match child.kind_str() {
            // **`str_content` reads the value, not the source.** A `\` closing a line joins it to
            // the next and contributes nothing, so the escapes have to be resolved rather than
            // copied -- `"foo\nbar\<newline>"` is `foo\nbar` upstream.
            "escape_sequence" => {
                let mut out = String::new();
                crate::rules::ruby_literal::unescape(context.source.node_text(child), &mut out);
                out
            }
            _ => context.source.node_text(child).to_owned(),
        })
        .collect();
    match parts.is_empty() {
        true => string_text(node, context).to_owned(),
        false => parts.concat(),
    }
}

/// `str.source[-1] == ending_delimiter(str)`: the literal both opens and closes with a quote, which
/// a fragment of a literal written over several lines never does.
fn ends_with_its_own_delimiter(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    matches!(first, '\'' | '"') && text.len() > 1 && text.ends_with(first)
}

/// The sentence upstream appends when the two literals stand where a comma was likely meant.
fn suffix(node: Node<'_>, context: &RuleContext<'_>) -> &'static str {
    let Some(parent) = node.parent_of(context) else {
        return "";
    };
    match parent.kind_str() {
        "array" => FOR_ARRAY,
        // Every operator is a `send` upstream except the four that join conditions, which are
        // `and` and `or` nodes; an index is a `send` too, as is a call with the literal as its
        // receiver, and an argument list is no node of its own there.
        "binary" => match parent.field("operator") {
            Some(operator)
                if matches!(
                    context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                ) =>
            {
                ""
            }
            _ => FOR_METHOD,
        },
        "unary" | "element_reference" => FOR_METHOD,
        "call" if is_plain_send(parent, context) => FOR_METHOD,
        "argument_list" => parent
            .parent_of(context)
            .filter(|call| call.kind_str() == "call" && is_plain_send(*call, context))
            .map_or("", |_| FOR_METHOD),
        _ => "",
    }
}
