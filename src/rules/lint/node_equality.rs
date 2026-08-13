//! Whether two nodes are the same node, the way `Node#==` upstream decides it.
//!
//! Upstream compares a node with another by its type and its children rather than by the text it
//! was written as, so a difference in spacing is no difference at all and two literals its parser
//! resolved to the same value are equal however they were spelled. Several cops rest on that --
//! identical operands, a repeated `when` condition, a self-assignment -- and each of them asks the
//! same question.

use tree_sitter::Node;

use crate::rules::RuleContext;

/// Whether the two operands are the same node. Upstream compares structurally rather than by
/// source -- `Node#==` looks at the type and the children -- so a difference in spacing is no
/// difference at all, and two literals its parser resolved to the same value are equal however
/// they were written.
pub(crate) fn identical(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> bool {
    match (literal(left, context), literal(right, context)) {
        (Some(left), Some(right)) => return left == right,
        // A literal is never equal to anything that is not one.
        (Some(_), None) | (None, Some(_)) => return false,
        (None, None) => {}
    }
    if left.kind() != right.kind() {
        return false;
    }
    let (mut left_cursor, mut right_cursor) = (left.walk(), right.walk());
    let left_children: Vec<Node<'_>> = left.named_children(&mut left_cursor).collect();
    let right_children: Vec<Node<'_>> = right.named_children(&mut right_cursor).collect();
    if left_children.is_empty() && right_children.is_empty() {
        return context.source.node_text(left) == context.source.node_text(right);
    }
    // The unnamed tokens are what tells apart two nodes holding the same named children: `a.b`
    // from `a&.b`, `[1]` from `%w[1]`.
    if operator_text(left, context) != operator_text(right, context) {
        return false;
    }
    left_children.len() == right_children.len()
        && left_children
            .iter()
            .zip(&right_children)
            .all(|(left, right)| identical(*left, *right, context))
}

fn operator_text(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| !child.is_named())
        .map(|child| context.source.node_text(child))
        .collect()
}

/// The value upstream's parser resolved a basic literal to, which is what its node holds and
/// compares by. `?a` and `"a"` are the same `str`, `:ruby` and `:"ruby"` the same `sym`, `0x10`
/// and `16` the same `int`.
#[derive(PartialEq)]
enum Literal {
    Text(Vec<u8>),
    Symbol(Vec<u8>),
    Integer(i128),
    Float(f64),
}

/// The value of an `int` or `float` literal, with a leading sign folded into it the way upstream's
/// parser folds one. A cop that asks whether a literal *is* some number is asking about the value
/// rather than about the digits: `0x1` and `1` are one and the same `(int 1)` there.
pub(crate) fn numeric_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<f64> {
    match literal(node, context)? {
        // The cast is lossy past 2^53, which is far beyond any literal a cop compares against a
        // small constant.
        Literal::Integer(value) => Some(value as f64),
        Literal::Float(value) => Some(value),
        Literal::Text(_) | Literal::Symbol(_) => None,
    }
}

