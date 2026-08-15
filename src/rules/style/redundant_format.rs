use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal::string_value;
use crate::rules::send_node::{Argument, arguments, named_children};

use super::format_sequences::{Sequence, SequenceStyle, sequences};
use super::format_value::{Field, Value, format_with};

/// `RESTRICT_ON_SEND`.
const METHODS: &[&str] = &["format", "sprintf"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        let Some(method) = call.field("method") else {
            continue;
        };
        let name = context.source.node_text(method);
        if !METHODS.contains(&name) {
            continue;
        }
        let list = arguments(call);
        // `format_without_additional_args?`: one argument, a string or a constant, on no receiver or
        // on `Kernel`.
        if let Some(offense) = without_additional_arguments(call, &list, name, context) {
            offenses.push(offense);
            continue;
        }
        if let Some(offense) = unnecessary_fields(call, &list, name, context) {
            offenses.push(offense);
        }
    }
}

/// `format_without_additional_args?`: `format('text')` says no more than `'text'` does, unless the text
/// still holds a format sequence -- `format('%s')` raises and `format('%%')` answers with `'%'`.
fn without_additional_arguments(
    call: Node<'_>,
    list: &[Argument<'_>],
    name: &str,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    if !is_kernel_receiver(call, context) {
        return None;
    }
    let [only] = list else {
        return None;
    };
    let node = only.first();
    if only.parts().len() != 1
        || !matches!(node.kind_str(), "string" | "constant" | "scope_resolution")
    {
        return None;
    }
    // `string_with_format_sequence?`: a constant says nothing about its text, so only a literal can be
    // ruled out this way.
    if node.kind_str() == "string" && !sequences(&static_string_value(node, context)).is_empty() {
        return None;
    }
    Some(offense(
        call,
        escape_control_chars(context.source.node_text(node)),
        name,
        context,
    ))
}

/// `static_string_value`: the text of the literal parts alone, since an interpolation says nothing
/// about what it will hold.
fn static_string_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    named_children(node)
        .into_iter()
        .filter(|child| child.kind_str() != "interpolation")
        .map(|child| context.source.node_text(child))
        .collect()
}

