//! Whether two nodes are the same node, the way `Node#==` upstream decides it.
//!
//! Upstream compares a node with another by its type and its children rather than by the text it
//! was written as, so a difference in spacing is no difference at all and two literals its parser
//! resolved to the same value are equal however they were spelled. Several cops rest on that --
//! identical operands, a repeated `when` condition, a self-assignment -- and each of them asks the
//! same question.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Whether the two operands are the same node. Upstream compares structurally rather than by
/// source -- `Node#==` looks at the type and the children -- so a difference in spacing is no
/// difference at all, and two literals its parser resolved to the same value are equal however
/// they were written.
/// Reachable from `style` too: `Style/RedundantParentheses` compares a call's receiver against the
/// group it is looking at, and `Node#!=` is structural there as everywhere else.
pub(crate) fn identical(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> bool {
    match (literal(left, context), literal(right, context)) {
        (Some(left), Some(right)) => return left == right,
        // A literal is never equal to anything that is not one.
        (Some(_), None) | (None, Some(_)) => return false,
        (None, None) => {}
    }
    if left.kind_str() != right.kind_str() {
        return false;
    }
    // A heredoc's text is written on the lines below the opener, and the grammar keeps it in a node
    // of its own beside the statement rather than under the opener. Upstream's parser puts it *in*
    // the literal, so comparing the openers alone makes every `<<~EOS` equal to every other one.
    if left.kind_str() == "heredoc_beginning" {
        return same_heredoc(left, right, context);
    }
    let left_children = named_children_with_fields(left);
    let right_children = named_children_with_fields(right);
    if left_children.is_empty() && right_children.is_empty() {
        return context.source.node_text(left) == context.source.node_text(right);
    }
    // The unnamed tokens are what tells apart two nodes holding the same named children: `a.b`
    // from `a&.b`, `[1]` from `%w[1]`.
    if operator_text(left, context) != operator_text(right, context) {
        return false;
    }
    left_children.len() == right_children.len()
        && left_children.iter().zip(&right_children).all(
            |((left_field, left), (right_field, right))| {
                left_field == right_field && identical(*left, *right, context)
            },
        )
}

/// A key two nodes share exactly when [`identical`] calls them equal.
///
/// **This is the same equivalence relation, written so that it can be hashed.** `identical` answers
/// one pair at a time, so a cop looking for a repeat among *n* nodes asks it n(n-1)/2 times, and
/// each answer walks two subtrees. Upstream never does that: its `Duplication` mixin puts the
/// collection through `group_by`, which is linear because a `Node` is hashable there. A single
/// generated table in `ruby/ruby` holds 7,859 pairs in one literal -- 30 million structural
/// comparisons, and `Lint/DuplicateHashKey` alone took longer on that file than every other cop
/// together.
///
/// The two must stay in step. Every branch below mirrors one in `identical`, in the same order:
/// the resolved literal value first (so `?a` and `"a"` share a key), then the node kind, then the
/// heredoc's text, then the source text of a leaf, then the unnamed tokens and the named children
/// with their fields.
///
/// **Bytes rather than a `String`**: a literal's value is a byte string, and Ruby lets a source
/// file hold bytes that are not UTF-8.
pub(crate) fn equality_key(node: Node<'_>, context: &RuleContext<'_>) -> Vec<u8> {
    let mut key = Vec::new();
    write_key(node, context, &mut key);
    key
}

fn write_key(node: Node<'_>, context: &RuleContext<'_>, out: &mut Vec<u8>) {
    if let Some(value) = literal(node, context) {
        match value {
            Literal::Text(bytes) => {
                out.push(b't');
                out.extend(bytes);
            }
            Literal::Symbol(bytes) => {
                out.push(b's');
                out.extend(bytes);
            }
            Literal::Integer(value) => {
                out.push(b'i');
                out.extend(value.to_string().into_bytes());
            }
            Literal::Float(value) => {
                out.push(b'f');
                // `identical` compares floats with `==`, and **`-0.0 == 0.0`**. Adding zero folds
                // the sign away so both land on one key; `to_bits` alone would give them two.
                out.extend((value + 0.0).to_bits().to_string().into_bytes());
            }
        }
        // A literal is never equal to a node that is not one, and the tag byte is what keeps the
        // two spaces apart -- every key below opens with `N`.
        out.push(0);
        return;
    }
    out.push(b'N');
    out.extend(node.kind_str().as_bytes());
    out.push(0);
    if node.kind_str() == "heredoc_beginning" {
        write_heredoc_key(node, context, out);
        return;
    }
    let children = named_children_with_fields(node);
    if children.is_empty() {
        out.push(b'T');
        out.extend(context.source.node_text(node).as_bytes());
        return;
    }
    out.extend(operator_text(node, context).into_bytes());
    out.push(b'(');
    for (field, child) in children {
        out.extend(field.unwrap_or("").as_bytes());
        out.push(b':');
        write_key(child, context, out);
        out.push(b',');
    }
    out.push(b')');
}

/// `same_heredoc` as a key: the runs of text verbatim and the interpolations structurally, with the
/// opener's own line cut off the first run.
///
/// The opener without a body is the one corner where `identical` is not an equivalence relation --
/// it falls back to comparing the openers' text, which can call a bodyless opener equal to one that
/// has a body while their bodies differ. A key cannot express that, so a bodyless opener is keyed by
/// its text under a tag of its own and never merges with a body-bearing one. **Only a source the
/// grammar could not finish reading produces one.**
fn write_heredoc_key(node: Node<'_>, context: &RuleContext<'_>, out: &mut Vec<u8>) {
    let Some(body) = crate::rules::send_node::heredoc_body(node, context) else {
        out.push(b'o');
        out.extend(context.source.node_text(node).as_bytes());
        return;
    };
    out.push(b'H');
    for (index, part) in heredoc_parts(body).into_iter().enumerate() {
        out.extend(part.kind_str().as_bytes());
        out.push(0);
        if part.kind_str() == "heredoc_content" {
            let text = context.source.node_text(part);
            let text = match index {
                0 => text.split_once('\n').map_or(text, |(_, rest)| rest),
                _ => text,
            };
            out.extend(text.as_bytes());
        } else {
            write_key(part, context, out);
        }
        out.push(1);
    }
}

/// The named children, each with the field it sits under.
///
/// The field is what a missing child is told from a present one by: `10..` and `..10` both hold one
/// integer, and only the field says which side it is on, where upstream's `irange` has a `nil` in
/// the other slot and compares unequal.
fn named_children_with_fields<'tree>(
    node: Node<'tree>,
) -> Vec<(Option<&'static str>, Node<'tree>)> {
    let mut cursor = node.walk();
    let mut children = Vec::new();
    if !cursor.goto_first_child() {
        return children;
    }
    loop {
        if cursor.node().is_named() {
            children.push((cursor.field_name(), cursor.node()));
        }
        if !cursor.goto_next_sibling() {
            return children;
        }
    }
}

