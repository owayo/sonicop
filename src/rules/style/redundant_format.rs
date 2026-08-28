//! `Style/RedundantFormat`: a `format` whose result is already written out in its arguments.

use tree_sitter::Node;

use super::format_sequences::{Sequence, SequenceStyle, sequences};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal::{string_value, unescape};
use crate::rules::send_node::{
    Argument, arguments, has_interpolation, heredoc_body, send_range, symbol_name,
};

/// `RESTRICT_ON_SEND`.
const FORMAT_METHODS: &[&str] = &["format", "sprintf"];

/// One argument as `format` would see it.
#[derive(Clone, PartialEq)]
enum Value {
    Text(String),
    /// A `dstr`: acceptable wherever a string is, but never a number.
    Dynamic(String),
    /// A `rational`, kept as the pair it reduces to.
    Rational(i128, i128),
    /// A `complex`, kept as what it prints as.
    Complex(String),
    Integer(i128),
    Float(f64),
    Boolean(bool),
    Nil,
    Pairs(Vec<(String, Value)>),
    /// A number upstream's parser keeps as the call it was written as -- `2 / 4r` and `1 + 2i`. Its
    /// value is known, but `ACCEPTABLE_LITERAL_TYPES` asks about the node, so a `%s` will not take it.
    Computed(Box<Value>),
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        if !is_format_method(call, context) {
            continue;
        }
        let call_arguments = arguments(call);
        // `format_without_additional_args?`: one argument, and it is already the answer. Only this
        // form asks about the receiver -- `detect_unnecessary_fields` matches `(send _ ...)`, so
        // `foo.format('%s', 'a')` is reported just as a receiverless one is.
        if is_kernel_receiver(call, context)
            && let [only] = call_arguments.as_slice()
            && let [value] = only.parts()
            && matches!(
                value.kind_str(),
                "string" | "chained_string" | "constant" | "scope_resolution" | "heredoc_beginning"
            )
            && !string_with_format_sequence(*value, context)
        {
            let replacement = escape_control_chars(context.source.node_text(*value));
            offenses.push(report(context, call, replacement));
            continue;
        }
        detect_unnecessary_fields(context, call, &call_arguments, offenses);
    }
}

/// The methods `RESTRICT_ON_SEND` names, whatever they were called on.
fn is_format_method(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    call.field("method")
        .is_some_and(|method| FORMAT_METHODS.contains(&context.source.node_text(method)))
}

/// `{(const {nil? cbase} :Kernel) nil?}`: the receiver the single-argument form asks for.
fn is_kernel_receiver(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    match call.field("receiver") {
        None => true,
        Some(receiver) => match receiver.kind_str() {
            "constant" => context.source.node_text(receiver) == "Kernel",
            "scope_resolution" => {
                receiver.field("scope").is_none()
                    && receiver
                        .field("name")
                        .is_some_and(|name| context.source.node_text(name) == "Kernel")
            }
            _ => false,
        },
    }
}

/// `string_with_format_sequence?`.
fn string_with_format_sequence(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match static_string_value(node, context) {
        Some(text) => !sequences(&text).is_empty(),
        None => false,
    }
}

/// `static_string_value`: the text a literal holds, with what it interpolates left out.
fn static_string_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match node.kind_str() {
        "string" if !has_interpolation(node) => Some(string_value(node, context)),
        "string" => {
            let mut text = String::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind_str() != "interpolation" {
                    text.push_str(&string_value(node, context));
                    break;
                }
            }
            Some(text)
        }
        "chained_string" => {
            let mut text = String::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind_str() == "string" && !has_interpolation(child) {
                    text.push_str(&string_value(child, context));
                }
            }
            Some(text)
        }
        // A heredoc's text is written under the statement rather than inside the opener.
        "heredoc_beginning" => {
            let body = heredoc_body(node, context)?;
            let mut text = String::new();
            let mut cursor = body.walk();
            for child in body.named_children(&mut cursor) {
                if child.kind_str() == "heredoc_content" {
                    text.push_str(context.source.node_text(child));
                }
            }
            Some(text)
        }
        _ => None,
    }
}

