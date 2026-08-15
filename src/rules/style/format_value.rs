//! What `Kernel#format` makes of a literal argument, for the one cop that has to work the answer out
//! rather than leave it to run time.
//!
//! `Style/RedundantFormat` reports `format('%05d', 42)` because the string it builds is known while
//! the file is being read. Working it out means reading each argument as the value Ruby would pass and
//! then filling the fields the way `sprintf` does -- but only for the four field types the cop accepts
//! an argument for (`s`, `d`/`i`/`u`, `f`), since a field of any other type is never reported.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal::{string_value, unescape_text};
use crate::rules::send_node::named_children;

use super::format_sequences::{Sequence, SequenceStyle};

/// The value a literal argument stands for.
#[derive(Clone, Debug)]
pub(super) enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Symbol(String),
    Bool(bool),
    Nil,
    /// A numerator and a denominator, already reduced.
    Rational(i64, i64),
    /// A real and an imaginary part, each kept as it was written so that `to_s` reads back the same.
    Complex(String, String),
}

/// One field of the format string, with the width a `*` took from an argument of its own.
pub(super) struct Field {
    pub(super) value: Value,
    pub(super) width: Option<i64>,
}

impl Value {
    /// The value the node stands for, or nothing when it is no literal `format` could be handed.
    pub(super) fn of(node: Node<'_>, context: &RuleContext<'_>) -> Option<Self> {
        // `argument = argument.children.first if argument.begin_type?`
        let node = match node.kind_str() {
            "parenthesized_statements" => match named_children(node).as_slice() {
                [single] => *single,
                _ => return None,
            },
            _ => node,
        };
        match node.kind_str() {
            "string" => Some(Self::Str(text_of(node, context))),
            // Literals written side by side are one `dstr`, whose value is what they spell together.
            "chained_string" => Some(Self::Str(
                named_children(node)
                    .into_iter()
                    .map(|part| text_of(part, context))
                    .collect(),
            )),
            "simple_symbol" => Some(Self::Symbol(
                context
                    .source
                    .node_text(node)
                    .trim_start_matches(':')
                    .to_owned(),
            )),
            "delimited_symbol" => Some(Self::Symbol(symbol_text(node, context))),
            "integer" => integer_text(context.source.node_text(node)).map(Self::Int),
            "float" => float_text(context.source.node_text(node)).map(Self::Float),
            "rational" => rational_of(context.source.node_text(node)),
            "complex" => complex_of(context.source.node_text(node), None),
            "true" => Some(Self::Bool(true)),
            "false" => Some(Self::Bool(false)),
            "nil" => Some(Self::Nil),
            // A sign belongs to the number upstream's parser folds it into.
            "unary" => signed(node, context),
            // `x / 2r` and `1 + 2i`, which the parser keeps as calls and the cop reads as one number.
            "binary" => combined(node, context),
            _ => None,
        }
    }

    /// `Integer(value, exception: false)`, which only a number or a string of one answers.
    pub(super) fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            // `Integer(1.9)` truncates towards zero.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "`Integer(1.9)` truncates in Ruby as well"
            )]
            Self::Float(value) => value.is_finite().then_some(*value as i64),
            Self::Str(text) => integer_text(text.trim()),
            // `Integer(Rational(3, 4))` is 0: it truncates rather than refusing.
            Self::Rational(numerator, denominator) => {
                (*denominator != 0).then(|| numerator / denominator)
            }
            Self::Complex(real, imaginary) => {
                (imaginary.trim_start_matches(['+', '-']) == "0").then(|| integer_text(real))?
            }
            _ => None,
        }
    }

    /// `Float(value, exception: false)`.
    pub(super) fn as_float(&self) -> Option<f64> {
        match self {
            Self::Int(value) => Some(*value as f64),
            Self::Float(value) => Some(*value),
            Self::Str(text) => float_text(text.trim())
                .or_else(|| integer_text(text.trim()).map(|integer| integer as f64)),
            #[expect(
                clippy::cast_precision_loss,
                reason = "Ruby's Float(Rational) loses the same precision"
            )]
            Self::Rational(numerator, denominator) => Some(*numerator as f64 / *denominator as f64),
            Self::Complex(real, imaginary) => (imaginary.trim_start_matches(['+', '-']) == "0")
                .then(|| {
                    float_text(real).or_else(|| integer_text(real).map(|value| value as f64))
                })?,
            _ => None,
        }
    }

    /// `to_s`, which is what a `%s` writes.
    pub(super) fn to_text(&self) -> String {
        match self {
            Self::Str(text) => text.clone(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => float_to_text(*value),
            Self::Symbol(name) => name.clone(),
            Self::Bool(value) => value.to_string(),
            // `format('%s', nil)` builds an empty string rather than the word.
            Self::Nil => String::new(),
            Self::Rational(numerator, denominator) => format!("{numerator}/{denominator}"),
            Self::Complex(real, imaginary) => format!("{real}{imaginary}"),
        }
    }
}

