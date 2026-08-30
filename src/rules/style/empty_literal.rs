use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::rules::RuleContext;
use crate::rules::send_node::{is_plain_send, arguments, top_level_constant};
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let frozen = frozen_strings(context);
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        let Some(literal) = Literal::read(node, context) else {
            continue;
        };
        if literal == Literal::String && frozen {
            continue;
        }
        let source = context.source.node_text(node);
        let message = match literal {
            Literal::Array => format!("Use array literal `[]` instead of `{source}`."),
            Literal::Hash => format!("Use hash literal `{{}}` instead of `{source}`."),
            Literal::String => format!(
                "Use string literal `{}` instead of `String.new`.",
                preferred_string_literal(context)
            ),
        };
        let (range, correction) = match literal {
            Literal::Array => (node.byte_range(), "[]".to_owned()),
            Literal::String => (node.byte_range(), preferred_string_literal(context)),
            Literal::Hash => match unparenthesized_first_argument(node, context) {
                // `some_method Hash.new` cannot become `some_method {}`: the braces would read as
                // a block, so the whole argument list is parenthesized instead.
                Some((start, end, rest)) => (
                    start..end,
                    format!(
                        "({})",
                        std::iter::once("{}".to_owned())
                            .chain(rest)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
                None => (node.byte_range(), "{}".to_owned()),
            },
        };
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: correction,
                    safe: true,
                }),
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Literal {
    Array,
    Hash,
    String,
}

impl Literal {
    fn read(node: Node<'_>, context: &RuleContext<'_>) -> Option<Self> {
        match node.kind_str() {
            // `Array[]` and `Hash[]`, which upstream reads as a call to `:[]` with no arguments.
            "element_reference" => {
                if super::nodes::children_in(node, context).len() != 1 {
                    return None;
                }
                let object = node.field("object")?;
                named_constant(object, context).filter(|literal| *literal != Self::String)
            }
            "call" => {
                let method = node.field("method")?;
                let name = context.source.node_text(method);
                let list = arguments(node);
                match node.field("receiver") {
                    // `Array.new`, `Hash.new` and `String.new`.
                    Some(receiver) => {
                        if name != "new" {
                            return None;
                        }
                        let literal = named_constant(receiver, context)?;
                        // `(send (const _ :Array) :new (array)?)`: `Array.new([])` is still an
                        // empty array, while `Array.new(5)` is not. `Hash.new` and `String.new`
                        // take no argument at all.
                        let takes_nothing = match list.as_slice() {
                            [] => true,
                            [only] => match only.parts() {
                                [argument] => {
                                    literal == Self::Array && is_empty_array(*argument, context)
                                }
                                _ => false,
                            },
                            _ => false,
                        };
                        if !takes_nothing {
                            return None;
                        }
                        // A block makes the result something other than the bare literal.
                        if literal != Self::String && node.field("block").is_some() {
                            return None;
                        }
                        // **`hash_with_block(node.parent)` reaches past the call's own block.**
                        // A `Hash.new` written as the body of another `Hash.new`'s block has that
                        // block for a parent upstream, so the check meant for the outer call
                        // silences the inner one too.
                        if literal == Self::Hash && inside_hash_new_block(node, context) {
                            return None;
                        }
                        Some(literal)
                    }
                    // `Array([])` and `Hash([])`, the conversion functions.
                    None => {
                        let literal = match name {
                            "Array" => Self::Array,
                            "Hash" => Self::Hash,
                            _ => return None,
                        };
                        let [only] = list.as_slice() else {
                            return None;
                        };
                        let [argument] = only.parts() else {
                            return None;
                        };
                        is_empty_array(*argument, context).then_some(literal)
                    }
                }
            }
            _ => None,
        }
    }
}

/// `(const {nil? cbase} :Array)` and its `Hash` and `String` counterparts.
/// Whether the call stands as the body of a `Hash.new { ... }` block, which is what
/// `hash_with_block(node.parent)` answers to for a nested call.
fn inside_hash_new_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(body) = node.parent_of(context) else {
        return false;
    };
    let Some(block) = body
        .parent_of(context)
        .filter(|parent| matches!(parent.kind_str(), "block" | "do_block"))
        .or_else(|| matches!(body.kind_str(), "block" | "do_block").then_some(body))
    else {
        return false;
    };
    block
        .parent_of(context)
        .filter(|call| call.kind_str() == "call")
        .is_some_and(|call| {
            call.field("method")
                .is_some_and(|method| context.source.node_text(method) == "new")
                && call
                    .field("receiver")
                    .is_some_and(|receiver| context.source.node_text(receiver) == "Hash")
        })
}

fn named_constant(node: Node<'_>, context: &RuleContext<'_>) -> Option<Literal> {
    for (name, literal) in [
        ("Array", Literal::Array),
        ("Hash", Literal::Hash),
        ("String", Literal::String),
    ] {
        if top_level_constant(node, name, context) {
            return Some(literal);
        }
    }
    None
}

fn is_empty_array(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "array"
        && super::nodes::children_in(node, context).is_empty()
        && context.source.node_text(node).starts_with('[')
}

/// `first_argument_unparenthesized?`: the span the whole argument list has to be rewritten over,
/// and the sources of the arguments that follow.
fn unparenthesized_first_argument(
    node: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<(usize, usize, Vec<String>)> {
    let parent = node.parent_of(context)?;
    let list = match parent.kind_str() {
        "call" | "super" => parent.field("arguments")?,
        "argument_list" => {
            let call = parent.parent_of(context)?;
            if !matches!(call.kind_str(), "call" | "super") {
                return None;
            }
            parent
        }
        _ => return None,
    };
    if context.source.node_text(list).starts_with('(') {
        return None;
    }
    let call = match list.id() == parent.id() {
        true => parent.parent_of(context)?,
        false => parent,
    };
    let all = arguments(call);
    let first = all.first()?;
    if first.first().id() != node.id() {
        return None;
    }
    let rest: Vec<String> = all[1..]
        .iter()
        .map(|argument| context.source.slice(argument.range()).to_owned())
        .collect();
    let last = all.last()?;
    Some((node.start_byte() - 1, last.range().end, rest))
}

/// `preferred_string_literal`, which follows `Style/StringLiterals`.
fn preferred_string_literal(context: &RuleContext<'_>) -> String {
    match context
        .setting_of::<String>("Style/StringLiterals", "EnforcedStyle")
        .as_deref()
    {
        Some("double_quotes") => "\"\"".to_owned(),
        _ => "''".to_owned(),
    }
}

/// `frozen_strings?`: whether a bare `''` would differ from `String.new`, which it does unless the
/// file explicitly turns frozen string literals off.
fn frozen_strings(context: &RuleContext<'_>) -> bool {
    let mut specified = None;
    for line_number in 1..=context.source.line_count() {
        let line = context.source.line(line_number);
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            break;
        }
        let comment = MagicComment::parse(line);
        if specified.is_none() && comment.frozen_string_literal_specified() {
            specified = Some(comment.frozen_string_literal_enabled());
        }
    }
    match specified {
        Some(enabled) => enabled,
        None => context
            .setting_of::<bool>("Style/FrozenStringLiteralComment", "Enabled")
            .unwrap_or(true),
    }
}