/// `detect_unnecessary_fields`.
fn detect_unnecessary_fields(
    context: &RuleContext<'_>,
    call: Node<'_>,
    call_arguments: &[Argument<'_>],
    offenses: &mut Vec<Offense>,
) {
    let Some((first, rest)) = call_arguments.split_first() else {
        return;
    };
    let [template] = first.parts() else {
        return;
    };
    if template.kind_str() != "string" || has_interpolation(*template) || rest.is_empty() {
        return;
    }
    // `$str`: a template written over lines is a `dstr` of one `str` per line, which the pattern
    // does not match.
    if crate::rules::support::quoted_spans_lines(context.source.node_text(*template)) {
        return;
    }
    if splatted_arguments(rest) {
        return;
    }
    let text = string_value(*template, context);
    let Some(values) = argument_values(context, rest) else {
        return;
    };
    if !all_fields_literal(context, &text, rest, &values) {
        return;
    }
    let Some(formatted) = apply_format(&text, &values) else {
        return;
    };
    let replacement = quote(context, *template, call, &formatted);
    offenses.push(report(context, call, replacement));
}

/// `splatted_arguments?`.
fn splatted_arguments(rest: &[Argument<'_>]) -> bool {
    rest.iter().any(|argument| {
        argument
            .parts()
            .iter()
            .any(|part| matches!(part.kind_str(), "splat_argument" | "hash_splat_argument"))
            || argument.parts().iter().any(|part| {
                part.kind_str() == "hash"
                    && super::nodes::children(*part)
                        .iter()
                        .any(|pair| pair.kind_str() == "hash_splat_argument")
            })
    })
}

/// `all_fields_literal?`.
fn all_fields_literal(
    context: &RuleContext<'_>,
    text: &str,
    rest: &[Argument<'_>],
    values: &[Value],
) -> bool {
    let found = sequences(text);
    if found.is_empty() {
        return false;
    }
    let mut pending: Vec<usize> = (0..rest.len()).collect();
    let hash = values
        .iter()
        .position(|value| matches!(value, Value::Pairs(_)));
    let mut count = 0;
    for sequence in &found {
        if sequence.style == SequenceStyle::Percent {
            continue;
        }
        if unknown_variable_width(sequence, &pending, values) {
            continue;
        }
        // `%999…$d`: upstream raises on an argument number no argument answers, and a raising
        // `format` is not a redundant one. Reading the unparsable number as "no number given"
        // instead made the cop fold the call to whatever the first argument printed as.
        if unreadable_argument_number(sequence) {
            return false;
        }
        // `format('%{y}', 2015)` raises `ArgumentError: one hash required`. A named sequence has
        // nothing to read without a hash to read it from, so there is no value to fold to.
        if hash.is_none()
            && matches!(
                sequence.style,
                SequenceStyle::Annotated | SequenceStyle::Template
            )
        {
            return false;
        }
        let Some(index) = find_argument(sequence, &mut pending, hash, values) else {
            continue;
        };
        let Some(value) = value_at(values, hash, sequence, index) else {
            continue;
        };
        if !matching_argument(sequence, value) {
            continue;
        }
        // `(sequence.width || sequence.precision) && argument.dstr_type?`.
        if (!sequence.width.is_empty() || !sequence.precision.is_empty())
            && is_interpolated(context, rest, index)
        {
            continue;
        }
        count += 1;
    }
    found.len() == count && !mixes_numbered_and_unnumbered(&found)
}

/// `format` raises `ArgumentError` for a string that names one argument by position and leaves
/// another to the order it was written in, and upstream reports nothing where it would have raised.
/// A `*` width takes an argument of its own, so it has a mode of its own too.
fn mixes_numbered_and_unnumbered(found: &[Sequence]) -> bool {
    let (mut numbered, mut sequential) = (false, false);
    for sequence in found {
        if sequence.style == SequenceStyle::Percent {
            continue;
        }
        if is_variable_width(sequence) {
            match sequence
                .width
                .strip_prefix('*')
                .is_some_and(has_argument_number)
            {
                true => numbered = true,
                false => sequential = true,
            }
        }
        if matches!(
            sequence.style,
            SequenceStyle::Annotated | SequenceStyle::Template
        ) {
            continue;
        }
        match argument_number(sequence).is_some() {
            true => numbered = true,
            false => sequential = true,
        }
    }
    numbered && sequential
}

/// Whether a `*` width names the argument it takes its number from.
fn has_argument_number(rest: &str) -> bool {
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    !digits.is_empty() && rest[digits.len()..].starts_with('$')
}

/// `find_argument`, which consumes the positional arguments as it goes.
fn find_argument(
    sequence: &Sequence,
    pending: &mut Vec<usize>,
    hash: Option<usize>,
    values: &[Value],
) -> Option<usize> {
    if hash.is_some()
        && matches!(
            sequence.style,
            SequenceStyle::Annotated | SequenceStyle::Template
        )
    {
        return hash;
    }
    if is_variable_width(sequence) {
        let number = variable_width_argument_number(sequence)?;
        let index = number.checked_sub(1)?;
        if index < pending.len() {
            pending.remove(index);
        }
        return (!pending.is_empty()).then(|| pending.remove(0));
    }
    if let Some(number) = argument_number(sequence) {
        let index = number.checked_sub(1)?;
        return pending.get(index).copied();
    }
    let _ = values;
    (!pending.is_empty()).then(|| pending.remove(0))
}

/// The value a sequence was matched with, which for a name is the one the hash holds under it.
fn value_at<'a>(
    values: &'a [Value],
    hash: Option<usize>,
    sequence: &Sequence,
    index: usize,
) -> Option<&'a Value> {
    if hash == Some(index)
        && matches!(
            sequence.style,
            SequenceStyle::Annotated | SequenceStyle::Template
        )
    {
        let Value::Pairs(pairs) = values.get(index)? else {
            return None;
        };
        let name = sequence.name.as_ref()?;
        return pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value);
    }
    values.get(index)
}

