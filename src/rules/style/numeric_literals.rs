use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str =
    "Use underscores(_) as thousands separator and separate every 3 digits with them.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let min_digits: usize = context.setting("MinDigits").unwrap_or(5);
    let strict: bool = context.setting("Strict").unwrap_or(false);

    for node in context.nodes_of_any(&["integer", "float"]) {
        // A rational or imaginary suffix makes a different literal upstream (`rational` / `complex`
        // nodes), and this cop only visits integers and floats.
        if node
            .parent_of(context)
            .is_some_and(|parent| matches!(parent.kind_str(), "rational" | "complex"))
        {
            continue;
        }

        let range = signed_range(node);
        let source = context.source.slice(range.clone());
        let integer = integer_part(source);
        // Anything starting with `0` is a non-decimal literal (`0x`, `0b`, `0o`, or a leading-zero
        // octal), which this cop does not know how to group.
        if integer.starts_with('0') || integer.chars().count() < min_digits {
            continue;
        }
        if !offending(integer, strict) {
            continue;
        }

        offenses.push(context.offense(MSG, range.clone()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: format_number(source),
            safe: true,
        }));
    }
}

/// The literal's own range, widened to take in a sign that belongs to it.
///
/// RuboCop sees `-9_223_372_036_854_775_808` as a single literal because the parser folds a `-` or
/// `+` sitting directly in front of a number into it, so the offense -- and the replacement -- has
/// to start at the sign. `-2**2` is not such a case: there the sign applies to the power, and the
/// grammar hangs the number off the `**` instead of off the unary node.
fn signed_range(node: Node<'_>) -> Range<usize> {
    let Some(parent) = node.parent() else {
        return node.byte_range();
    };
    let signed = parent.kind_str() == "unary"
        && parent
            .field("operator")
            .is_some_and(|operator| matches!(operator.kind_str(), "-" | "+"))
        && parent
            .field("operand")
            .is_some_and(|operand| operand.id() == node.id());
    if signed {
        parent.byte_range()
    } else {
        node.byte_range()
    }
}

/// The digits before any exponent or decimal point, without the sign.
///
/// The sign is stripped but whitespace behind it is not, exactly as upstream: `- 12_345` keeps its
/// space here, which is why it can never look like a plain run of digits.
fn integer_part(source: &str) -> &str {
    let unsigned = source.strip_prefix(['+', '-']).unwrap_or(source);
    match unsigned.find(['e', 'E', '.']) {
        Some(offset) => &unsigned[..offset],
        None => unsigned,
    }
}

/// Whether the integer part is grouped the way this cop wants.
///
/// A run of bare digits is always wrong once it is long enough. An already underscored number is
/// wrong only when the grouping itself is off: four digits in a row means a group was missed, and
/// a group of one or two digits means one was cut short.
fn offending(integer: &str, strict: bool) -> bool {
    static FOUR_DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}").unwrap());
    // Without `Strict`, a trailing short group is the accepted way to write cents
    // (`10_000_00` for $10,000), so only an interior one counts.
    static SHORT_GROUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_\d{1,2}_").unwrap());
    static SHORT_GROUP_STRICT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"_\d{1,2}(_|$)").unwrap());

    if !integer.is_empty() && integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    FOUR_DIGITS.is_match(integer)
        || if strict {
            SHORT_GROUP_STRICT.is_match(integer)
        } else {
            SHORT_GROUP.is_match(integer)
        }
}

/// The literal rewritten with an underscore every three digits, keeping its sign and whatever
/// followed the exponent or decimal point.
fn format_number(source: &str) -> String {
    let compact: String = source
        .chars()
        .filter(|character| !matches!(character, ' ' | '\t' | '\r' | '\n' | '\u{b}' | '\u{c}'))
        .collect();
    match compact.find(['e', 'E', '.']) {
        Some(offset) => format!(
            "{}{}",
            format_int_part(&compact[..offset]),
            &compact[offset..]
        ),
        None => format_int_part(&compact),
    }
}

fn format_int_part(int_part: &str) -> String {
    let digits: String = int_part
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    let grouped = grouped_number(&digits);
    if int_part.starts_with('-') {
        format!("-{grouped}")
    } else {
        grouped
    }
}

fn grouped_number(number: &str) -> String {
    let first = number.len() % 3;
    let mut output = String::with_capacity(number.len() + number.len() / 3);
    let mut index = 0;
    if first != 0 {
        output.push_str(&number[..first]);
        index = first;
    }
    while index < number.len() {
        if !output.is_empty() {
            output.push('_');
        }
        output.push_str(&number[index..index + 3]);
        index += 3;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{format_number, grouped_number, integer_part, offending};

    #[test]
    fn groups_decimal_digits() {
        assert_eq!(grouped_number("12345"), "12_345");
        assert_eq!(grouped_number("1234567"), "1_234_567");
    }

    #[test]
    fn takes_the_digits_before_a_point_or_exponent() {
        assert_eq!(integer_part("1234567890.50"), "1234567890");
        assert_eq!(integer_part("-9223372036854775808"), "9223372036854775808");
        assert_eq!(integer_part("1234567e10"), "1234567");
        // 符号のうしろの空白は残る。本家が `sub(/^[+-]/, '')` しかしないため。
        assert_eq!(integer_part("- 12345"), " 12345");
    }

    #[test]
    fn an_underscored_number_offends_only_when_its_grouping_is_wrong() {
        assert!(!offending("1_000_000", false));
        // 4 桁続けばグループを取りこぼしている。
        assert!(offending("2018_02_12_164506", false));
        assert!(offending("1_0000", false));
        // 途中の 2 桁グループも取りこぼし。
        assert!(offending("18_00_00", false));
        // 末尾だけが短いのはセント表記なので Strict のときだけ咎める。
        assert!(!offending("10_000_00", false));
        assert!(offending("10_000_00", true));
    }

    #[test]
    fn keeps_the_sign_and_the_fraction() {
        assert_eq!(format_number("-12345.0"), "-12_345.0");
        assert_eq!(format_number("1234567890.50"), "1_234_567_890.50");
        assert_eq!(format_number("2018_02_12_164506"), "20_180_212_164_506");
        assert_eq!(format_number("- 12345"), "-12_345");
    }
}
