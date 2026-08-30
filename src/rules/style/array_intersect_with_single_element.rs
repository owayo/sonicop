//! `Style/ArrayIntersectWithSingleElement`: one element asks `include?`, not `intersect?`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, is_plain_send};

const MSG: &str = "Use `include?(element)` instead of `intersect?([element])`.";

/// `(send _ _ $(array $_))` with `RESTRICT_ON_SEND = %i[intersect?]`.
///
/// `on_csend` is aliased to `on_send` upstream, but the pattern names `send`, so a safe navigation
/// call never matches it -- the alias is dead. [`is_plain_send`] keeps that.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if context.source.node_text(selector) != "intersect?" || !is_plain_send(node, context) {
            continue;
        }
        let list = arguments(node);
        let [argument] = list.as_slice() else {
            continue;
        };
        let array = argument.first();
        let Some(kind) = ArrayKind::of(array) else {
            continue;
        };
        let elements = super::nodes::children_in(array, context);
        let [element] = elements.as_slice() else {
            continue;
        };
        // `[*foo]` spreads a collection rather than naming one element, so `include?` would ask a
        // different question.
        if element.kind_str() == "splat_argument" {
            continue;
        }
        // `element.value.inspect` for a percent literal: its elements are written bare, so the
        // source of one is not a literal that stands on its own.
        let replacement = match kind {
            ArrayKind::Bracketed => context.source.node_text(*element).to_owned(),
            _ => {
                let Some(value) = super::literal::node_value(context, *element) else {
                    continue;
                };
                match kind {
                    ArrayKind::Strings => super::literal::inspect_string(&value.value),
                    _ => super::literal::inspect_symbol(&value.value),
                }
            }
        };
        // The offense starts at the selector: the receiver stays as written.
        let range = selector.start_byte()..node.end_byte();
        offenses.push(context.offense(MSG, range).corrected_by_all([
            Edit {
                start: selector.start_byte(),
                end: selector.end_byte(),
                replacement: "include?".to_owned(),
                safe: true,
            },
            Edit {
                start: array.start_byte(),
                end: array.end_byte(),
                replacement,
                safe: true,
            },
        ]));
    }
}

/// The three spellings of an `array` node, which upstream tells apart with `percent_literal?`.
enum ArrayKind {
    Bracketed,
    Strings,
    Symbols,
}

impl ArrayKind {
    fn of(node: Node<'_>) -> Option<Self> {
        match node.kind_str() {
            "array" => Some(Self::Bracketed),
            "string_array" => Some(Self::Strings),
            "symbol_array" => Some(Self::Symbols),
            _ => None,
        }
    }
}