/// `matching_argument?`.
fn matching_argument(sequence: &Sequence, value: &Value) -> bool {
    if sequence.style == SequenceStyle::Template {
        return acceptable_literal(value);
    }
    match sequence.kind {
        Some('s') => acceptable_literal(value),
        Some('d' | 'i' | 'u') => as_integer(value).is_some(),
        Some('f') => as_float(value).is_some(),
        _ => false,
    }
}

/// `ACCEPTABLE_LITERAL_TYPES`.
fn acceptable_literal(value: &Value) -> bool {
    !matches!(value, Value::Pairs(_) | Value::Computed(_))
}

/// `numeric?` together with `Integer(value, exception: false)`.
fn as_integer(value: &Value) -> Option<i128> {
    match value {
        Value::Integer(number) => Some(*number),
        // `Integer(Rational(3, 4))` is 0: it truncates just as `Integer(0.75)` does.
        Value::Rational(numerator, denominator) if *denominator != 0 => {
            Some(numerator / denominator)
        }
        Value::Float(number) => Some(number.trunc() as i128),
        // `Integer('0x10')` is 16: the prefixes count, which a plain parse would refuse.
        Value::Text(text) => parse_integer(text.trim()),
        // `Integer((5+0i))` is 5: a complex whose imaginary part is zero converts, and one whose
        // is not raises. `Value::Complex` keeps what the literal prints as, which is where the
        // two halves are read from.
        Value::Complex(text) => real_part_of_complex(text),
        Value::Computed(inner) => as_integer(inner),
        _ => None,
    }
}

/// The real part of a complex literal whose imaginary part is zero, as `Integer()` reads it.
fn real_part_of_complex(text: &str) -> Option<i128> {
    let body = text.trim();
    let imaginary = body.rfind(['+', '-']).filter(|index| *index > 0)?;
    if !body.ends_with('i') {
        return None;
    }
    let tail = &body[imaginary + 1..body.len() - 1];
    // `(5+0i)` converts, `(5+1i)` does not.
    if tail.parse::<f64>().ok()? != 0.0 {
        return None;
    }
    parse_integer(body[..imaginary].trim())
}

/// `numeric?` together with `Float(value, exception: false)`.
fn as_float(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(number) => Some(*number as f64),
        Value::Rational(numerator, denominator) => Some(*numerator as f64 / *denominator as f64),
        Value::Float(number) => Some(*number),
        Value::Text(text) => text.trim().parse::<f64>().ok(),
        Value::Complex(text) => real_part_of_complex(text).map(|number| number as f64),
        Value::Computed(inner) => as_float(inner),
        _ => None,
    }
}

/// `unknown_variable_width?`.
fn unknown_variable_width(sequence: &Sequence, pending: &[usize], values: &[Value]) -> bool {
    if !is_variable_width(sequence) {
        return false;
    }
    let Some(number) = variable_width_argument_number(sequence) else {
        return true;
    };
    let Some(index) = number.checked_sub(1).and_then(|index| pending.get(index)) else {
        return true;
    };
    !matches!(
        values.get(*index),
        Some(Value::Integer(_) | Value::Float(_) | Value::Text(_))
    )
}