/// `format(string, *values)` for the fields the cop accepts.
pub(super) fn format_with(string: &str, found: &[Sequence], fields: &[Field]) -> Option<String> {
    let mut out = String::new();
    let mut copied = 0;
    for (sequence, field) in found.iter().zip(fields) {
        out.push_str(string.get(copied..sequence.begin)?);
        copied = sequence.end;
        if sequence.style == SequenceStyle::Percent {
            out.push('%');
            continue;
        }
        out.push_str(&render(sequence, field)?);
    }
    out.push_str(string.get(copied..)?);
    Some(out)
}

/// One field, filled and padded.
fn render(sequence: &Sequence, field: &Field) -> Option<String> {
    let precision = precision_of(sequence);
    let body = match sequence.kind {
        // A template field gives no type and writes the value as it stands.
        None => field.value.to_text(),
        Some('s') => truncated(field.value.to_text(), precision),
        Some('d' | 'i' | 'u') => integer_body(field.value.as_integer()?, sequence, precision),
        Some('f') => float_body(field.value.as_float()?, sequence, precision.unwrap_or(6)),
        _ => return None,
    };
    Some(padded(body, sequence, field))
}

/// `%.Ns` cuts the text down to `N` characters.
fn truncated(text: String, precision: Option<usize>) -> String {
    match precision {
        Some(precision) => text.chars().take(precision).collect(),
        None => text,
    }
}

/// The digits of an integer field, with the sign the flags ask for and the minimum width a precision
/// sets.
fn integer_body(value: i64, sequence: &Sequence, precision: Option<usize>) -> String {
    let digits = value.unsigned_abs().to_string();
    let digits = match precision {
        Some(precision) if digits.len() < precision => {
            format!("{}{digits}", "0".repeat(precision - digits.len()))
        }
        _ => digits,
    };
    format!("{}{digits}", sign_of(value < 0, sequence))
}

/// The digits of a float field, which a precision counts after the point and defaults to six.
fn float_body(value: f64, sequence: &Sequence, precision: usize) -> String {
    let digits = format!("{:.*}", precision, value.abs());
    format!("{}{digits}", sign_of(value.is_sign_negative(), sequence))
}

/// `-` for a negative value, and `+` or a space in front of a positive one where the flags say so.
fn sign_of(negative: bool, sequence: &Sequence) -> &'static str {
    if negative {
        return "-";
    }
    if sequence.flags.contains('+') {
        return "+";
    }
    if sequence.flags.contains(' ') {
        return " ";
    }
    ""
}

/// The width the field is padded out to: `-` and a negative `*` justify to the left, and `0` pads a
/// number with zeros as long as no precision already did.
fn padded(body: String, sequence: &Sequence, field: &Field) -> String {
    let Some(width) = width_of(sequence, field) else {
        return body;
    };
    let left = sequence.flags.contains('-') || width < 0;
    let width = width.unsigned_abs() as usize;
    let length = body.chars().count();
    if length >= width {
        return body;
    }
    let fill = width - length;
    if left {
        return format!("{body}{}", " ".repeat(fill));
    }
    // A precision on an integer field already says how many digits to write, so the `0` flag has
    // nothing left to do; on a float it still pads the whole field out.
    let zeros = sequence.flags.contains('0')
        && match sequence.kind {
            Some('d' | 'i' | 'u') => sequence.precision.is_empty(),
            Some('f') => true,
            _ => false,
        };
    if !zeros {
        return format!("{}{body}", " ".repeat(fill));
    }
    // The sign stays in front of the zeros it pads.
    let (sign, digits) = match body.starts_with(['-', '+', ' ']) {
        true => body.split_at(1),
        false => ("", body.as_str()),
    };
    format!("{sign}{}{digits}", "0".repeat(fill))
}

/// The width a field asks for, whether written out or taken from an argument.
fn width_of(sequence: &Sequence, field: &Field) -> Option<i64> {
    if sequence.width.starts_with('*') {
        return field.width;
    }
    sequence.width.parse().ok()
}

/// The `precision` capture, where an empty one after the point means none at all.
fn precision_of(sequence: &Sequence) -> Option<usize> {
    match sequence.precision.as_str() {
        "" => None,
        digits => digits.parse().ok(),
    }
}

