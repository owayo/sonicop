//! Percent literals as RuboCop's `PercentLiteral` mixin sees them.
//!
//! Upstream reads `node.loc.begin` -- the `%w(` that opens the literal -- and takes everything but
//! its last character as the literal's *type*. Every cop that reasons about `%`-literals starts
//! there, so the parse of that opener lives here rather than in each cop.

use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_iter;

/// Node kinds that can be written as a `%`-literal. A node of one of these kinds is only a percent
/// literal when its opening delimiter actually starts with `%`; `"a"` and `%q(a)` share a kind.
pub(super) const LITERAL_KINDS: &[&str] = &[
    "string_array",
    "symbol_array",
    "regex",
    "string",
    "delimited_symbol",
    "subshell",
];

/// The percent-literal types each node kind can spell, mirroring which `on_*` handler upstream
/// passes which types to `process`.
fn types_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "string_array" | "symbol_array" => &["%w", "%W", "%i", "%I"],
        "regex" => &["%r"],
        "string" => &["%", "%Q", "%q"],
        "delimited_symbol" => &["%s"],
        "subshell" => &["%x"],
        _ => &[],
    }
}

pub(super) struct PercentLiteral {
    /// `%w`, `%r`, `%q` or bare `%`: the opener without its delimiter.
    pub percent_type: String,
    /// The character the literal opens with, `begin_source(node)[-1]` upstream.
    pub opening: char,
    /// The span of the whole opener, `loc.begin` upstream.
    pub begin: Range<usize>,
    /// The span of the closing delimiter alone. A regexp's closing node also carries its options,
    /// which are not part of `loc.end`.
    pub close: Range<usize>,
}

impl PercentLiteral {
    pub(super) fn new(node: Node<'_>, context: &RuleContext<'_>) -> Option<Self> {
        if node.child_count() < 2 || is_modulo_operand(node, context) {
            return None;
        }
        let begin = node.child(0)?;
        let close = node.child(node.child_count().saturating_sub(1) as u32)?;
        if begin.id() == close.id() {
            return None;
        }
        let opener = context.source.node_text(begin);
        if !opener.starts_with('%') {
            return None;
        }
        // The delimiter is the opener's last character; the type is everything before it.
        let (delimiter_start, opening) = opener.char_indices().next_back()?;
        let percent_type = opener[..delimiter_start].to_owned();
        if !types_for(node.kind_str()).contains(&percent_type.as_str()) {
            return None;
        }
        // **A literal closed by a newline leaves the grammar with an empty closing node.** `%\n\n`
        // is a string whose delimiter is the line break: the break that ends it is counted into the
        // content and the closing node sits after it with no text of its own. The delimiter is
        // still there in the source, one character back from where the literal ends.
        let close_text = context.source.node_text(close);
        let close = match close_text.chars().next() {
            Some(character) => close.start_byte()..close.start_byte() + character.len_utf8(),
            None => node.end_byte().saturating_sub(opening.len_utf8())..node.end_byte(),
        };
        Some(Self {
            percent_type,
            opening,
            begin: begin.byte_range(),
            close,
        })
    }
}

/// The literal parts of a percent literal, as `contains_delimiter?` reads them.
///
/// Upstream walks the node's children and keeps only those that are strings or symbols, so an
/// interpolated element contributes nothing at all: `%W(a #{b})` is a `dstr` array whose second
/// element upstream skips. The rest come out as raw source.
pub(super) fn literal_segments<'a>(
    node: Node<'_>,
    context: &'a RuleContext<'_>,
    literal: &PercentLiteral,
) -> Vec<&'a str> {
    let text = context.source.text();
    let mut segments = Vec::new();
    if matches!(node.kind_str(), "string_array" | "symbol_array") {
        let _cursor = node.walk();
        for element in named_children_iter(node, context) {
            if !holds_interpolation(element) {
                segments.push(&text[element.byte_range()]);
            }
        }
        return segments;
    }

    // Everything else is one run of text, broken only where an interpolation replaces it with code.
    let mut start = literal.begin.end;
    let _cursor = node.walk();
    for child in named_children_iter(node, context) {
        if child.kind_str() != "interpolation" {
            continue;
        }
        segments.push(&text[start..child.start_byte()]);
        start = child.end_byte();
    }
    segments.push(&text[start..literal.close.start]);
    segments
}

/// Whether the node is really the right-hand side of a `%` operator.
///
/// Ruby only opens a percent literal where a value may begin, so the `%` after a complete literal
/// is modulo and `"%s" %[x]` is a `send` whose argument is an array. tree-sitter chains the two into
/// one literal anyway, which would otherwise hand every such call a percent literal that upstream
/// never saw.
pub(super) fn is_modulo_operand(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    context
        .parent(node)
        .is_some_and(|parent| parent.kind_str() == "chained_string")
        && node.prev_named_sibling().is_some()
}

fn holds_interpolation(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.kind_str() == "interpolation" || node.named_children(&mut cursor).any(holds_interpolation)
}