fn is_variable_width(sequence: &Sequence) -> bool {
    sequence.width.starts_with('*')
}

/// The `N` of a `%*N$d`, or the position that follows when none was written.
fn variable_width_argument_number(sequence: &Sequence) -> Option<usize> {
    match sequence.width.strip_prefix('*') {
        Some(rest) => match rest.strip_suffix('$') {
            Some(number) => number.parse().ok(),
            None => Some(1),
        },
        None => None,
    }
}

/// The `N` of a `%N$s`.
fn argument_number(sequence: &Sequence) -> Option<usize> {
    sequence
        .flags
        .split('$')
        .next()
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .filter(|_| sequence.flags.contains('$'))
        .and_then(|digits| digits.parse().ok())
}

/// Whether the argument at that position was written as an interpolated string.
fn is_interpolated(context: &RuleContext<'_>, rest: &[Argument<'_>], index: usize) -> bool {
    let _ = context;
    rest.get(index).is_some_and(|argument| {
        matches!(argument.parts(), [only]
            if only.kind_str() == "chained_string"
                || (only.kind_str() == "string" && has_interpolation(*only)))
    })
}

/// `argument_values`.
fn argument_values(context: &RuleContext<'_>, rest: &[Argument<'_>]) -> Option<Vec<Value>> {
    let mut values = Vec::new();
    for argument in rest {
        // A brace-less hash reaches here as its pairs, which upstream had already folded into one
        // `hash` argument.
        if argument
            .parts()
            .iter()
            .all(|part| part.kind_str() == "pair")
        {
            let mut held = Vec::new();
            for pair in argument.parts() {
                held.push(pair_value(context, *pair)?);
            }
            values.push(Value::Pairs(held));
            continue;
        }
        match argument.parts() {
            [only] => values.push(argument_value(context, *only)?),
            _ => return None,
        }
    }
    Some(values)
}

fn pair_value(context: &RuleContext<'_>, pair: Node<'_>) -> Option<(String, Value)> {
    if pair.kind_str() != "pair" {
        return None;
    }
    let key = pair.field("key")?;
    let name = symbol_name(key, context).map(str::to_owned).or_else(|| {
        (key.kind_str() == "string" && !has_interpolation(key)).then(|| string_value(key, context))
    })?;
    Some((name, argument_value(context, pair.field("value")?)?))
}

/// `argument_value`, for the literals this cop can reproduce.
fn argument_value(context: &RuleContext<'_>, node: Node<'_>) -> Option<Value> {
    let node = match node.kind_str() {
        "parenthesized_statements" => *super::nodes::children(node).first()?,
        _ => node,
    };
    let text = context.source.node_text(node);
    match node.kind_str() {
        "nil" => Some(Value::Nil),
        "true" => Some(Value::Boolean(true)),
        "false" => Some(Value::Boolean(false)),
        "integer" => parse_integer(text).map(Value::Integer),
        "float" => text.replace('_', "").parse().ok().map(Value::Float),
        "string" if !has_interpolation(node) => Some(Value::Text(string_value(node, context))),
        "string" | "chained_string" => Some(Value::Dynamic(dstr_value(node, context))),
        // The parser folds a leading sign into the literal it precedes.
        "unary" => {
            let operator = node.field("operator")?;
            let operand = node.field("operand")?;
            let sign = context.source.node_text(operator);
            if !matches!(sign, "-" | "+") || !matches!(operand.kind_str(), "integer" | "float") {
                return None;
            }
            match argument_value(context, operand)? {
                Value::Integer(number) if sign == "-" => Some(Value::Integer(-number)),
                Value::Float(number) if sign == "-" => Some(Value::Float(-number)),
                value => Some(value),
            }
        }
        "simple_symbol" | "hash_key_symbol" | "bare_symbol" => {
            symbol_name(node, context).map(|name| Value::Text(name.to_owned()))
        }
        "delimited_symbol" if !has_interpolation(node) => {
            symbol_name(node, context).map(|name| Value::Text(name.to_owned()))
        }
        // `dsym_value`: a symbol written with an interpolation formats to what its own text reads
        // as, so `:"#{foo}"` carries the same value the string `"#{foo}"` does.
        "delimited_symbol" => Some(Value::Dynamic(dstr_value(node, context))),
        // `?a`, which the parser resolves into the one-character string it stands for.
        "character" => Some(Value::Text(crate::rules::ruby_literal::character_value(
            text,
        ))),
        "rational" => rational_value(text).map(|(num, den)| Value::Rational(num, den)),
        // `rational_number?` and `complex_number?`: the two shapes upstream reads as one number even
        // though the parser keeps them as calls.
        "binary" => computed_number(context, node),
        "complex" => complex_value(text).map(Value::Complex),
        "hash" => {
            let mut held = Vec::new();
            for pair in super::nodes::children(node) {
                held.push(pair_value(context, pair)?);
            }
            Some(Value::Pairs(held))
        }
        _ => None,
    }
}

