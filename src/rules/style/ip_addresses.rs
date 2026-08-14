use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not hardcode IP addresses.";

/// `IPV6_MAX_SIZE`: an IPv4-mapped IPv6 address is the longest one there is.
const IPV6_MAX_SIZE: usize = 45;

/// `Resolv::IPv6::Regex`, which is the union of the six spellings the standard allows -- the last
/// two of them link-local addresses carrying a `%scope` suffix.
static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    let hex = "[0-9A-Fa-f]{1,4}";
    let group = format!("(?:{hex}(?::{hex})*)?");
    let scope = "%[-0-9A-Za-z._~]+";
    let alternatives = [
        format!(r"\A(?:{hex}:){{7}}{hex}\z"),
        format!(r"\A{group}::{group}\z"),
        format!(r"\A(?:{hex}:){{6}}(\d+)\.(\d+)\.(\d+)\.(\d+)\z"),
        format!(r"\A{group}::(?:{hex}:)*(\d+)\.(\d+)\.(\d+)\.(\d+)\z"),
        format!(r"\A[Ff][Ee]80(?::{hex}){{7}}{scope}\z"),
        format!(r"\A[Ff][Ee]80:(?:{group}::{group}|:{group})?:{hex}{scope}\z"),
    ];
    Regex::new(&format!("(?:{})", alternatives.join("|"))).unwrap()
});

/// A string literal that spells out an IP address.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context
        .setting::<Vec<String>>("AllowedAddresses")
        .unwrap_or_default()
        .iter()
        .map(|entry| entry.to_lowercase())
        .collect();
    for node in context.nodes_of("string") {
        // `on_regexp` ignores the node, which takes the literals inside an interpolated regexp
        // with it.
        if inside_a_regexp(node) {
            continue;
        }
        let source = context.source.node_text(node);
        // `node.source[1...-1]`: one character comes off each end, whatever the delimiters were.
        let Some(contents) = source
            .char_indices()
            .nth(1)
            .and_then(|(start, _)| source[start..].char_indices().next_back().map(|(end, _)| &source[start..start + end]))
        else {
            continue;
        };
        if contents.is_empty() {
            continue;
        }
        if allowed.contains(&contents.to_lowercase()) {
            continue;
        }
        if !potential_ip(contents) {
            continue;
        }
        if !is_ipv4(contents) && !IPV6.is_match(contents) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// `potential_ip?`: short enough, and starting with a character an address can start with.
fn potential_ip(text: &str) -> bool {
    if text.chars().count() > IPV6_MAX_SIZE {
        return false;
    }
    // `(48..58)` reaches past the digits and takes in `:` as well.
    text.as_bytes().first().is_some_and(|byte| {
        (48..=58).contains(byte) || (65..=70).contains(byte) || (97..=102).contains(byte)
    })
}

/// `Resolv::IPv4::Regex`: four octets, each written without a leading zero.
fn is_ipv4(text: &str) -> bool {
    let mut octets = text.split('.');
    let matched = (0..4).all(|_| octets.next().is_some_and(is_octet));
    matched && octets.next().is_none()
}

/// `Resolv::IPv4::Regex256`.
fn is_octet(text: &str) -> bool {
    if text == "0" {
        return true;
    }
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > 3 || bytes[0] == b'0' {
        return false;
    }
    if !bytes.iter().all(u8::is_ascii_digit) {
        return false;
    }
    text.parse::<u16>().is_ok_and(|value| value <= 255)
}

/// Whether a regexp encloses the literal.
fn inside_a_regexp(node: Node<'_>) -> bool {
    std::iter::successors(node.parent(), |current| current.parent())
        .any(|ancestor| ancestor.kind_str() == "regex")
}
