//! `Style/RedundantInterpolation`: a string that holds nothing but one interpolation is a `to_s`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Prefer `to_s` over string interpolation.";

/// Node kinds upstream's parser reads as a variable or a reference, which stand for themselves in
/// the rewrite.
const VARIABLE_KINDS: &[&str] = &["instance_variable", "class_variable", "global_variable"];

/// Node kinds a literal a string is written inside of comes out as, where the interpolation is
/// what separates the elements rather than something that could be dropped.
const NOT_ON_ITS_OWN: &[&str] = &["chained_string", "string_array", "symbol_array"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("string") {
        let children = super::nodes::children(node);
        let [interpolation] = children.as_slice() else {
            continue;
        };
        if interpolation.kind_str() != "interpolation" {
            continue;
        }
        // `implicit_concatenation?` / `embedded_in_percent_array?`.
        if node
            .parent()
            .is_some_and(|parent| NOT_ON_ITS_OWN.contains(&parent.kind_str()))
        {
            continue;
        }
        // `"#{x}": v` keys the pair by a symbol: upstream's parser has already turned the literal
        // into a `dsym`, which this cop never sees.
        if is_symbol_key(context, node) {
            continue;
        }
        let embedded = super::nodes::children(*interpolation);
        // `use_match_pattern?`: `"#{x => y}"` binds a name rather than producing one.
        if context.target_ruby_version() > RubyVersion::new(2, 7)
            && embedded
                .iter()
                .any(|child| matches!(child.kind_str(), "match_pattern" | "test_pattern"))
        {
            continue;
        }
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by_all(autocorrect(context, node, *interpolation, &embedded)),
        );
    }
}

fn autocorrect(
    context: &RuleContext<'_>,
    node: Node<'_>,
    interpolation: Node<'_>,
    embedded: &[Node<'_>],
) -> Vec<Edit> {
    // `"#@foo"` interpolates without a `begin`: the variable is the string's only child upstream.
    let short_form = !context.source.node_text(interpolation).starts_with("#{");
    if short_form {
        if let [only] = embedded {
            return vec![replace(
                node,
                format!("{}.to_s", context.source.node_text(*only)),
            )];
        }
    }
    if let [only] = embedded {
        if let Some(source) = stands_for_itself(context, *only) {
            return vec![replace(node, format!("{source}.to_s"))];
        }
    }
    // `autocorrect_other`: the delimiters become the parentheses the `to_s` is called on.
    let Some((open, close)) = delimiters(node) else {
        return Vec::new();
    };
    let Some((embedded_open, embedded_close)) = delimiters(interpolation) else {
        return Vec::new();
    };
    vec![
        replace(open, String::new()),
        replace(close, String::new()),
        replace(embedded_open, "(".to_owned()),
        replace(embedded_close, ").to_s".to_owned()),
    ]
}

/// `single_variable_interpolation?`: what the interpolation holds can be written on its own, so
/// only `.to_s` has to be added to it.
fn stands_for_itself(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    if VARIABLE_KINDS.contains(&node.kind_str()) {
        return Some(context.source.node_text(node).to_owned());
    }
    // A bare name is a local variable or a receiverless call with no arguments; either way it
    // stands alone.
    if node.kind_str() == "identifier" {
        return Some(context.source.node_text(node).to_owned());
    }
    if node.kind_str() != "call" || !send_node::is_plain_send(node, context) {
        return None;
    }
    let method = node.field("method")?;
    // `super` is a node of its own upstream, not a `send`.
    if method.kind_str() == "super" {
        return None;
    }
    if super::nodes::is_operator_method(context.source.node_text(method)) {
        return None;
    }
    let arguments = send_node::arguments(node);
    if arguments.is_empty() {
        return Some(context.source.node_text(node).to_owned());
    }
    // `require_parentheses?`: a call written without them needs them once `.to_s` follows.
    if node
        .field("arguments")
        .and_then(|list| list.child(0))
        .is_some_and(|first| context.source.node_text(first) == "(")
    {
        return Some(context.source.node_text(node).to_owned());
    }
    let receiver = context
        .source
        .slice(node.start_byte()..method.end_byte())
        .to_owned();
    let written: Vec<&str> = arguments
        .iter()
        .map(|argument| context.source.slice(argument.range()))
        .collect();
    Some(format!("{receiver}({})", written.join(", ")))
}

/// The opening and closing delimiters of a literal, which is what upstream's `loc.begin` and
/// `loc.end` name.
fn delimiters<'t>(node: Node<'t>) -> Option<(Node<'t>, Node<'t>)> {
    let mut cursor = node.walk();
    let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
    let first = *children.first()?;
    let last = *children.last()?;
    (!first.is_named() && !last.is_named() && first.id() != last.id()).then_some((first, last))
}

fn replace(node: Node<'_>, replacement: String) -> Edit {
    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement,
        safe: true,
    }
}

/// Whether the literal is a hash key written with the `:` separator, which makes it a symbol.
fn is_symbol_key(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind_str() == "pair"
        && parent
            .field("key")
            .is_some_and(|key| key.id() == node.id())
        && parent
            .child(1)
            .is_some_and(|separator| context.source.node_text(separator) == ":")
}