/// `detect_unnecessary_fields` and `register_all_fields_literal`: every field of the format string is
/// filled by a literal, so the string it builds is known here and now.
fn unnecessary_fields(
    call: Node<'_>,
    list: &[Argument<'_>],
    name: &str,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    let (first, rest) = list.split_first()?;
    let literal = first.first();
    // `node.first_argument&.str_type?` and `return if node.first_argument.heredoc?`
    if first.parts().len() != 1 || literal.kind_str() != "string" || has_interpolation(literal) {
        return None;
    }
    if rest.is_empty() {
        return None;
    }
    // `splatted_arguments?`: what a splat holds is unknown.
    if rest.iter().any(|argument| is_splatted(argument)) {
        return None;
    }
    let string = string_value(literal, context);
    let found = sequences(&string);
    let values = all_fields_literal(&found, rest, context)?;
    let formatted = format_with(&string, &found, &values)?;
    Some(offense(
        call,
        quote(literal, call, escape_control_chars(&formatted), context),
        name,
        context,
    ))
}

/// `all_fields_literal?`: the value each field will be filled with, in the order the sequences take
/// them, or nothing when any field cannot be worked out here.
fn all_fields_literal(
    found: &[Sequence],
    rest: &[Argument<'_>],
    context: &RuleContext<'_>,
) -> Option<Vec<Field>> {
    if found.is_empty() {
        return None;
    }
    let hash = rest.iter().find(|argument| is_hash(argument));
    // The positional arguments are consumed in order, and a variable width takes one of its own.
    let mut queue: Vec<&Argument<'_>> = rest.iter().collect();
    let mut values = Vec::new();
    let (mut used_numbered, mut used_sequential) = (false, false);
    for sequence in found {
        // `next if sequence.percent?`: the count is not raised for one, so `sequences.size == count`
        // can never hold once a `%%` was written.
        if sequence.style == SequenceStyle::Percent {
            return None;
        }
        let width = variable_width(sequence, &queue, context)?;
        // `format` refuses a string that names one argument by position and leaves another to the
        // order it was written in, and upstream reports nothing where `format` would have raised. A
        // `*` width takes an argument of its own, so it has a mode of its own too.
        let (numbered, sequential) = argument_modes(sequence);
        used_numbered |= numbered;
        used_sequential |= sequential;
        let argument = find_argument(sequence, &mut queue, hash, context)?;
        let value = argument_value(argument, context)?;
        if !matching_argument(sequence, &value, argument) {
            return None;
        }
        values.push(Field { value, width });
    }
    (!(used_numbered && used_sequential)).then_some(values)
}

/// How the field reaches the arguments it needs: by position, in the order they were written, or in
/// both ways at once where a `*` width names one and the value takes the next in line.
fn argument_modes(sequence: &Sequence) -> (bool, bool) {
    let (mut numbered, mut sequential) = (false, false);
    if let Some(rest) = sequence.width.strip_prefix('*') {
        match argument_number(rest).is_some() {
            true => numbered = true,
            false => sequential = true,
        }
    }
    // A named field reads the hash and asks nothing of the order.
    if matches!(
        sequence.style,
        SequenceStyle::Annotated | SequenceStyle::Template
    ) {
        return (numbered, sequential);
    }
    match argument_number(&sequence.flags).is_some() {
        true => numbered = true,
        false => sequential = true,
    }
    (numbered, sequential)
}

/// `unknown_variable_width?`: a `*` takes its width from an argument, which has to be a number for the
/// string to be known.
fn variable_width(
    sequence: &Sequence,
    queue: &[&Argument<'_>],
    context: &RuleContext<'_>,
) -> Option<Option<i64>> {
    if !sequence.width.starts_with('*') {
        return Some(None);
    }
    let number = argument_number(&sequence.width[1..]).unwrap_or(1);
    let argument = queue.get(number - 1)?;
    if argument.parts().len() != 1 {
        return None;
    }
    match argument_value(argument.first(), context)? {
        Value::Int(width) => Some(Some(width)),
        _ => None,
    }
}

/// `find_argument`: a named field reads the hash, a numbered one counts from the front, and the rest
/// take the next one in line.
fn find_argument<'a, 'tree>(
    sequence: &Sequence,
    queue: &mut Vec<&'a Argument<'tree>>,
    hash: Option<&'a Argument<'tree>>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let named = matches!(
        sequence.style,
        SequenceStyle::Annotated | SequenceStyle::Template
    );
    if let (Some(hash), true) = (hash, named) {
        return hash_value(hash, sequence.name.as_deref()?, context);
    }
    if sequence.width.starts_with('*') {
        let number = argument_number(&sequence.width[1..]).unwrap_or(1);
        if number - 1 < queue.len() {
            queue.remove(number - 1);
        }
        return (!queue.is_empty()).then(|| queue.remove(0).first());
    }
    if let Some(number) = argument_number(&sequence.flags) {
        return queue.get(number - 1).map(|argument| argument.first());
    }
    (!queue.is_empty()).then(|| queue.remove(0).first())
}

/// The `\d+$` a flag run or a `*` width may carry, which names the argument by position.
fn argument_number(text: &str) -> Option<usize> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty() && text[digits.len()..].starts_with('$'))
        .then(|| digits.parse().ok())
        .flatten()
}

/// `find_hash_value_node`: the value a `key: value` pair holds under `name`.
fn hash_value<'tree>(
    argument: &Argument<'tree>,
    name: &str,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    pairs(argument).into_iter().find_map(|pair| {
        let key = pair.field("key")?;
        let text = context.source.node_text(key);
        (text.trim_end_matches(':').trim_start_matches(':') == name).then(|| pair.field("value"))?
    })
}

/// `matching_argument?`: whether the field can be filled with what was written.
fn matching_argument(sequence: &Sequence, value: &Value, node: Node<'_>) -> bool {
    // `argument.type?(*ACCEPTABLE_LITERAL_TYPES)` asks about the node, so a number written as
    // `2 / 4r` -- a call upstream -- is no literal however well its value is known.
    if sequence.style == SequenceStyle::Template {
        return is_literal(node);
    }
    match sequence.kind {
        Some('s') => is_literal(node),
        Some('d' | 'i' | 'u') => value.as_integer().is_some(),
        Some('f') => value.as_float().is_some(),
        _ => false,
    }
}

/// `ACCEPTABLE_LITERAL_TYPES`: the kinds upstream's parser writes as a literal rather than a call.
fn is_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "string"
            | "chained_string"
            | "simple_symbol"
            | "delimited_symbol"
            | "integer"
            | "float"
            | "rational"
            | "complex"
            | "true"
            | "false"
            | "nil"
    ) || (node.kind_str() == "unary" && signed_number(node))
}

