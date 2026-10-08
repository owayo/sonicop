use std::borrow::Cow;
use std::collections::HashMap;

use base64::Engine;
use serde::Deserialize;
use tree_sitter::Node;

use super::literal;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::ruby_literal;
use crate::rules::send_node;
use crate::rules::support;

/// 本家が receiver のない呼び出しも対象とする２メソッド。
const NO_RECEIVER_METHODS: [&str; 2] = ["exit", "exit!"];

/// `Methods` の既定値。Psych の binary scalar は UTF-8 に変換しない。
enum MethodDefault {
    Bool(bool),
    Int(i64),
    Text(String),
    Symbol(String),
    Binary(Vec<u8>),
}

impl<'de> Deserialize<'de> for MethodDefault {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_yaml_ng::Value::deserialize(deserializer)?;
        match value {
            serde_yaml_ng::Value::Bool(value) => Ok(Self::Bool(value)),
            serde_yaml_ng::Value::Number(value) => value
                .as_i64()
                .map(Self::Int)
                .ok_or_else(|| serde::de::Error::custom("unsupported method default number")),
            serde_yaml_ng::Value::String(value) => Ok(Self::Text(value)),
            serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "ruby/symbol" => tagged
                .value
                .as_str()
                .map(|value| Self::Symbol(value.to_owned()))
                .ok_or_else(|| serde::de::Error::custom("symbol default must be a scalar")),
            serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "binary" => {
                let value = tagged
                    .value
                    .as_str()
                    .ok_or_else(|| serde::de::Error::custom("binary default must be a scalar"))?;
                decode_binary(value)
                    .map(Self::Binary)
                    .map_err(serde::de::Error::custom)
            }
            // Psych は未知の scalar タグを値の型として扱わない。
            serde_yaml_ng::Value::Tagged(tagged)
                if !tagged.tag.to_string().starts_with("!ruby/") =>
            {
                tagged
                    .value
                    .as_str()
                    .map(|value| Self::Text(value.to_owned()))
                    .ok_or_else(|| serde::de::Error::custom("unsupported method default"))
            }
            _ => Err(serde::de::Error::custom("unsupported method default")),
        }
    }
}

/// Psych の binary は `unpack1('m')` と同じく、余白・無効な文字・不足した padding を許す。
pub(super) fn decode_binary(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const DECODER: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    let mut encoded = Vec::with_capacity(value.len());
    for byte in value.bytes() {
        // Ruby の pack.c は各組の３・４文字目にある '=' でのみ打ち切る。
        if byte == b'=' && encoded.len() % 4 >= 2 {
            break;
        }
        if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/') {
            encoded.push(byte);
        }
    }
    // 末尾の１文字からはバイトを復元できず、本家もその文字を捨てる。
    if encoded.len() % 4 == 1 {
        encoded.pop();
    }
    DECODER.decode(encoded)
}

impl MethodDefault {
    /// 本家が比較対象に保存する `arg.inspect`。符号化によってエスケープ表現も異なる。
    fn inspect(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Text(value) => literal::inspect_string(value),
            Self::Symbol(value) => ruby_literal::inspect_symbol(value),
            Self::Binary(value) => format!("\"{}\"", literal::inspect_bytes(value, true)),
        }
    }
}

/// 本家の `arg.inspect` と、呼び出しに渡された引数の値を照合する。
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(methods) = context.setting::<HashMap<String, MethodDefault>>("Methods") else {
        return;
    };
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        // receiver のない呼び出しを本家が認めるのは `exit` と `exit!` だけ。
        if node.field("receiver").is_none() && !NO_RECEIVER_METHODS.contains(&method) {
            continue;
        }
        let Some(default) = methods.get(method) else {
            continue;
        };
        let Some(arguments) = node.field("arguments") else {
            continue;
        };
        let written = super::nodes::children_in(arguments, context);
        let [only] = written.as_slice() else {
            continue;
        };
        if written_value(*only, context) != default.inspect() {
            continue;
        }
        // `argument_range` は括弧があれば括弧ごと、なければ改行以外の前後の空白を含める。
        let text = context.source.text();
        let range = if context.source.node_text(arguments).starts_with('(') {
            arguments.byte_range()
        } else {
            support::final_pos(text, only.start_byte(), false, false, false, false)
                ..support::final_pos(text, only.end_byte(), true, false, false, false)
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Argument {} is redundant because it is implied by default.",
                        argument_source(*only, context)
                    ),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

