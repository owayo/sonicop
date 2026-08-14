use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `(array (splat $_))`: a bracketed literal holding nothing but a splat.
    for node in context.nodes_of("array") {
        let children = super::nodes::children(node);
        let [only] = children.as_slice() else {
            continue;
        };
        if only.kind_str() != "splat_argument" {
            continue;
        }
        let Some(argument) = super::nodes::children(*only).into_iter().next() else {
            continue;
        };
        let written = context.source.node_text(argument);
        offenses.push(
            context
                .offense(
                    format!("Use `Array({written})` instead of `[*{written}]`."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!("Array({written})"),
                    safe: true,
                }),
        );
    }

    let locals = LocalVariables::new(context);
    for node in context.nodes_of_any(&["unless", "unless_modifier"]) {
        let Some(name) = wraps_unless_array(node, context, &locals) else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    format!("Use `Array({name})` instead of explicit `Array` check."),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: format!("{name} = Array({name})"),
                    safe: true,
                }),
        );
    }
}

/// ```text
/// (if (send (lvar $_) :is_a? (const nil? :Array)) nil?
///     (lvasgn $_ (array (lvar $_))))
/// ```
///
/// The three names have to be the same one, which is what makes the whole thing a coercion. An
/// `unless` carrying an `else` has a non-nil if-branch upstream and is not a match.
fn wraps_unless_array<'a>(
    node: Node<'_>,
    context: &'a RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<&'a str> {
    let condition = node.field("condition")?;
    let body = match node.kind_str() {
        "unless_modifier" => node.field("body")?,
        _ => {
            if node.field("alternative").is_some() {
                return None;
            }
            let consequence = node.field("consequence")?;
            match super::nodes::children(consequence).as_slice() {
                [only] => *only,
                _ => return None,
            }
        }
    };
    // `(send (lvar $_) :is_a? (const nil? :Array))`.
    if condition.kind_str() != "call" {
        return None;
    }
    let checked = condition.field("receiver")?;
    if !is_local_read(checked, locals) {
        return None;
    }
    if context.source.node_text(condition.field("method")?) != "is_a?" {
        return None;
    }
    match super::nodes::children(condition.field("arguments")?).as_slice() {
        [klass]
            if klass.kind_str() == "constant" && context.source.node_text(*klass) == "Array" => {}
        _ => return None,
    }
    // `(lvasgn $_ (array (lvar $_)))`.
    if body.kind_str() != "assignment" {
        return None;
    }
    let target = body.field("left")?;
    if target.kind_str() != "identifier" {
        return None;
    }
    let value = body.field("right")?;
    if value.kind_str() != "array" {
        return None;
    }
    let wrapped = match super::nodes::children(value).as_slice() {
        [only] => *only,
        _ => return None,
    };
    if !is_local_read(wrapped, locals) {
        return None;
    }
    let name = context.source.node_text(checked);
    (name == context.source.node_text(target) && name == context.source.node_text(wrapped))
        .then_some(name)
}

/// `(lvar _)`: an identifier that names a local variable rather than a receiverless call.
fn is_local_read(node: Node<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    node.kind_str() == "identifier" && locals.is_lvar(node)
}
