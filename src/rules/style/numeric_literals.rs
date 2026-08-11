use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let min_digits: usize = context.setting("MinDigits").unwrap_or(5);
    for node in context.nodes_of("integer") {
        let text = context.source.node_text(node);
        if text.len() < min_digits
            || text.contains('_')
            || text.starts_with('0')
            || !text.bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let replacement = grouped_number(text);
        offenses.push(
            context
                .offense(
                    "Use underscores(_) as thousands separator and separate every 3 digits with them.",
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
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
    use super::grouped_number;

    #[test]
    fn groups_decimal_digits() {
        assert_eq!(grouped_number("12345"), "12_345");
        assert_eq!(grouped_number("1234567"), "1_234_567");
    }
}
