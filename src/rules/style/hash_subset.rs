//! `HashSubset`: the half of `Style/HashExcept` and `Style/HashSlice` upstream keeps in a mixin.
//!
//! Both cops read the same shape -- a `reject`/`select`/`filter` whose block tests the key alone --
//! and differ only in which way round the test has to read.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `SUBSET_METHODS` and the two `ACTIVE_SUPPORT_SUBSET_METHODS` add.
const SUBSET_METHODS: &[&str] = &["==", "!=", "eql?", "include?"];
const ACTIVE_SUPPORT_METHODS: &[&str] = &["in?", "exclude?"];

/// The block body, read the way `(send {(lvar _key) $_ _ | _ $_ (lvar _key)})` reads it.
struct Test<'tree> {
    receiver: Node<'tree>,
    method: String,
    argument: Node<'tree>,
    /// Whether a `!` was written around the whole test.
    negated: bool,
}

/// Reports every `reject`/`select`/`filter` whose block only looks at the key.
///
/// `wanted` decides which of the two cops is speaking: `Style/HashExcept` wants the tests that read
/// as "drop these keys", `Style/HashSlice` the ones that read as "keep these keys".
pub(super) fn check(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    preferred_method_name: &str,
    wants_except: bool,
) {
    let active_support = context
        .setting_of::<bool>("AllCops", "ActiveSupportExtensionsEnabled")
        .unwrap_or(false);
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !matches!(name, "reject" | "select" | "filter") {
            continue;
        }
        let Some(block) = node.field("block") else {
            continue;
        };
        if !arguments(node).is_empty() {
            continue;
        }
        let parameters = super::nodes::children_in(
            match block.field("parameters") {
                Some(parameters) => parameters,
                None => continue,
            },
            context,
        );
        let [key, value] = parameters.as_slice() else {
            continue;
        };
        if key.kind_str() != "identifier" || value.kind_str() != "identifier" {
            continue;
        }
        let body = super::nodes::children_in(
            match block.field("body") {
                Some(body) => body,
                None => continue,
            },
            context,
        );
        let [statement] = body.as_slice() else {
            continue;
        };
        let Some(test) = read_test(*statement, context) else {
            continue;
        };
        if !extracts_hash_subset(&test, *key, *value, active_support, context) {
            continue;
        }
        let except_key = except_key(&test, *key, context);
        // `safe_to_register_offense?`: an equality test only names one key when that key is a
        // literal name.
        if !test.negated
            && matches!(test.method.as_str(), "==" | "!=")
            && !matches!(
                except_key.kind_str(),
                "simple_symbol" | "delimited_symbol" | "string"
            )
        {
            continue;
        }
        if semantically_except(name, &test) != wants_except {
            continue;
        }
        let key_source = key_source(except_key, context);
        let preferred = format!("{preferred_method_name}({key_source})");
        let range = selector.start_byte()..node.end_byte();
        offenses.push(
            context
                .offense(format!("Use `{preferred}` instead."), range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: preferred,
                    safe: true,
                }),
        );
    }
}

/// `extract_body_if_negated` and the two shapes of the test itself.
fn read_test<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Test<'tree>> {
    let (node, negated) = match node.kind_str() {
        "unary" if node.child(0).is_some_and(|op| op.kind_str() == "!") => {
            (node.field("operand")?, true)
        }
        _ => (node, false),
    };
    match node.kind_str() {
        "binary" => Some(Test {
            receiver: node.field("left")?,
            method: context.source.node_text(node.field("operator")?).to_owned(),
            argument: node.field("right")?,
            negated,
        }),
        "call" if node.field("block").is_none() => {
            let list = arguments(node);
            let [argument] = list.as_slice() else {
                return None;
            };
            Some(Test {
                receiver: node.field("receiver")?,
                method: context.source.node_text(node.field("method")?).to_owned(),
                argument: argument.first(),
                negated,
            })
        }
        _ => None,
    }
}