/// `{rational (send int :/ rational)}` and `{complex (send int :+ complex)}`, for the halves written
/// as a call.
fn computed_number(context: &RuleContext<'_>, node: Node<'_>) -> Option<Value> {
    let operator = node.field("operator")?;
    let (left, right) = (node.field("left")?, node.field("right")?);
    if left.kind_str() != "integer" {
        return None;
    }
    let left_text = context.source.node_text(left);
    let right_text = context.source.node_text(right);
    match context.source.node_text(operator) {
        "/" if right.kind_str() == "rational" => {
            let numerator = parse_integer(left_text)?;
            let (denominator, _) = rational_value(right_text)?;
            if denominator == 0 {
                return None;
            }
            let divisor =
                greatest_common_divisor(numerator.unsigned_abs(), denominator.unsigned_abs());
            let divisor = i128::try_from(divisor.max(1)).ok()?;
            Some(Value::Computed(Box::new(Value::Rational(
                numerator / divisor,
                denominator / divisor,
            ))))
        }
        "+" if right.kind_str() == "complex" => {
            let imaginary = complex_value(right_text)?;
            let sign = match imaginary.starts_with('-') {
                true => "",
                false => "+",
            };
            // Only the `0+` real part that `complex_value` writes comes off. Trimming a further
            // `0` turned `(5+0i)` into `5+i`, which is `5+1i` -- and `Integer()` then refused it,
            // because a complex with a non-zero imaginary part does not convert.
            let imaginary = imaginary.trim_start_matches("0+");
            Some(Value::Computed(Box::new(Value::Complex(format!(
                "{left_text}{sign}{imaginary}"
            )))))
        }
        _ => None,
    }
}

/// `DstrNode#value`: the parts that hold text contribute what they stand for, and what is
/// interpolated contributes the source it was written as.
fn dstr_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    if node.kind_str() == "chained_string" {
        return super::nodes::children(node)
            .into_iter()
            .map(|child| dstr_value(child, context))
            .collect();
    }
    if !has_interpolation(node) {
        return string_value(node, context);
    }
    let mut value = String::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind_str() {
            "string_content" => value.push_str(context.source.node_text(child)),
            "escape_sequence" => unescape(context.source.node_text(child), &mut value),
            "interpolation" => value.push_str(context.source.node_text(child)),
            _ => {}
        }
    }
    value
}

/// `rational_value`: what `3r` and `0.5r` reduce to.
fn rational_value(text: &str) -> Option<(i128, i128)> {
    let body = text.strip_suffix('r')?.replace('_', "");
    let (numerator, denominator) = match body.split_once('.') {
        None => (body.parse::<i128>().ok()?, 1),
        Some((whole, fraction)) => {
            let scale = 10_i128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
            let digits = format!("{whole}{fraction}").parse::<i128>().ok()?;
            (digits, scale)
        }
    };
    let divisor = greatest_common_divisor(numerator.unsigned_abs(), denominator.unsigned_abs());
    let divisor = i128::try_from(divisor.max(1)).ok()?;
    Some((numerator / divisor, denominator / divisor))
}

fn greatest_common_divisor(left: u128, right: u128) -> u128 {
    match right {
        0 => left,
        _ => greatest_common_divisor(right, left % right),
    }
}

/// `complex_value`: what `1i` prints as, which spells out the real part it never had.
fn complex_value(text: &str) -> Option<String> {
    let body = text.strip_suffix('i')?.replace('_', "");
    match body.contains('.') {
        true => Some(format!("0.0+{}i", body.parse::<f64>().ok()?)),
        false => Some(format!("0+{}i", body.parse::<i128>().ok()?)),
    }
}