fn argument_source<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Cow<'a, str> {
    let text = context.source.node_text(node);
    if text.is_ascii()
        || crate::engine::declared_literal_encoding(context.source.text())
            != crate::engine::LiteralEncoding::Binary
    {
        return Cow::Borrowed(text);
    }
    // 座標計算用の 1 バイト 1 文字表現を戻し、本家 JSON が保持する UTF-8 バイト列を表示する。
    let bytes: Result<Vec<u8>, _> = text
        .chars()
        .map(|character| u8::try_from(character as u32))
        .collect();
    bytes
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map_or(Cow::Borrowed(text), Cow::Owned)
}

/// 本家の `argument_matched?` は値があるノードを `inspect` し、それ以外は元ソースを使う。
fn written_value(node: Node<'_>, context: &RuleContext<'_>) -> String {
    match node.kind_str() {
        "string" if !send_node::has_interpolation(node) => inspected_string(node, context),
        "character" => inspected_string(node, context),
        "integer" => integer_value(context.source.node_text(node)).map_or_else(
            || context.source.node_text(node).to_owned(),
            |value| value.to_string(),
        ),
        _ => context.source.node_text(node).to_owned(),
    }
}

fn inspected_string(node: Node<'_>, context: &RuleContext<'_>) -> String {
    let Some(bytes) = literal::node_bytes(context, node) else {
        return ruby_literal::inspect_string(&ruby_literal::string_value(node, context));
    };
    // Unicode escape は binary/US-ASCII 宣言下でも文字列を UTF-8 にする。
    let is_unicode_escape = |child: Node<'_>| {
        child.kind_str() == "escape_sequence" && context.source.node_text(child).starts_with("\\u")
    };
    let unicode_escape = match context.children(node) {
        Some(mut children) => children.any(is_unicode_escape),
        None => {
            let mut cursor = node.walk();
            node.children(&mut cursor).any(is_unicode_escape)
        }
    } || (node.kind_str() == "character"
        && context.source.node_text(node).starts_with("?\\u"));
    let binary = !unicode_escape
        && match crate::engine::declared_literal_encoding(context.source.text()) {
            crate::engine::LiteralEncoding::Binary => true,
            crate::engine::LiteralEncoding::SevenBit => context.source.node_text(node).is_ascii(),
            crate::engine::LiteralEncoding::Text => false,
        };
    format!("\"{}\"", literal::inspect_bytes(&bytes, binary))
}

/// `Integer#inspect` はソースの基数によらず十進数を返す。
fn integer_value(text: &str) -> Option<i64> {
    let cleaned: String = text.chars().filter(|character| *character != '_').collect();
    let (sign, digits) = match cleaned.strip_prefix('-') {
        Some(rest) => (-1, rest.to_owned()),
        None => (1, cleaned.trim_start_matches('+').to_owned()),
    };
    let lowered = digits.to_lowercase();
    let (radix, body) = if let Some(rest) = lowered.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = lowered.strip_prefix("0b") {
        (2, rest)
    } else if let Some(rest) = lowered.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = lowered.strip_prefix("0d") {
        (10, rest)
    } else if lowered.len() > 1 && lowered.starts_with('0') {
        (8, &lowered[1..])
    } else {
        (10, lowered.as_str())
    };
    i64::from_str_radix(body, radix)
        .ok()
        .map(|value| sign * value)
}

#[cfg(test)]
mod tests {
    use super::decode_binary;

    #[test]
    fn binary_defaults_follow_rubys_permissive_unpack() {
        // Ruby の String#unpack1("m") で採取した値。strict Base64 では拒む入力も含める。
        for (encoded, bytes) in [
            ("gg==", vec![130]),
            ("gg", vec![130]),
            ("gh", vec![130]),
            ("g$g", vec![130]),
            ("g=g", vec![130]),
            ("==gg", vec![130]),
            (":gg", vec![130]),
            ("g", vec![]),
            ("ggg", vec![130, 8]),
            ("////", vec![255, 255, 255]),
            ("gg==AAAA", vec![130]),
            ("1234", vec![215, 109, 248]),
            ("false", vec![125, 169, 108]),
            ("~", vec![]),
            ("0x1F", vec![211, 29, 69]),
            ("1_0", vec![215]),
        ] {
            assert_eq!(decode_binary(encoded).unwrap(), bytes, "{encoded}");
        }
    }
}
