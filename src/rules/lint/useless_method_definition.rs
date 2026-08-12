use tree_sitter::Node;

use super::access_modifier::{in_macro_scope, statements};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;

const MSG: &str = "Useless method definition detected.";

/// The parameter kinds `use_rest_or_optional_args?` rejects: a `restarg`, an `optarg` or a
/// `kwoptarg`. A method taking any of them can be called in ways its parent cannot, so passing the
/// call straight up is not the same as not defining it.
fn takes_rest_or_optional_argument(parameter: Node<'_>) -> bool {
    match parameter.kind() {
        "splat_parameter" | "optional_parameter" => true,
        // `x:` is a `kwarg` and `x: 1` a `kwoptarg`; only the one with a default is rejected.
        "keyword_parameter" => parameter.child_by_field_name("value").is_some(),
        _ => false,
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let parameters = parameters(node);
        if parameters
            .iter()
            .copied()
            .any(takes_rest_or_optional_argument)
        {
            continue;
        }
        // `method_definition_with_modifier?`: a definition handed to a macro -- `memoize def foo`
        // -- is that macro's business. An access modifier written with the definition as its
        // argument is not: it still only makes the method private.
        let modifier = enclosing_send(node);
        if let Some(modifier) = modifier
            && !non_bare_access_modifier(modifier, context)
        {
            continue;
        }
        if !delegating(node, &parameters, context) {
            continue;
        }
        let range = modifier.unwrap_or(node).byte_range();
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// The parameters of the definition, as `DefNode#arguments` lists them.
fn parameters<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(list) = node.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|child| child.kind() != "comment")
        .collect()
}

/// The call the definition was written as an argument of, which is what `node.parent&.send_type?`
/// asks. tree-sitter puts an argument list between the two.
fn enclosing_send<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let list = node
        .parent()
        .filter(|parent| parent.kind() == "argument_list")?;
    list.parent().filter(|call| call.kind() == "call")
}

/// `non_bare_access_modifier?`: an access modifier called with the definition as its argument. The
/// scope half of `macro?` is already given -- the call holds a method definition, so it stands
/// where a definition stands.
fn non_bare_access_modifier(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    if call.child_by_field_name("receiver").is_some() {
        return false;
    }
    call.child_by_field_name("method")
        .map(|method| context.source.node_text(method))
        .is_some_and(|name| matches!(name, "public" | "protected" | "private" | "module_function"))
        && in_macro_scope(call, context)
}

/// `delegating?`: the body is a bare `super`, or a `super` handed exactly the parameters the
/// definition declares -- compared by source, so `super(a)` delegates for `def m(a)` while
/// `super(x: x)` does not for `def m(x:)`.
fn delegating(node: Node<'_>, parameters: &[Node<'_>], context: &RuleContext<'_>) -> bool {
    let Some(body) = body(node) else {
        return false;
    };
    // A bare `super` is a `zsuper`, which passes the arguments on whatever they are.
    if body.kind() == "super" {
        return true;
    }
    // `super` with a block is a `block` node wrapping the call upstream, and a block body is not a
    // delegation.
    if body.kind() != "call"
        || body.child_by_field_name("block").is_some()
        || body
            .child_by_field_name("method")
            .is_none_or(|method| method.kind() != "super")
    {
        return false;
    }
    let sources = |nodes: &[Node<'_>]| -> Vec<String> {
        nodes
            .iter()
            .map(|node| context.source.node_text(*node).to_owned())
            .collect()
    };
    let passed: Vec<String> = arguments(body)
        .iter()
        .map(|argument| context.source.slice(argument.range()).to_owned())
        .collect();
    passed == sources(parameters)
}

/// The body upstream reads for the definition: nothing when it is empty, the statement itself when
/// there is one, and a `begin` -- which is never a `super` -- when there are more.
fn body<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let body = node.child_by_field_name("body")?;
    // An endless method's body is the expression itself, with no statement list around it.
    if body.kind() != "body_statement" {
        return Some(body);
    }
    match statements(body)?.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}
