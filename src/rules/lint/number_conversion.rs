use std::ops::Range;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{self, arguments, symbol_name};

/// `CONVERSION_METHOD_CLASS_MAPPING`: the class parsing each conversion should have used.
const CONVERSION_METHOD_CLASS_MAPPING: [(&str, &str, &str); 4] = [
    ("to_i", "Integer(", ", 10)"),
    ("to_f", "Float(", ")"),
    ("to_c", "Complex(", ")"),
    ("to_r", "Rational(", ")"),
];

/// `CONVERSION_METHODS`: a receiver already produced by one of these needs no second look.
const CONVERSION_METHODS: [&str; 8] = [
    "Integer", "Float", "Complex", "Rational", "to_i", "to_f", "to_c", "to_r",
];

struct Allowed {
    methods: Vec<String>,
    patterns: Vec<Regex>,
    classes: Vec<String>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = Allowed {
        methods: context.setting("AllowedMethods").unwrap_or_default(),
        patterns: context
            .setting::<Vec<String>>("AllowedPatterns")
            .unwrap_or_default()
            .iter()
            .filter_map(|pattern| Regex::new(pattern).ok())
            .collect(),
        classes: context
            .setting("AllowedClasses")
            .unwrap_or_else(|| vec!["Time".to_owned(), "DateTime".to_owned()]),
    };
    // `ignore_node`: the calls a correction has already rewritten. A conversion written inside one
    // is still reported, but rewriting it as well would work on text that no longer exists, so the
    // offense goes out without a correction.
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&["call", "binary", "element_reference"]) {
        if node.kind_str() == "call" {
            handle_conversion_method(context, offenses, node, &allowed, &mut ignored);
        }
        handle_as_symbol(context, offenses, node);
    }
}

/// `IgnoredNode#part_of_ignored_node?`, which compares offsets rather than identity.
fn part_of_ignored_node(node: Node<'_>, ignored: &[Range<usize>]) -> bool {
    let range = node.byte_range();
    ignored
        .iter()
        .any(|outer| outer.start <= range.start && outer.end >= range.end)
}

/// `handle_conversion_method`: `x.to_i` and its three siblings.
fn handle_conversion_method(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    allowed: &Allowed,
    ignored: &mut Vec<Range<usize>>,
) {
    let (Some(method), Some(receiver)) = (node.field("method"), node.field("receiver")) else {
        return;
    };
    let name = context.source.node_text(method);
    let Some(&(_, open, close)) = CONVERSION_METHOD_CLASS_MAPPING
        .iter()
        .find(|(conversion, _, _)| *conversion == name)
    else {
        return;
    };
    // `(call $_ $_)`: the pattern leaves no room for arguments.
    if !arguments(node).is_empty() || allow_receiver(receiver, context, allowed) {
        return;
    }
    let corrected = format!("{open}{}{close}", context.source.node_text(receiver));
    let operator = node
        .field("operator")
        .map_or(".", |operator| context.source.node_text(operator));
    let message = format!(
        "Replace unsafe number conversion with number class parsing, instead of using \
         `{}{operator}{name}`, use stricter `{corrected}`.",
        context.source.node_text(receiver),
    );
    let range = node.byte_range();
    let offense = context.offense(message, range.clone());
    // `safe_navigation?`: rewriting `x&.to_i` would call the parser on a `nil` the chain meant to
    // let through.
    offenses.push(
        if uses_safe_navigation(node, context) || part_of_ignored_node(node, ignored) {
            offense
        } else {
            ignored.push(range.clone());
            offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: corrected,
                safe: true,
            })
        },
    );
}