/// Whether two heredocs hold the same text.
///
/// What the terminator was named is no part of the literal upstream, so only what lies between the
/// opener's line and the terminator is compared: the runs of text verbatim, and anything the heredoc
/// interpolates structurally. The text a run opens with reaches back to the opener itself, so the
/// first line break is where the body actually starts.
fn same_heredoc(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> bool {
    let (Some(ours), Some(theirs)) = (
        crate::rules::send_node::heredoc_body(left, context),
        crate::rules::send_node::heredoc_body(right, context),
    ) else {
        return context.source.node_text(left) == context.source.node_text(right);
    };
    let (ours, theirs) = (heredoc_parts(ours), heredoc_parts(theirs));
    ours.len() == theirs.len()
        && ours
            .iter()
            .zip(&theirs)
            .enumerate()
            .all(|(index, (one, other))| {
                if one.kind_str() != other.kind_str() {
                    return false;
                }
                if one.kind_str() != "heredoc_content" {
                    return identical(*one, *other, context);
                }
                let text = |node: &Node<'_>| {
                    let text = context.source.node_text(*node);
                    match index {
                        0 => text.split_once('\n').map_or(text, |(_, rest)| rest),
                        _ => text,
                    }
                };
                text(one) == text(other)
            })
}

/// The parts of a heredoc body that belong to the literal, which is everything but the terminator.
fn heredoc_parts<'tree>(body: Node<'tree>) -> Vec<Node<'tree>> {
    named_children_with_fields(body)
        .into_iter()
        .map(|(_, node)| node)
        .filter(|node| node.kind_str() != "heredoc_end")
        .collect()
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
    match node.kind_str() {
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
            let operator = context.source.node_text(node.field("operator")?);
            let operand = node.field("operand")?;
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
        .any(|child| child.kind_str() == "interpolation")
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