/// `Integer(text, exception: false)` for the spellings a literal or a string of one can take.
fn integer_text(text: &str) -> Option<i64> {
    let text = text.trim();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let digits: String = digits
        .chars()
        .filter(|character| *character != '_')
        .collect();
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        return i64::from_str_radix(hex, 16).ok().map(|value| sign * value);
    }
    if let Some(binary) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        return i64::from_str_radix(binary, 2)
            .ok()
            .map(|value| sign * value);
    }
    // `Integer('1.5')` raises rather than truncating, so only whole digits answer here.
    digits.parse::<i64>().ok().map(|value| sign * value)
}

/// `Float(text, exception: false)`.
fn float_text(text: &str) -> Option<f64> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|character| *character != '_')
        .collect();
    cleaned.parse().ok()
}

/// `Float#to_s`, which keeps a point on a whole number where Rust's own writing drops it.
fn float_to_text(value: f64) -> String {
    if !value.is_finite() {
        return match (value.is_nan(), value.is_sign_negative()) {
            (true, _) => "NaN".to_owned(),
            (false, true) => "-Infinity".to_owned(),
            (false, false) => "Infinity".to_owned(),
        };
    }
    let written = format!("{value}");
    match written.contains(['.', 'e', 'E']) {
        true => written,
        false => format!("{written}.0"),
    }
}

/// `StrNode#value` and `DstrNode#value`: what the literal holds, with an interpolation written out as
/// it stands. Upstream cannot resolve one either, and reads its source in the value's place.
fn text_of(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let children = named_children(node);
    if !children
        .iter()
        .any(|child| child.kind_str() == "interpolation")
    {
        return string_value(node, context);
    }
    // A string that interpolates is never single-quoted, so every escape written in it resolves.
    children
        .into_iter()
        .map(|child| match child.kind_str() {
            "escape_sequence" => unescape_text(context.source.node_text(child)),
            _ => context.source.node_text(child).to_owned(),
        })
        .collect()
}

/// `:"name"`: the text a quoted symbol holds.
fn symbol_text(node: Node<'_>, context: &RuleContext<'_>) -> String {
    named_children(node)
        .into_iter()
        .map(|child| context.source.node_text(child))
        .collect()
}

/// `1r` and `3/4r`, reduced the way `Rational` reduces.
fn rational_of(text: &str) -> Option<Value> {
    let digits = text.strip_suffix('r')?;
    let numerator = integer_text(digits)?;
    Some(reduced(numerator, 1))
}

fn reduced(numerator: i64, denominator: i64) -> Value {
    let divisor = gcd(numerator.unsigned_abs(), denominator.unsigned_abs()).max(1);
    #[expect(
        clippy::cast_possible_wrap,
        reason = "the divisor came out of these two"
    )]
    let divisor = divisor as i64;
    Value::Rational(numerator / divisor, denominator / divisor)
}

fn gcd(left: u64, right: u64) -> u64 {
    match right {
        0 => left,
        _ => gcd(right, left % right),
    }
}

/// `1i` and `2i`, whose real part is zero unless a `+` put one there.
fn complex_of(text: &str, real: Option<&str>) -> Option<Value> {
    let digits = text.strip_suffix('i')?;
    let imaginary = match digits.starts_with('-') {
        true => digits.to_owned(),
        false => format!("+{digits}"),
    };
    Some(Value::Complex(
        real.unwrap_or("0").to_owned(),
        format!("{imaginary}i"),
    ))
}

/// A signed number, which the parser folds into the literal it was written on.
fn signed(node: Node<'_>, context: &RuleContext<'_>) -> Option<Value> {
    let operator = node.field("operator")?;
    let sign = context.source.node_text(operator);
    if !matches!(sign, "-" | "+") {
        return None;
    }
    let operand = node.field("operand")?;
    let text = format!("{sign}{}", context.source.node_text(operand));
    match operand.kind_str() {
        "integer" => integer_text(&text).map(Value::Int),
        "float" => float_text(&text).map(Value::Float),
        "rational" => rational_of(&text),
        "complex" => complex_of(&text, None),
        _ => None,
    }
}

/// `(send int :/ rational)` and `(send int :+ complex)`: the two shapes the cop reads as one number.
fn combined(node: Node<'_>, context: &RuleContext<'_>) -> Option<Value> {
    let operator = node.field("operator")?;
    let (left, right) = (node.field("left")?, node.field("right")?);
    match context.source.node_text(operator) {
        "/" if right.kind_str() == "rational" => {
            let numerator = integer_text(context.source.node_text(left))?;
            let denominator = integer_text(context.source.node_text(right).strip_suffix('r')?)?;
            (denominator != 0).then(|| reduced(numerator, denominator))
        }
        "+" if right.kind_str() == "complex" => complex_of(
            context.source.node_text(right),
            Some(context.source.node_text(left)),
        ),
        _ => None,
    }
}