/// A Ruby integer literal, in any of its bases.
fn parse_integer(text: &str) -> Option<i128> {
    let text = text.replace('_', "");
    let (radix, digits) = match text.get(..2) {
        Some("0x" | "0X") => (16, &text[2..]),
        Some("0b" | "0B") => (2, &text[2..]),
        Some("0o" | "0O") => (8, &text[2..]),
        _ => (10, text.as_str()),
    };
    i128::from_str_radix(digits, radix).ok()
}

/// `format(string, *arguments)`, for the sequences `matching_argument?` lets through.
fn apply_format(text: &str, values: &[Value]) -> Option<String> {
    let found = sequences(text);
    let hash = values
        .iter()
        .position(|value| matches!(value, Value::Pairs(_)));
    let mut pending: Vec<usize> = (0..values.len()).collect();
    let mut out = String::new();
    let mut cursor = 0;
    for sequence in &found {
        out.push_str(text.get(cursor..sequence.begin)?);
        cursor = sequence.end;
        if sequence.style == SequenceStyle::Percent {
            out.push('%');
            continue;
        }
        let width = match is_variable_width(sequence) {
            true => {
                let number = variable_width_argument_number(sequence)?;
                let index = number.checked_sub(1)?;
                let taken = *pending.get(index)?;
                pending.remove(index);
                Some(as_integer(values.get(taken)?)? as isize)
            }
            false => sequence.width.parse::<isize>().ok(),
        };
        // `%.*f` takes its precision from an argument too, and consumes it before the value.
        let precision = match sequence.precision.starts_with('*') {
            true => {
                let taken = *pending.first()?;
                pending.remove(0);
                Some(as_integer(values.get(taken)?)?.max(0) as usize)
            }
            false => None,
        };
        let index = match hash.is_some()
            && matches!(
                sequence.style,
                SequenceStyle::Annotated | SequenceStyle::Template
            ) {
            true => hash?,
            false => match argument_number(sequence) {
                Some(number) => *pending.get(number.checked_sub(1)?)?,
                None => {
                    let taken = *pending.first()?;
                    pending.remove(0);
                    taken
                }
            },
        };
        let value = value_at(values, hash, sequence, index)?;
        out.push_str(&render(sequence, value, width, precision)?);
    }
    out.push_str(text.get(cursor..)?);
    Some(out)
}

/// One formatted field.
fn render(
    sequence: &Sequence,
    value: &Value,
    width: Option<isize>,
    from_argument: Option<usize>,
) -> Option<String> {
    let precision: Option<usize> = match from_argument {
        Some(precision) => Some(precision),
        None => match (sequence.has_precision, sequence.precision.is_empty()) {
            (false, _) => None,
            // `%.d`: the dot with no digits behind it asks for a precision of zero.
            (true, true) => Some(0),
            (true, false) => Some(sequence.precision.parse().ok()?),
        },
    };
    let kind = match sequence.style {
        SequenceStyle::Template => 's',
        _ => sequence.kind?,
    };
    let body = match kind {
        's' => {
            let mut text = to_s(value);
            if let Some(precision) = precision {
                text = text.chars().take(precision).collect();
            }
            text
        }
        'd' | 'i' | 'u' => {
            let number = as_integer(value)?;
            let mut digits = number.unsigned_abs().to_string();
            if let Some(precision) = precision {
                // `format('%.d', 0)` is `''`: a precision of zero asks for no digits at all, and
                // zero has none to keep. Padding up to the precision left the `0` in place.
                if precision == 0 && number == 0 {
                    digits.clear();
                }
                while digits.len() < precision {
                    digits.insert(0, '0');
                }
            }
            // `0` pads between the sign and the digits, and a precision turns the flag off.
            render_signed_number(
                digits,
                number < 0,
                &sequence.flags,
                width,
                precision.is_none(),
            )
        }
        'f' => {
            let number = as_float(value)?;
            let precision = precision.unwrap_or(6);
            render_signed_number(
                format!("{:.*}", precision, number.abs()),
                number.is_sign_negative(),
                &sequence.flags,
                width,
                true,
            )
        }
        _ => return None,
    };
    let Some(width) = width else {
        return Some(body);
    };
    let (left, width) = match width < 0 {
        true => (true, width.unsigned_abs()),
        false => (sequence.flags.contains('-'), width as usize),
    };
    let length = body.chars().count();
    if length >= width {
        return Some(body);
    }
    let padding = " ".repeat(width - length);
    Some(match left {
        true => format!("{body}{padding}"),
        false => format!("{padding}{body}"),
    })
}

