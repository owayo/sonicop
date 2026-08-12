use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "scientific".to_owned());
    let message = match style.as_str() {
        "engineering" => "Use an exponent divisible by 3 and a mantissa >= 0.1 and < 1000.",
        "integral" => "Use an integer as mantissa, without trailing zero.",
        "scientific" => "Use a mantissa >= 1 and < 10.",
        _ => return,
    };

    for node in context.nodes_of("float") {
        let source = context.source.node_text(node);
        let Some((mantissa, exponent)) = source.split_once('e') else {
            continue;
        };
        let acceptable = match style.as_str() {
            "scientific" => scientific(mantissa),
            "engineering" => engineering(mantissa, exponent),
            _ => integral(mantissa),
        };
        if acceptable {
            continue;
        }
        offenses.push(context.offense(message, node.byte_range()));
    }
}

/// `/^-?[1-9](\.\d*[0-9])?$/`.
fn scientific(mantissa: &str) -> bool {
    let body = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let mut characters = body.chars();
    if !characters.next().is_some_and(|first| ('1'..='9').contains(&first)) {
        return false;
    }
    let rest = characters.as_str();
    if rest.is_empty() {
        return true;
    }
    let Some(digits) = rest.strip_prefix('.') else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
}

/// `/^-?[1-9](\d*[1-9])?$/`.
fn integral(mantissa: &str) -> bool {
    let body = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let mut characters = body.chars();
    if !characters.next().is_some_and(|first| ('1'..='9').contains(&first)) {
        return false;
    }
    let rest = characters.as_str();
    rest.is_empty()
        || (rest.chars().all(|character| character.is_ascii_digit())
            && rest.ends_with(|character: char| ('1'..='9').contains(&character)))
}

fn engineering(mantissa: &str, exponent: &str) -> bool {
    let body = exponent.strip_prefix('-').unwrap_or(exponent);
    if body.is_empty() || !body.chars().all(|character| character.is_ascii_digit()) {
        return false;
    }
    let Ok(value) = exponent.parse::<i64>() else {
        return false;
    };
    if value % 3 != 0 {
        return false;
    }
    let digits = mantissa.strip_prefix('-').unwrap_or(mantissa);
    // `/^-?\d{4}/`, `/^-?0\d/` and `/^-?0.0/`: four digits, a leading zero, or a value below 0.1.
    if digits.chars().take(4).filter(|c| c.is_ascii_digit()).count() == 4 {
        return false;
    }
    let bytes = digits.as_bytes();
    if bytes.first() == Some(&b'0') && bytes.get(1).is_some_and(u8::is_ascii_digit) {
        return false;
    }
    // The regexp's `.` matches any character, so `0x0` would fail it too.
    !(bytes.first() == Some(&b'0') && bytes.get(2) == Some(&b'0'))
}
