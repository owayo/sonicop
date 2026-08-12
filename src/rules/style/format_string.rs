//! `Style/FormatString`: one of `format`, `sprintf` and `String#%` per project.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{Argument, arguments, is_plain_send};

/// Literal kinds a leading sign belongs to rather than turning into a `:-@` call.
const NUMBER_KINDS: &[&str] = &["integer", "float", "rational", "complex"];

/// `AUTOCORRECTABLE_METHODS`: conversions whose result is never an array, so folding the argument
/// into `format`'s list cannot change what it prints.
const AUTOCORRECTABLE_METHODS: &[&str] =
    &["to_d", "to_f", "to_h", "to_i", "to_r", "to_s", "to_sym"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "format".to_owned());

    for node in context.nodes_of_any(&["call", "binary", "chained_string"]) {
        let Some(found) = Formatter::of(context, node) else {
            continue;
        };
        if found.detected == style {
            continue;
        }
        let message = format!(
            "Favor `{}` over `{}`.",
            method_name(&style),
            method_name(&found.detected)
        );
        let offense = context.offense(message, found.selector.clone());
        offenses.push(match correction(context, &found, &style) {
            Some(edit) => offense.corrected_by(edit),
            None => offense,
        });
    }
}

fn method_name(style: &str) -> &str {
    if style == "percent" {
        "String#%"
    } else {
        style
    }
}

/// The right-hand side of a `%`, which the grammar sometimes leaves as raw text.
enum Operand<'tree> {
    Node(Node<'tree>),
    /// `"%s"%[a, b]`: the grammar reads the `%[...]` as a percent literal of its own, so the array
    /// upstream's parser built was never parsed at all.
    Text(Range<usize>),
}

/// One call this cop recognises: which spelling it used, and where its selector is.
struct Formatter<'tree> {
    detected: String,
    selector: Range<usize>,
    /// The whole expression, which the `%` correction replaces.
    span: Range<usize>,
    /// `node.receiver`, which only the `%` spelling has.
    receiver: Option<Range<usize>>,
    /// `node.first_argument`, which is all either correction reads.
    argument: Option<Operand<'tree>>,
}