/// A sign the parser folds into the number it was written on.
fn signed_number(node: Node<'_>) -> bool {
    node.field("operand").is_some_and(|operand| {
        matches!(
            operand.kind_str(),
            "integer" | "float" | "rational" | "complex"
        )
    })
}

/// `argument_value`: what the literal stands for, or nothing when it is no literal at all.
fn argument_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<Value> {
    Value::of(node, context)
}

/// `quote`: the delimiters the format string was written with, changed where an interpolation forces
/// a reading pair.
fn quote(literal: Node<'_>, call: Node<'_>, text: String, context: &RuleContext<'_>) -> String {
    let source = context.source.node_text(literal);
    let mut open = opening_delimiter(source);
    let mut close = closing_delimiter(source);
    if has_interpolated_descendant(call) {
        if open == "'" {
            open = "\"".to_owned();
            close = "\"".to_owned();
        } else if let Some(rest) = open.strip_prefix("%q") {
            open = format!("%Q{rest}");
        }
    }
    format!("{open}{text}{close}")
}

fn opening_delimiter(source: &str) -> String {
    match source.starts_with('%') {
        true => source.chars().take(3).collect(),
        false => source.chars().take(1).collect(),
    }
}

fn closing_delimiter(source: &str) -> String {
    source
        .chars()
        .next_back()
        .map(String::from)
        .unwrap_or_default()
}

/// `node.each_descendant(:dstr, :dsym).any?`: a literal upstream's parser builds out of parts, which
/// is one that interpolates and one written as literals side by side.
fn has_interpolated_descendant(call: Node<'_>) -> bool {
    let mut stack = named_children(call);
    while let Some(node) = stack.pop() {
        if node.kind_str() == "chained_string"
            || (matches!(node.kind_str(), "string" | "delimited_symbol")
                && has_interpolation(node))
        {
            return true;
        }
        stack.extend(named_children(node));
    }
    false
}

/// `escape_control_chars`: `string.gsub(/\p{Cc}/) { |s| s.dump[1..-2] }`, which touches the control
/// characters and nothing else.
fn escape_control_chars(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            '\u{1b}' => out.push_str("\\e"),
            '\0' => out.push_str("\\0"),
            other if other.is_control() => out.push_str(&format!("\\u{:04X}", other as u32)),
            other => out.push(other),
        }
    }
    out
}

fn has_interpolation(node: Node<'_>) -> bool {
    named_children(node)
        .iter()
        .any(|child| child.kind_str() == "interpolation")
}

/// `(send {(const {nil? cbase} :Kernel) nil?} ...)`.
fn is_kernel_receiver(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(receiver) = call.field("receiver") else {
        return true;
    };
    match receiver.kind_str() {
        "constant" => context.source.node_text(receiver) == "Kernel",
        "scope_resolution" => {
            receiver.field("scope").is_none()
                && receiver
                    .field("name")
                    .is_some_and(|name| context.source.node_text(name) == "Kernel")
        }
        _ => false,
    }
}

/// `splat` and `(hash <kwsplat ...>)`.
fn is_splatted(argument: &Argument<'_>) -> bool {
    argument.parts().iter().any(|part| {
        matches!(
            part.kind_str(),
            "splat_argument" | "hash_splat_argument" | "splat_parameter"
        )
    })
}

/// Whether the argument is the hash a run of `key: value` pairs builds, however it was written.
fn is_hash(argument: &Argument<'_>) -> bool {
    argument.parts().len() > 1 || matches!(argument.first().kind_str(), "hash" | "pair")
}

/// The pairs the hash holds, whether it was written with braces or without.
fn pairs<'tree>(argument: &Argument<'tree>) -> Vec<Node<'tree>> {
    match argument.first().kind_str() {
        "hash" => named_children(argument.first()),
        _ => argument.parts().to_vec(),
    }
}

/// The offense, whose correction replaces the whole call with the string it builds.
fn offense(call: Node<'_>, replacement: String, name: &str, context: &RuleContext<'_>) -> Offense {
    let message = format!("Use `{replacement}` directly instead of `{name}`.");
    context
        .offense(message, call.byte_range())
        .corrected_by(Edit {
            start: call.start_byte(),
            end: call.end_byte(),
            replacement,
            safe: true,
        })
}