/// `handle_as_symbol`: `map(&:to_i)`, which becomes a block doing the parsing.
fn handle_as_symbol(context: &RuleContext<'_>, offenses: &mut Vec<Offense>, node: Node<'_>) {
    let Some(argument) = single_argument(node) else {
        return;
    };
    // `{(sym M) (block_pass (sym M))}`.
    let symbol = match argument.kind_str() {
        "block_argument" => argument.named_child(0),
        _ => Some(argument),
    };
    let Some(name) = symbol.and_then(|symbol| symbol_name(symbol, context)) else {
        return;
    };
    let Some(&(_, open, close)) = CONVERSION_METHOD_CLASS_MAPPING
        .iter()
        .find(|(conversion, _, _)| *conversion == name)
    else {
        return;
    };
    let corrected = format!("{{ |i| {open}i{close} }}");
    let message = format!(
        "Replace unsafe number conversion with number class parsing, instead of using `{}`, use \
         stricter `{corrected}`.",
        context.source.node_text(argument),
    );
    let mut edits = Vec::new();
    // `remove_parentheses`: the block takes the place the argument list had.
    if let Some(list) = node.field("arguments")
        && context.source.node_text(list).starts_with('(')
    {
        edits.push(Edit {
            start: list.start_byte(),
            end: list.start_byte() + 1,
            replacement: " ".to_owned(),
            safe: true,
        });
        edits.push(Edit {
            start: list.end_byte() - 1,
            end: list.end_byte(),
            replacement: String::new(),
            safe: true,
        });
    }
    edits.push(Edit {
        start: argument.start_byte(),
        end: argument.end_byte(),
        replacement: corrected,
        safe: true,
    });
    offenses.push(
        context
            .offense(message, node.byte_range())
            .corrected_by_all(edits),
    );
}

/// The one argument `(call _ $_ ${...} ...)` asks for, and the receiver it asks to be there.
///
/// An operator and an index are calls upstream too, so `conversion == :to_f` and `hash[:to_i]` both
/// match the pattern. The grammar writes their argument in a field of its own rather than in an
/// argument list.
fn single_argument<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "call" => {
            node.field("receiver")?;
            let call_arguments = arguments(node);
            let [only] = call_arguments.as_slice() else {
                return None;
            };
            Some(only.first())
        }
        "binary" => node.field("right"),
        "element_reference" => {
            let object = node.field("object")?;
            let mut indices = send_node::named_children(node)
                .into_iter()
                .filter(|child| child.id() != object.id());
            let only = indices.next()?;
            indices.next().is_none().then_some(only)
        }
        _ => None,
    }
}

/// `safe_navigation?`: the call itself, or anything below it, written with `&.`.
fn uses_safe_navigation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "call"
            && current
                .field("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "&.")
        {
            return true;
        }
        crate::rules::push_named_children_in(current, context, &mut stack);
    }
    false
}

/// `allow_receiver?`: a number, something a conversion already produced, or an allowed class.
fn allow_receiver(receiver: Node<'_>, context: &RuleContext<'_>, allowed: &Allowed) -> bool {
    if matches!(
        receiver.kind_str(),
        "integer" | "float" | "rational" | "complex"
    ) {
        return true;
    }
    if receiver.kind_str() == "call"
        && let Some(method) = receiver.field("method")
    {
        let name = context.source.node_text(method);
        if CONVERSION_METHODS.contains(&name)
            || allowed.methods.iter().any(|method| method == name)
            || allowed
                .patterns
                .iter()
                .any(|pattern| pattern.is_match(name))
        {
            return true;
        }
    }
    let top = top_receiver(receiver);
    const_name(top, context).is_some_and(|name| allowed.classes.contains(&name))
}

/// `top_receiver`: the head of the chain, which is where a class name would stand.
fn top_receiver<'tree>(node: Node<'tree>) -> Node<'tree> {
    let mut current = node;
    while let Some(receiver) = current
        .field("receiver")
        .filter(|_| current.kind_str() == "call")
    {
        current = receiver;
    }
    current
}

/// `const_name`, which drops the leading `::` of a constant reached from the top level.
fn const_name(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match node.kind_str() {
        "constant" => Some(context.source.node_text(node).to_owned()),
        "scope_resolution" => {
            let name = context.source.node_text(node.field("name")?);
            match node.field("scope") {
                Some(scope) => Some(format!("{}::{name}", const_name(scope, context)?)),
                None => Some(name.to_owned()),
            }
        }
        _ => None,
    }
}
