use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const CLASS_MSG: &str = "Prefer `Time` over `DateTime`.";
const COERCION_MSG: &str = "Do not use `#to_datetime`.";

/// `(call (const {nil? (cbase)} :DateTime) ...)` and, unless `AllowCoercion`,
/// `(call !nil? :to_datetime)`.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_coercion = context.setting::<bool>("AllowCoercion").unwrap_or(false);
    for node in context.nodes_of("call") {
        let coercion = is_coercion(node, context);
        if !is_date_time_class(node, context) && !(coercion && !allow_coercion) {
            continue;
        }
        // `historic_date?`: a call taking two arguments whose second is a `Date::SOMETHING`, which
        // only `DateTime` can express.
        if is_historic_date(node, context) {
            continue;
        }
        let offense = context.offense(
            if coercion { COERCION_MSG } else { CLASS_MSG },
            send_node::send_range(node, context),
        );
        // The coercion has no replacement; the class does, and only its name is rewritten so a
        // leading `::` survives.
        offenses.push(if coercion {
            offense
        } else {
            let name = constant_name(node.field("receiver").expect("checked"));
            offense.corrected_by(Edit {
                start: name.start_byte(),
                end: name.end_byte(),
                replacement: "Time".to_owned(),
                safe: true,
            })
        });
    }
}

/// `(call (const {nil? (cbase)} :DateTime) ...)`.
fn is_date_time_class(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("receiver")
        .is_some_and(|receiver| send_node::top_level_constant(receiver, "DateTime", context))
}

/// `(call !nil? :to_datetime)`.
fn is_coercion(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("receiver").is_some()
        && node
            .field("method")
            .is_some_and(|selector| context.source.node_text(selector) == "to_datetime")
}

/// `(call _ _ _ (const (const {nil? (cbase)} :Date) _))`: exactly two arguments, the second of them
/// a constant reached through `Date`.
fn is_historic_date(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let arguments = node
        .field("arguments")
        .map(super::nodes::children)
        .unwrap_or_default();
    let [_, second] = arguments.as_slice() else {
        return false;
    };
    if second.kind_str() != "scope_resolution" {
        return false;
    }
    second
        .field("scope")
        .is_some_and(|scope| send_node::top_level_constant(scope, "Date", context))
}

/// `node.loc.name` of a constant: the name without the `::` a `cbase` puts in front of it.
fn constant_name<'tree>(node: Node<'tree>) -> Node<'tree> {
    if node.kind_str() == "scope_resolution" {
        if let Some(name) = node.field("name") {
            return name;
        }
    }
    node
}
