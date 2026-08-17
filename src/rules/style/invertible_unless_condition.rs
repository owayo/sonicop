use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `"unless".len()`.
const UNLESS_LENGTH: usize = 6;

/// The two spellings of each logical operator, and what each turns into.
const LOGICAL: [(&str, &str); 4] = [("&&", "||"), ("||", "&&"), ("and", "or"), ("or", "and")];

/// An `unless` whose condition can be said the other way round, which an `if` then reads better.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(inverses) = inverse_methods(context) else {
        return;
    };
    for node in context.nodes_of_any(&["unless", "unless_modifier"]) {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        if !invertible(condition, &inverses, context) {
            continue;
        }
        let Some(keyword) = keyword_range(node, condition, context) else {
            continue;
        };
        let preferred = preferred_condition(condition, &inverses, context);
        let mut edits = vec![Edit {
            start: keyword.start,
            end: keyword.end,
            replacement: "if".to_owned(),
            safe: true,
        }];
        invert(condition, &inverses, context, &mut edits);
        offenses.push(
            context
                .offense(
                    format!(
                        "Prefer `if {preferred}` over `unless {}`.",
                        context.source.node_text(condition)
                    ),
                    node.byte_range(),
                )
                .corrected_by_all(edits),
        );
    }
}

/// `invertible?`.
fn invertible(
    node: Node<'_>,
    inverses: &HashMap<String, String>,
    context: &RuleContext<'_>,
) -> bool {
    match node.kind_str() {
        "parenthesized_statements" => match super::nodes::children(node).as_slice() {
            [only] => invertible(*only, inverses, context),
            _ => false,
        },
        // `node.method?(:!)`: the grammar spells the same `(send _ :!)` as a `unary` for `!x`
        // and as a `call` for `x.!`. The `call` arm below only consults `inverse_methods`, which
        // has no entry for `!`, so without this the postfix spelling is invisible.
        _ if send_node::bang(node, context).is_some() => true,
        "binary" => {
            let Some(operator) = node
                .field("operator")
                .map(|node| context.source.node_text(node))
            else {
                return false;
            };
            if is_logical(operator) {
                return node
                    .field("left")
                    .is_some_and(|left| invertible(left, inverses, context))
                    && node
                        .field("right")
                        .is_some_and(|right| invertible(right, inverses, context));
            }
            if !inverses.contains_key(operator) {
                return false;
            }
            // `inheritance_check?`: `Foo < Bar` declares a subclass rather than compares.
            !is_inheritance_check(node, operator, context)
        }
        // A block makes the condition a `block` node upstream, which is not a `send`. Neither is a
        // call written with `&.`, which its parser gives a `csend` node of its own.
        "call" if node.field("block").is_none() && send_node::is_plain_send(node, context) => node
            .field("method")
            .is_some_and(|selector| inverses.contains_key(context.source.node_text(selector))),
        _ => false,
    }
}

/// `preferred_condition`.
fn preferred_condition(
    node: Node<'_>,
    inverses: &HashMap<String, String>,
    context: &RuleContext<'_>,
) -> String {
    match node.kind_str() {
        "parenthesized_statements" => match super::nodes::children(node).as_slice() {
            [only] => format!("({})", preferred_condition(*only, inverses, context)),
            _ => context.source.node_text(node).to_owned(),
        },
        // `return receiver_source if node.method?(:!)`: the receiver alone, whichever way the
        // `!` was written.
        _ if send_node::bang(node, context).is_some() => send_node::bang(node, context)
            .map_or_else(String::new, |found| {
                context.source.node_text(found.operand).to_owned()
            }),
        "binary" => {
            let operator = node
                .field("operator")
                .map_or("", |node| context.source.node_text(node));
            let (Some(left), Some(right)) = (node.field("left"), node.field("right")) else {
                return context.source.node_text(node).to_owned();
            };
            if is_logical(operator) {
                return format!(
                    "{} {} {}",
                    operand(node, left, inverses, context),
                    inverse_operator(operator),
                    operand(node, right, inverses, context)
                );
            }
            format!(
                "{} {} {}",
                context.source.node_text(left),
                inverses.get(operator).map_or(operator, String::as_str),
                context.source.node_text(right)
            )
        }
        _ => preferred_send_condition(node, inverses, context),
    }
}

/// `preferred_send_condition` for the dotted spelling.
fn preferred_send_condition(
    node: Node<'_>,
    inverses: &HashMap<String, String>,
    context: &RuleContext<'_>,
) -> String {
    let Some(selector) = node.field("method") else {
        return context.source.node_text(node).to_owned();
    };
    let name = context.source.node_text(selector);
    let inverse = inverses.get(name).map_or(name, String::as_str);
    let receiver = node.field("receiver").map_or_else(String::new, |receiver| {
        format!("{}.", context.source.node_text(receiver))
    });
    let arguments = node
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    if arguments.is_empty() {
        return format!("{receiver}{inverse}");
    }
    let written: Vec<&str> = arguments
        .iter()
        .map(|argument| context.source.node_text(*argument))
        .collect();
    let list = written.join(", ");
    let parenthesized = node
        .field("arguments")
        .is_some_and(|node| context.source.node_text(node).starts_with('('));
    if parenthesized {
        format!("{receiver}{inverse}({list})")
    } else {
        format!("{receiver}{inverse} {list}")
    }
}

