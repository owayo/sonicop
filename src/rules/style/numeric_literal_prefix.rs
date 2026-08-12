use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// The prefix a literal was written with, and the one it should carry.
#[derive(Clone, Copy)]
enum Kind {
    /// `EnforcedOctalStyle: zero_only`, where `0o` is the spelling to remove.
    OctalZeroOnly,
    Octal,
    Hex,
    Binary,
    Decimal,
}

impl Kind {
    fn message(self) -> &'static str {
        match self {
            Self::OctalZeroOnly => "Use 0 for octal literals.",
            Self::Octal => "Use 0o for octal literals.",
            Self::Hex => "Use 0x for hexadecimal literals.",
            Self::Binary => "Use 0b for binary literals.",
            Self::Decimal => "Do not use prefixes for decimal literals.",
        }
    }

    /// `format_<type>`: the same `sub` upstream runs over the literal's whole source, so a sign in
    /// front of the prefix leaves the literal exactly as it was.
    fn format(self, source: &str) -> String {
        let replace = |prefixes: &[&str], with: &str| {
            for prefix in prefixes {
                if let Some(rest) = source.strip_prefix(prefix) {
                    return format!("{with}{rest}");
                }
            }
            source.to_owned()
        };
        match self {
            Self::OctalZeroOnly => replace(&["0O", "0o", "0"], "0"),
            Self::Octal => replace(&["0O", "0"], "0o"),
            Self::Hex => replace(&["0X"], "0x"),
            Self::Binary => replace(&["0B"], "0b"),
            Self::Decimal => replace(&["0d", "0D"], ""),
        }
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let zero_only = context
        .setting::<String>("EnforcedOctalStyle")
        .is_some_and(|style| style == "zero_only");

    for integer in context.nodes_of("integer") {
        let node = signed(integer);
        let source = context.source.node_text(node);
        let Some(kind) = literal_type(integer_part(source), zero_only) else {
            continue;
        };
        offenses.push(
            context
                .offense(kind.message(), node.byte_range())
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: kind.format(source),
                    safe: true,
                }),
        );
    }
}

/// The node upstream's parser calls the `int`: a sign written directly in front of a numeric
/// literal belongs to the literal there, so the offense covers it and `integer_part` has to strip
/// it back off. A sign in front of an already-signed literal is a method call instead, which is why
/// only a bare integer operand folds.
fn signed(integer: Node<'_>) -> Node<'_> {
    let Some(parent) = integer.parent() else {
        return integer;
    };
    let folds = parent.kind() == "unary"
        && parent
            .child_by_field_name("operator")
            .is_some_and(|operator| matches!(operator.kind(), "+" | "-"))
        && parent
            .child_by_field_name("operand")
            .is_some_and(|operand| operand.id() == integer.id());
    match folds {
        true => parent,
        false => integer,
    }
}

/// `IntegerNode#integer_part`.
fn integer_part(source: &str) -> &str {
    let unsigned = source.strip_prefix(['+', '-']).unwrap_or(source);
    match unsigned.find(['e', 'E', '.']) {
        Some(offset) => &unsigned[..offset],
        None => unsigned,
    }
}

fn literal_type(literal: &str, zero_only: bool) -> Option<Kind> {
    if zero_only {
        if matches(literal, &["0O", "0o"], is_octal_digit) {
            return Some(Kind::OctalZeroOnly);
        }
    } else if matches(literal, &["0O", "0"], is_octal_digit) {
        return Some(Kind::Octal);
    }
    if matches(
        literal,
        &["0X"],
        |digit| matches!(digit, '0'..='9' | 'A'..='F'),
    ) {
        return Some(Kind::Hex);
    }
    if matches(literal, &["0B"], |digit| matches!(digit, '0' | '1')) {
        return Some(Kind::Binary);
    }
    matches(literal, &["0d", "0D"], |digit| digit.is_ascii_digit()).then_some(Kind::Decimal)
}

/// One of the prefixes followed by at least one digit the predicate accepts, and nothing else.
fn matches(literal: &str, prefixes: &[&str], digit: impl Fn(char) -> bool) -> bool {
    prefixes.iter().any(|prefix| {
        literal
            .strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(&digit))
    })
}

fn is_octal_digit(digit: char) -> bool {
    matches!(digit, '0'..='7')
}