fn render_signed_number(
    mut magnitude: String,
    negative: bool,
    flags: &str,
    width: Option<isize>,
    allow_zero_padding: bool,
) -> String {
    let sign = numeric_sign(negative, flags);
    if allow_zero_padding
        && flags.contains('0')
        && !flags.contains('-')
        && let Some(width) = width
        && width > 0
    {
        let width = width as usize;
        while sign.len() + magnitude.len() < width {
            magnitude.insert(0, '0');
        }
    }
    format!("{sign}{magnitude}")
}

fn numeric_sign(negative: bool, flags: &str) -> &'static str {
    match (negative, flags.contains('+'), flags.contains(' ')) {
        (true, _, _) => "-",
        (false, true, _) => "+",
        (false, false, true) => " ",
        _ => "",
    }
}

/// `value.to_s`.
fn to_s(value: &Value) -> String {
    match value {
        Value::Text(text) | Value::Dynamic(text) => text.clone(),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) => match number.fract() == 0.0 && number.abs() < 1e16 {
            true => format!("{number:.1}"),
            false => number.to_string(),
        },
        Value::Rational(numerator, denominator) => format!("{numerator}/{denominator}"),
        Value::Complex(text) => text.clone(),
        Value::Boolean(flag) => flag.to_string(),
        Value::Nil => String::new(),
        Value::Pairs(_) => String::new(),
        Value::Computed(inner) => to_s(inner),
    }
}

/// `quote`: the result keeps the delimiters the template was written with.
fn quote(context: &RuleContext<'_>, template: Node<'_>, call: Node<'_>, formatted: &str) -> String {
    let mut cursor = template.walk();
    let delimiters: Vec<Node<'_>> = template
        .children(&mut cursor)
        .filter(|child| !child.is_named())
        .collect();
    let mut opening = delimiters.first().map_or("'".to_owned(), |token| {
        context.source.node_text(*token).to_owned()
    });
    let mut closing = delimiters.last().map_or("'".to_owned(), |token| {
        context.source.node_text(*token).to_owned()
    });
    // An interpolated argument means the result has to stay interpolatable.
    if has_interpolated_descendant(call) {
        if opening == "'" {
            opening = "\"".to_owned();
            closing = "\"".to_owned();
        } else if let Some(rest) = opening.strip_prefix("%q") {
            opening = format!("%Q{rest}");
        }
    }
    format!("{opening}{}{closing}", escape_control_chars(formatted))
}

/// `node.each_descendant(:dstr, :dsym).any?`.
fn has_interpolated_descendant(call: Node<'_>) -> bool {
    let mut stack: Vec<Node<'_>> = Vec::new();
    crate::rules::push_named_children(call, &mut stack);
    while let Some(node) = stack.pop() {
        if matches!(node.kind_str(), "chained_string")
            || (matches!(node.kind_str(), "string" | "delimited_symbol") && has_interpolation(node))
        {
            return true;
        }
        crate::rules::push_named_children(node, &mut stack);
    }
    false
}

/// `escape_control_chars`.
fn escape_control_chars(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\x0b' => out.push_str("\\v"),
            '\x0c' => out.push_str("\\f"),
            '\x08' => out.push_str("\\b"),
            '\x07' => out.push_str("\\a"),
            '\x1b' => out.push_str("\\e"),
            '\0' => out.push_str("\\0"),
            character if (character as u32) < 0x20 || character as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02X}", character as u32));
            }
            character => out.push(character),
        }
    }
    out
}

/// The offence and the rewrite it carries.
fn report(context: &RuleContext<'_>, call: Node<'_>, replacement: String) -> Offense {
    let name = call.field("method").map_or_else(String::new, |method| {
        context.source.node_text(method).to_owned()
    });
    let range = send_range(call, context);
    context
        .offense(
            format!("Use `{replacement}` directly instead of `{name}`."),
            range.clone(),
        )
        .corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement,
            safe: true,
        })
}

/// A `%N$` whose `N` no `usize` holds -- upstream raises rather than formatting.
fn unreadable_argument_number(sequence: &Sequence) -> bool {
    sequence.flags.contains('$') && argument_number(sequence).is_none()
}