impl<'tree> Formatter<'tree> {
    fn of(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Self> {
        match node.kind() {
            "chained_string" => Self::of_chained_string(context, node),
            "binary" => Self::of_binary(context, node),
            _ => Self::of_call(context, node),
        }
    }

    /// `(send {str dstr} $:% ...)` and `(send !nil? $:% {array hash})` written as an operator,
    /// which the grammar spells as a node of its own rather than a call.
    fn of_binary(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Self> {
        let operator = node.child_by_field_name("operator")?;
        if context.source.node_text(operator) != "%" {
            return None;
        }
        let left = node.child_by_field_name("left")?;
        let right = node.child_by_field_name("right")?;
        if !matches!(left.kind(), "string" | "chained_string")
            && !matches!(right.kind(), "array" | "hash")
        {
            return None;
        }
        Some(Self {
            detected: "percent".to_owned(),
            selector: operator.byte_range(),
            span: node.byte_range(),
            receiver: Some(left.byte_range()),
            argument: Some(Operand::Node(right)),
        })
    }

    /// `"%s"%[a, b]`, where nothing separates the literal from the `%`.
    ///
    /// Ruby only opens a percent literal where a value may begin, so this is a call to `:%` whose
    /// argument is an array; the grammar reads a second string literal and chains the two.
    fn of_chained_string(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Self> {
        let first = node.child(0)?;
        let second = node.child(1)?;
        let opener = context.source.node_text(second.child(0)?);
        if node.child_count() != 2 || !opener.starts_with('%') || opener.chars().count() != 2 {
            return None;
        }
        // A `{` operand is a hash to upstream's parser, which rejects the file outright when what
        // stands between the braces is not one. Reading it here would rewrite a file upstream
        // never got as far as inspecting.
        if opener.ends_with('{') {
            return None;
        }
        Some(Self {
            detected: "percent".to_owned(),
            selector: second.start_byte()..second.start_byte() + 1,
            span: node.byte_range(),
            receiver: Some(first.byte_range()),
            argument: Some(Operand::Text(second.start_byte() + 1..second.end_byte())),
        })
    }

    fn of_call(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Self> {
        let selector = node.child_by_field_name("method")?;
        let name = context.source.node_text(selector);
        // `'%s'.%(x)`: the same two `%` alternatives, written as an ordinary call.
        if name == "%" {
            let receiver = node.child_by_field_name("receiver")?;
            let written = arguments(node);
            let first = written.first().map(Argument::first);
            let recognised = matches!(receiver.kind(), "string" | "chained_string")
                || (written.len() == 1
                    && first.is_some_and(|node| matches!(node.kind(), "array" | "hash")));
            if !recognised {
                return None;
            }
            return Some(Self {
                detected: "percent".to_owned(),
                selector: selector.byte_range(),
                span: node.byte_range(),
                receiver: Some(receiver.byte_range()),
                argument: first.map(Operand::Node),
            });
        }
        // `(send nil? ${:sprintf :format} _ _ ...)`: no receiver and at least two arguments.
        if !matches!(name, "sprintf" | "format")
            || node.child_by_field_name("receiver").is_some()
            || !is_plain_send(node, context)
        {
            return None;
        }
        if arguments(node).len() < 2 {
            return None;
        }
        Some(Self {
            detected: name.to_owned(),
            selector: selector.byte_range(),
            span: node.byte_range(),
            receiver: None,
            argument: None,
        })
    }
}

fn correction(context: &RuleContext<'_>, found: &Formatter<'_>, style: &str) -> Option<Edit> {
    // `autocorrect_to_percent` is only reachable with `EnforcedStyle: percent`, which no corpus
    // configures; the offense is reported without a correction rather than guessed at.
    if style == "percent" {
        return None;
    }
    if found.detected != "percent" {
        return Some(Edit {
            start: found.selector.start,
            end: found.selector.end,
            replacement: style.to_owned(),
            safe: true,
        });
    }
    let argument = found.argument.as_ref()?;
    // `variable_argument?`: the argument may already be an array, so folding it into `format`'s
    // list would print something else.
    if is_variable_argument(context, found, argument) {
        return None;
    }
    let receiver = found.receiver.clone()?;
    let args = match argument {
        Operand::Node(node) if matches!(node.kind(), "array" | "hash") => {
            super::nodes::children(*node)
                .iter()
                .map(|child| context.source.node_text(*child))
                .collect::<Vec<_>>()
                .join(", ")
        }
        Operand::Node(node) => context.source.node_text(*node).to_owned(),
        Operand::Text(range) => elements_of(context.source.slice(range.clone())),
    };
    Some(Edit {
        start: found.span.start,
        end: found.span.end,
        replacement: format!(
            "{style}({}, {args})",
            context.source.slice(receiver.clone())
        ),
        safe: true,
    })
}

/// `children.map(&:source).join(', ')` for the array or hash the grammar left as text.
fn elements_of(source: &str) -> String {
    let inner = source
        .strip_prefix(['[', '{'])
        .and_then(|rest| rest.strip_suffix([']', '}']));
    let Some(inner) = inner else {
        return source.to_owned();
    };
    let mut depth = 0i32;
    let mut parts = Vec::new();
    let mut start = 0;
    for (index, character) in inner.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(inner[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(inner[start..].trim());
    parts.join(", ")
}

/// `variable_argument?`: `(send {str dstr} :% #autocorrectable?)`, where `autocorrectable?` holds
/// for a local variable and for a call that is not one of the safe conversions.
///
/// A bare identifier is a local variable or a receiverless call and answers the same either way,
/// except for the name of a conversion method written as a call, which no project does.
fn is_variable_argument(
    context: &RuleContext<'_>,
    found: &Formatter<'_>,
    argument: &Operand<'_>,
) -> bool {
    let literal_receiver = found.receiver.clone().is_some_and(|range| {
        let text = context.source.slice(range);
        !text.starts_with(['[', '{'])
    });
    if !literal_receiver {
        return false;
    }
    let Operand::Node(node) = argument else {
        return false;
    };
    match node.kind() {
        "identifier" => !AUTOCORRECTABLE_METHODS.contains(&context.source.node_text(*node)),
        "call" => node.child_by_field_name("method").is_none_or(|method| {
            !AUTOCORRECTABLE_METHODS.contains(&context.source.node_text(method))
        }),
        // Every other spelling of a `send`: an index, an operator, a unary minus. None of their
        // selectors is one of the safe conversions.
        "element_reference" => true,
        // A sign in front of a number is part of the literal upstream, not a call to `:-@`.
        "unary" => {
            let operator = node
                .child_by_field_name("operator")
                .map_or("", |operator| context.source.node_text(operator));
            let signed_number = matches!(operator, "-" | "+")
                && node
                    .child_by_field_name("operand")
                    .is_some_and(|operand| NUMBER_KINDS.contains(&operand.kind()));
            !signed_number && matches!(operator, "-" | "+" | "!" | "~" | "not")
        }
        "binary" => node
            .child_by_field_name("operator")
            .is_some_and(|operator| {
                super::nodes::is_operator_method(context.source.node_text(operator))
            }),
        _ => false,
    }
}