/// `preferred_operand`: an `and` inside an `or` needs parentheses once the two swap.
fn operand(
    node: Node<'_>,
    operand: Node<'_>,
    inverses: &HashMap<String, String>,
    context: &RuleContext<'_>,
) -> String {
    let preferred = preferred_condition(operand, inverses, context);
    if parenthesize(node, operand, context) {
        format!("({preferred})")
    } else {
        preferred
    }
}

/// `autocorrect`.
fn invert(
    node: Node<'_>,
    inverses: &HashMap<String, String>,
    context: &RuleContext<'_>,
    edits: &mut Vec<Edit>,
) {
    match node.kind_str() {
        "parenthesized_statements" => {
            if let [only] = super::nodes::children(node).as_slice() {
                invert(*only, inverses, context, edits);
            }
        }
        // `corrector.remove(node.loc.selector)`: the `!` goes away, whichever way it was
        // written. **The postfix spelling has to be here too.** Leaving `x.!` alone while the
        // keyword still flipped turned `foo unless x.!` into `foo if x.!` -- valid Ruby that says
        // the opposite. (Upstream removes only the `!` and leaves `foo if x.`, which is a syntax
        // error; the reparse guard then drops the whole correction, and the offense is reported
        // without one. That is the safe half of the same behaviour.)
        _ if send_node::bang(node, context).is_some() => {
            if let Some(found) = send_node::bang(node, context) {
                edits.push(Edit {
                    start: found.selector.start_byte(),
                    end: found.selector.end_byte(),
                    replacement: String::new(),
                    safe: true,
                });
            }
        }
        "binary" => {
            let Some(operator) = node.field("operator") else {
                return;
            };
            let text = context.source.node_text(operator);
            if is_logical(text) {
                edits.push(Edit {
                    start: operator.start_byte(),
                    end: operator.end_byte(),
                    replacement: inverse_operator(text).to_owned(),
                    safe: true,
                });
                for side in ["left", "right"] {
                    if let Some(child) = node.field(side) {
                        invert(child, inverses, context, edits);
                        if parenthesize(node, child, context) {
                            edits.push(insert(child.start_byte(), "("));
                            edits.push(insert(child.end_byte(), ")"));
                        }
                    }
                }
                return;
            }
            if let Some(inverse) = inverses.get(text) {
                edits.push(Edit {
                    start: operator.start_byte(),
                    end: operator.end_byte(),
                    replacement: inverse.clone(),
                    safe: true,
                });
            }
        }
        _ => {
            if let Some(selector) = node.field("method")
                && let Some(inverse) = inverses.get(context.source.node_text(selector))
            {
                edits.push(Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: inverse.clone(),
                    safe: true,
                });
            }
        }
    }
}

/// `parenthesize_inverted_operand?`.
fn parenthesize(node: Node<'_>, operand: Node<'_>, context: &RuleContext<'_>) -> bool {
    let outer = node
        .field("operator")
        .map_or("", |operator| context.source.node_text(operator));
    let inner = if operand.kind_str() == "binary" {
        operand
            .field("operator")
            .map_or("", |operator| context.source.node_text(operator))
    } else {
        ""
    };
    matches!(outer, "||" | "or") && matches!(inner, "&&" | "and")
}

/// `inheritance_check?`: `x < Bar` where the constant is not written in capitals.
fn is_inheritance_check(node: Node<'_>, operator: &str, context: &RuleContext<'_>) -> bool {
    if operator != "<" {
        return false;
    }
    let Some(argument) = node.field("right") else {
        return false;
    };
    let name = match argument.kind_str() {
        "constant" => context.source.node_text(argument),
        "scope_resolution" => match argument.field("name") {
            Some(name) => context.source.node_text(name),
            None => return false,
        },
        _ => return false,
    };
    name.to_uppercase() != name
}

/// The `unless` keyword, wherever it was written.
fn keyword_range(
    node: Node<'_>,
    condition: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<std::ops::Range<usize>> {
    if node.kind_str() == "unless" {
        return Some(node.start_byte()..node.start_byte() + UNLESS_LENGTH);
    }
    // The modifier form puts the keyword between the body and the condition.
    let body = node.field("body")?;
    let between = &context.source.text()[body.end_byte()..condition.start_byte()];
    let offset = between.find("unless")?;
    let start = body.end_byte() + offset;
    Some(start..start + UNLESS_LENGTH)
}

/// `InverseMethods`, whose keys and values are written as symbols.
fn inverse_methods(context: &RuleContext<'_>) -> Option<HashMap<String, String>> {
    let configured = context.setting::<HashMap<String, String>>("InverseMethods")?;
    Some(
        configured
            .into_iter()
            .map(|(key, value)| {
                (
                    key.trim_start_matches(':').to_owned(),
                    value.trim_start_matches(':').to_owned(),
                )
            })
            .collect(),
    )
}

fn is_logical(operator: &str) -> bool {
    LOGICAL.iter().any(|(name, _)| *name == operator)
}

fn inverse_operator(operator: &str) -> &'static str {
    LOGICAL
        .iter()
        .find(|(name, _)| *name == operator)
        .map_or("&&", |(_, inverse)| *inverse)
}

fn insert(at: usize, text: &str) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text.to_owned(),
        safe: true,
    }
}