/// `extracts_hash_subset?`: whether the test names the key and nothing else.
fn extracts_hash_subset(
    test: &Test<'_>,
    key: Node<'_>,
    value: Node<'_>,
    active_support: bool,
    context: &RuleContext<'_>,
) -> bool {
    let named = |node: Node<'_>, name: Node<'_>| {
        node.kind_str() == "identifier"
            && context.source.node_text(node) == context.source.node_text(name)
    };
    // `{(lvar _key) $_ _ | _ $_ (lvar _key)}`.
    if !named(test.receiver, key) && !named(test.argument, key) {
        return false;
    }
    let supported = SUBSET_METHODS.contains(&test.method.as_str())
        || (active_support && ACTIVE_SUPPORT_METHODS.contains(&test.method.as_str()));
    if !supported || range_include(test) {
        return false;
    }
    // `slices_key?`: the collection has to be the other operand, and the value must not be read.
    match test.method.as_str() {
        "include?" | "exclude?" => {
            !named(test.receiver, value)
                && !named(test.argument, value)
                && named(test.argument, key)
        }
        "in?" => {
            !named(test.receiver, value)
                && !named(test.argument, value)
                && named(test.receiver, key)
        }
        _ => true,
    }
}

/// `range_include?`: a range membership test names no key at all.
fn range_include(test: &Test<'_>) -> bool {
    if test.argument.kind_str() == "range" {
        return true;
    }
    let mut receiver = test.receiver;
    while receiver.kind_str() == "parenthesized_statements" {
        match super::nodes::children(receiver).as_slice() {
            [only] => receiver = *only,
            _ => break,
        }
    }
    receiver.kind_str() == "range"
}

/// `except_key`: whichever operand is not the key variable.
fn except_key<'tree>(test: &Test<'tree>, key: Node<'_>, context: &RuleContext<'_>) -> Node<'tree> {
    if context.source.node_text(test.receiver) == context.source.node_text(key) {
        test.argument
    } else {
        test.receiver
    }
}

/// `semantically_except_method?`: whether the test reads as "drop these keys".
fn semantically_except(selector: &str, test: &Test<'_>) -> bool {
    // `included?`: a negated `exclude?` is an inclusion, and so is a plain `include?`/`in?`.
    let included = if test.negated {
        test.method == "exclude?"
    } else {
        matches!(test.method.as_str(), "include?" | "in?")
    };
    let not_included = if test.negated {
        matches!(test.method.as_str(), "include?" | "in?")
    } else {
        test.method == "exclude?"
    };
    // The method is read after the negation is peeled off, so `!(k == :a)` still counts as an
    // equality test here even though `safe_to_register_offense?` no longer sees one.
    if selector == "reject" {
        matches!(test.method.as_str(), "==" | "eql?") || included
    } else {
        test.method == "!=" || not_included
    }
}

/// `except_key_source`: the keys as the replacement writes them out.
fn key_source(key: Node<'_>, context: &RuleContext<'_>) -> String {
    match key.kind_str() {
        "array" => super::nodes::children_in(key, context)
            .iter()
            .map(|value| context.source.node_text(*value).to_owned())
            .collect::<Vec<_>>()
            .join(", "),
        // `percent_literal?`: the elements are written bare, so each is spelled back as a literal.
        "string_array" | "symbol_array" => super::nodes::children_in(key, context)
            .iter()
            .map(|value| decorate_source(*value, context))
            .collect::<Vec<_>>()
            .join(", "),
        kind if LITERAL_KINDS.contains(&kind) => context.source.node_text(key).to_owned(),
        _ => format!("*{}", context.source.node_text(key)),
    }
}

/// `Node#literal?`: the types upstream's parser builds for a literal value.
const LITERAL_KINDS: &[&str] = &[
    "string",
    "chained_string",
    "subshell",
    "simple_symbol",
    "delimited_symbol",
    "integer",
    "float",
    "complex",
    "rational",
    "true",
    "false",
    "nil",
    "hash",
    "range",
    "regex",
    "character",
];

/// `decorate_source`: one element of a percent literal, written back as a literal of its own.
fn decorate_source(value: Node<'_>, context: &RuleContext<'_>) -> String {
    let source = context.source.node_text(value);
    let interpolated = crate::rules::send_node::has_interpolation(value);
    match value.kind_str() {
        "bare_symbol" if interpolated => format!(":\"{source}\""),
        "bare_string" if interpolated => format!("\"{source}\""),
        "bare_symbol" => format!(":{source}"),
        "bare_string" => to_single_quoted(&decoded(value, context)),
        _ => format!("'{source}'"),
    }
}

fn decoded(value: Node<'_>, context: &RuleContext<'_>) -> String {
    super::literal::node_value(context, value)
        .map(|decoded| decoded.value)
        .unwrap_or_else(|| context.source.node_text(value).to_owned())
}

/// `to_single_quoted`.
fn to_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}
