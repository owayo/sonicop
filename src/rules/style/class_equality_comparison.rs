use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Use `instance_of?%<class_argument>s` instead of comparing classes.";

/// `CLASS_NAME_METHODS`: the messages that turn a class into its name.
const CLASS_NAME_METHODS: &[&str] = &["name", "to_s", "inspect"];

/// `RESTRICT_ON_SEND`.
const COMPARISONS: &[&str] = &["==", "equal?", "eql?"];

/// `unable_to_determine_type?`: a variable or a call says nothing about the class it holds.
const UNKNOWN_TYPE_KINDS: &[&str] = &[
    "identifier",
    "instance_variable",
    "global_variable",
    "class_variable",
    "call",
    "method_call",
    "binary",
    "unary",
    "element_reference",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed_methods: Vec<String> = context
        .setting("AllowedMethods")
        .unwrap_or_else(|| vec!["==".to_owned(), "equal?".to_owned(), "eql?".to_owned()]);
    let allowed_patterns: Vec<Regex> = context
        .setting::<Vec<String>>("AllowedPatterns")
        .unwrap_or_default()
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect();

    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((receiver, argument)) = comparison(context, node) else {
            continue;
        };
        let Some((class_send, name_method)) = class_receiver(context, receiver) else {
            continue;
        };
        if is_dstr(argument) {
            continue;
        }
        if enclosing_method(context, node)
            .is_some_and(|name| is_allowed(&name, &allowed_methods, &allowed_patterns))
        {
            continue;
        }
        let Some(selector) = class_send.field("method") else {
            continue;
        };
        let range = selector.start_byte()..node.end_byte();
        let class_name = class_name(context, argument, name_method);
        let class_argument = match &class_name {
            Some(name) => format!("({name})"),
            None => String::new(),
        };
        let offense = context.offense(
            MSG.replace("%<class_argument>s", &class_argument),
            range.clone(),
        );
        offenses.push(match class_name.is_some() {
            true => offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: format!("instance_of?{class_argument}"),
                safe: true,
            }),
            false => offense,
        });
    }
}

/// `(send RECEIVER {:== :equal? :eql?} $_)`, however the grammar spells the message.
fn comparison<'t>(context: &RuleContext<'_>, node: Node<'t>) -> Option<(Node<'t>, Node<'t>)> {
    match node.kind_str() {
        "binary" => {
            let operator = node.field("operator")?;
            (context.source.node_text(operator) == "==").then_some(())?;
            Some((
                node.field("left")?,
                node.field("right")?,
            ))
        }
        "call" => {
            let method = node.field("method")?;
            COMPARISONS
                .contains(&context.source.node_text(method))
                .then_some(())?;
            let arguments = super::nodes::children(node.field("arguments")?);
            match arguments.as_slice() {
                [only] => Some((node.field("receiver")?, *only)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `{$(send _ :class) (send $(send _ :class) #class_name_method?)}`: the `.class` send, and the
/// name method written after it when there is one.
fn class_receiver<'t>(
    context: &RuleContext<'_>,
    receiver: Node<'t>,
) -> Option<(Node<'t>, Option<&'static str>)> {
    if is_send_of(context, receiver, &["class"]) {
        return Some((receiver, None));
    }
    let name_method = *CLASS_NAME_METHODS
        .iter()
        .find(|method| is_send_of(context, receiver, &[method]))?;
    let inner = receiver.field("receiver")?;
    is_send_of(context, inner, &["class"]).then_some((inner, Some(name_method)))
}

/// A call of one of `methods` taking no arguments, which is all a two-child `send` can be.
fn is_send_of(context: &RuleContext<'_>, node: Node<'_>, methods: &[&str]) -> bool {
    node.kind_str() == "call"
        && node.field("arguments").is_none()
        && node.field("block").is_none()
        && node
            .field("method")
            .is_some_and(|method| methods.contains(&context.source.node_text(method)))
}

/// `class_name`: the source `instance_of?` would take as its argument, or nothing when the
/// comparison says nothing about a class.
fn class_name(
    context: &RuleContext<'_>,
    class_node: Node<'_>,
    name_method: Option<&str>,
) -> Option<String> {
    let source = context.source.node_text(class_node);
    if name_method.is_none() {
        // `var.class == 'Foo'` compares a `Class` to a `String`, which no `instance_of?` says.
        return (!is_str(class_node)).then(|| source.to_owned());
    }
    // `x.class.name == y.class.name`: the other side names a class of its own.
    if CLASS_NAME_METHODS
        .iter()
        .any(|method| is_send_of(context, class_node, &[method]))
        && let Some(receiver) = class_node.field("receiver")
    {
        return Some(context.source.node_text(receiver).to_owned());
    }
    if is_str(class_node) {
        return Some(string_class_name(class_node, source));
    }
    (!UNKNOWN_TYPE_KINDS.contains(&class_node.kind_str())).then(|| source.to_owned())
}

/// `string_class_name`: the quoted name, qualified when the comparison sits inside a namespace.
fn string_class_name(class_node: Node<'_>, source: &str) -> String {
    let value: String = source.chars().filter(|c| *c != '"' && *c != '\'').collect();
    match require_cbase(class_node) && !value.starts_with("::") {
        true => format!("::{value}"),
        false => value,
    }
}

/// `require_cbase?`: whether a `class` or `module` encloses the comparison.
fn require_cbase(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(parent.kind_str(), "class" | "module") {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// `node.each_ancestor(:any_def).first`: the method the comparison is written in.
fn enclosing_method(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(parent.kind_str(), "method" | "singleton_method") {
            let name = parent.field("name")?;
            return Some(context.source.node_text(name).to_owned());
        }
        current = parent.parent();
    }
    None
}

fn is_allowed(name: &str, methods: &[String], patterns: &[Regex]) -> bool {
    methods.iter().any(|method| method == name) || patterns.iter().any(|it| it.is_match(name))
}

/// A `str` upstream: a literal without interpolation that fits on one line.
fn is_str(node: Node<'_>) -> bool {
    match node.kind_str() {
        "character" => true,
        "string" => !is_dstr(node),
        _ => false,
    }
}

/// A `dstr` upstream: an interpolated literal, or one written across more than one line.
fn is_dstr(node: Node<'_>) -> bool {
    if !matches!(
        node.kind_str(),
        "string" | "chained_string" | "heredoc_beginning"
    ) {
        return false;
    }
    node.kind_str() != "string"
        || node.start_position().row != node.end_position().row
        || super::nodes::children(node)
            .iter()
            .any(|child| child.kind_str() == "interpolation")
}
