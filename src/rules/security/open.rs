use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::{
    arguments, heredoc_body, is_plain_send, is_string, named_children, string_text,
    top_level_constant,
};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if context.source.node_text(method) != "open" || !is_plain_send(node, context) {
            continue;
        }
        // `(send ${nil? (const {nil? cbase} :URI)} :open $_ ...)`: `Kernel#open` and `URI.open`
        // alone. Any other receiver names a method that cannot start a process.
        let receiver = node.child_by_field_name("receiver");
        if receiver.is_some_and(|receiver| !top_level_constant(receiver, "URI", context)) {
            continue;
        }
        let arguments = arguments(node);
        let Some(first) = arguments.first() else {
            continue;
        };
        if safe(first.first(), context) {
            continue;
        }
        let receiver = match receiver {
            Some(receiver) => format!("{}.", context.source.node_text(receiver)),
            None => "Kernel#".to_owned(),
        };
        offenses.push(context.offense(
            format!("The use of `{receiver}open` is a serious security risk."),
            method.byte_range(),
        ));
    }
}

/// Mirrors `safe?`: a literal string cannot smuggle in a command as long as it names something. A
/// string built out of parts is judged by the part it opens with, and anything else -- a variable,
/// a method call, a symbol -- is dynamic and unsafe.
fn safe(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    // `?a`, `__FILE__` and every quoted literal without interpolation is a `str` upstream.
    if is_string(node, context) {
        return safe_argument(string_text(node, context));
    }
    match node.kind() {
        // A string that interpolates is judged by the literal text it opens with; one that opens
        // with the interpolation itself has no literal part and stays unsafe.
        "string" => safe_argument(&leading_literal(node, context)),
        // `"a" "b"` reaches upstream as a `dstr` of the two literals, so the first one decides.
        "chained_string" => named_children(node)
            .first()
            .is_some_and(|first| safe(*first, context)),
        "heredoc_beginning" => {
            heredoc_leading_text(node, context).is_some_and(|text| safe_argument(&text))
        }
        // `open("| " + command)` is judged by the literal it concatenates onto, which is the only
        // shape `concatenated_string?` accepts.
        "binary" => {
            node.child_by_field_name("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "+")
                && node
                    .child_by_field_name("left")
                    .is_some_and(|left| is_string(left, context) && safe(left, context))
        }
        _ => false,
    }
}

/// `safe_argument?`: an empty path names nothing, and a leading pipe runs a command.
fn safe_argument(argument: &str) -> bool {
    !argument.is_empty() && !argument.starts_with('|')
}

/// The literal text a string opens with, before its first interpolation. Upstream's parser merges
/// every adjacent literal part into one `str`, so escapes and plain runs count together.
fn leading_literal(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let mut text = String::new();
    for child in named_children(node) {
        if child.kind() == "interpolation" {
            break;
        }
        text.push_str(context.source.node_text(child));
    }
    text
}

/// The heredoc body as `str_content` reports it, cut short at the first interpolation: without the
/// newline that ended the line the heredoc was opened on, and with the common indentation removed
/// when it was opened with `<<~`.
fn heredoc_leading_text(beginning: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let body = heredoc_body(beginning, context)?;
    let children = named_children(body);
    let boundary = |kinds: &[&str]| {
        children
            .iter()
            .find(|child| kinds.contains(&child.kind()))
            .map_or(body.end_byte(), Node::start_byte)
    };
    let content = context
        .source
        .slice(body.start_byte()..boundary(&["heredoc_end"]));
    let leading = context
        .source
        .slice(body.start_byte()..boundary(&["interpolation", "heredoc_end"]));
    // Every line of the body decides how much indentation comes off, not just the part before the
    // interpolation, so the width is measured against the whole thing.
    let indent = match context.source.node_text(beginning).starts_with("<<~") {
        true => content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0),
        false => 0,
    };
    Some(strip_indent(
        leading.strip_prefix('\n').unwrap_or(leading),
        indent,
    ))
}

fn strip_indent(text: &str, indent: usize) -> String {
    if indent == 0 {
        return text.to_owned();
    }
    text.split_inclusive('\n')
        .map(|line| match line.len() >= indent {
            true => &line[indent..],
            false => line,
        })
        .collect()
}