fn literal(node: Node<'_>, context: &RuleContext<'_>) -> Option<Literal> {
    let text = context.source.node_text(node);
    match node.kind() {
        "integer" => integer_value(text).map(Literal::Integer),
        "float" => float_value(text).map(Literal::Float),
        "character" => decode(&text[1..], false).map(Literal::Text),
        "string" => quoted_value(node, context).map(Literal::Text),
        "simple_symbol" => Some(Literal::Symbol(text.as_bytes()[1..].to_vec())),
        "hash_key_symbol" => Some(Literal::Symbol(text.as_bytes().to_vec())),
        "delimited_symbol" => quoted_value(node, context).map(Literal::Symbol),
        // The parser folds the sign of a numeric literal into the literal itself, which is how
        // `-0.0` and `0.0` end up equal: they are two floats, and `-0.0 == 0.0`.
        "unary" => {
            let operator = context
                .source
                .node_text(node.child_by_field_name("operator")?);
            let operand = node.child_by_field_name("operand")?;
            match (operator, literal(operand, context)?) {
                ("-", Literal::Integer(value)) => Some(Literal::Integer(-value)),
                ("-", Literal::Float(value)) => Some(Literal::Float(-value)),
                ("+", value @ (Literal::Integer(_) | Literal::Float(_))) => Some(value),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The bytes between a literal's delimiters, with its escapes resolved. A literal that interpolates
/// is a `dstr`/`dsym` upstream and has no value of its own, and an escape this cannot resolve
/// leaves the node to be compared structurally instead.
fn quoted_value(node: Node<'_>, context: &RuleContext<'_>) -> Option<Vec<u8>> {
    let mut cursor = node.walk();
    if node
        .named_children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
    {
        return None;
    }
    let open = node.child(0)?;
    let close = node.child(u32::try_from(node.child_count()).ok()?.saturating_sub(1))?;
    if open.id() == close.id() || close.start_byte() < open.end_byte() {
        return None;
    }
    let raw = context.source.slice(open.end_byte()..close.start_byte());
    // `'...'` and `%q(...)` resolve only `\\` and `\'`; everything else resolves the full set.
    let opening = context.source.node_text(open);
    let single = opening == "'" || opening.starts_with("%q");
    decode(raw, single)
}

fn decode(raw: &str, single_quoted: bool) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        let byte = *bytes.get(index)?;
        if single_quoted {
            // Anything but `\\` and `\'` keeps its backslash.
            if byte != b'\\' && byte != b'\'' {
                out.push(b'\\');
            }
            out.push(byte);
            index += 1;
            continue;
        }
        let (value, consumed) = escape(&bytes[index..])?;
        out.extend(value);
        index += consumed;
    }
    Some(out)
}

/// One escape sequence of a double-quoted literal, and how many bytes of it were read. `None` for
/// a sequence this does not resolve, which leaves the whole literal without a value.
fn escape(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    let byte = *bytes.first()?;
    let simple = |value: u8| Some((vec![value], 1));
    match byte {
        b'n' => simple(b'\n'),
        b't' => simple(b'\t'),
        b'r' => simple(b'\r'),
        b'f' => simple(0x0c),
        b'v' => simple(0x0b),
        b'b' => simple(0x08),
        b'a' => simple(0x07),
        b'e' => simple(0x1b),
        b's' => simple(b' '),
        b'\\' | b'\'' | b'"' | b'#' => simple(byte),
        b'\n' => Some((Vec::new(), 1)),
        b'0'..=b'7' => {
            let digits = bytes
                .iter()
                .take(3)
                .take_while(|byte| (b'0'..=b'7').contains(byte))
                .count();
            let value = u32::from_str_radix(std::str::from_utf8(&bytes[..digits]).ok()?, 8).ok()?;
            Some((vec![u8::try_from(value & 0xff).ok()?], digits))
        }
        b'x' => {
            let digits = bytes[1..]
                .iter()
                .take(2)
                .take_while(|byte| byte.is_ascii_hexdigit())
                .count();
            if digits == 0 {
                return None;
            }
            let value =
                u8::from_str_radix(std::str::from_utf8(&bytes[1..=digits]).ok()?, 16).ok()?;
            Some((vec![value], digits + 1))
        }
        // `\C-x` and `\cx` clear the top three bits; `\M-x` sets the eighth. Both may wrap another
        // escape, which is how `?\M-\C-a` reaches `\x81`.
        b'C' | b'c' | b'M' => {
            let prefix = match (byte, bytes.get(1)) {
                (b'C', Some(b'-')) | (b'M', Some(b'-')) => 2,
                (b'c', _) => 1,
                _ => return None,
            };
            let rest = &bytes[prefix..];
            let (value, consumed) = match rest.first()? {
                b'\\' => {
                    let (value, consumed) = escape(&rest[1..])?;
                    (value, consumed + 1)
                }
                other => (vec![*other], 1),
            };
            let [value] = value.as_slice() else {
                return None;
            };
            let value = if byte == b'M' {
                value | 0x80
            } else {
                value & 0x9f
            };
            Some((vec![value], prefix + consumed))
        }
        _ => None,
    }
}

fn integer_value(text: &str) -> Option<i128> {
    let text: String = text.chars().filter(|character| *character != '_').collect();
    let (radix, digits) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (16, &text[2..]),
        Some("0b") => (2, &text[2..]),
        Some("0o") => (8, &text[2..]),
        Some("0d") => (10, &text[2..]),
        _ if text.len() > 1 && text.starts_with('0') => (8, &text[1..]),
        _ => (10, &text[..]),
    };
    i128::from_str_radix(digits, radix).ok()
}

fn float_value(text: &str) -> Option<f64> {
    let text: String = text.chars().filter(|character| *character != '_').collect();
    text.parse().ok()
}
