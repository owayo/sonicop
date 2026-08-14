//! `Style/InverseMethods`: `none?` rather than `!any?`, `reject` rather than `select { !… }`.

use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

/// `CLASS_COMPARISON_METHODS`.
const CLASS_COMPARISON_METHODS: &[&str] = &["<=", ">=", "<", ">"];

/// `SAFE_NAVIGATION_INCOMPATIBLE_METHODS`.
const SAFE_NAVIGATION_INCOMPATIBLE: &[&str] = &["<=", ">=", "<", ">", "any?", "none?"];

/// `EQUALITY_METHODS`, whose inverse is written in place of the operator rather than around it.
const EQUALITY_METHODS: &[&str] = &["==", "!=", "=~", "!~", "<=", ">=", "<", ">"];

/// `NEGATED_EQUALITY_METHODS`.
const NEGATED_EQUALITY_METHODS: &[&str] = &["!=", "!~"];

const DEFAULT_METHODS: &[(&str, &str)] = &[
    ("any?", "none?"),
    ("even?", "odd?"),
    ("==", "!="),
    ("=~", "!~"),
    ("<", ">="),
    (">", "<="),
];

const DEFAULT_BLOCKS: &[(&str, &str)] = &[("select", "reject"), ("select!", "reject!")];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let methods = pairs(context, "InverseMethods", DEFAULT_METHODS);
    let blocks = pairs(context, "InverseBlocks", DEFAULT_BLOCKS);
    // `ignore_node`: the negation inside a block being inverted is rewritten there, not on its own.
    let mut ignored: Vec<Range<usize>> = Vec::new();

    for node in context.nodes_of_any(&["unary", "call", "binary"]) {
        if let Some(offense) = inverse_block(context, &blocks, node, &mut ignored) {
            offenses.push(offense);
            continue;
        }
        if let Some(offense) = inverse_candidate(context, &methods, node, &ignored) {
            offenses.push(offense);
        }
    }
}

/// A cop configuration mapping symbols to symbols, which YAML writes with their leading colons,
/// merged with its own inverse the way `inverse_methods` builds its table.
fn pairs(
    context: &RuleContext<'_>,
    key: &str,
    fallback: &[(&str, &str)],
) -> HashMap<String, String> {
    let configured: Option<HashMap<String, String>> = context.setting(key);
    let mut table: HashMap<String, String> = match &configured {
        Some(configured) => configured
            .iter()
            .map(|(name, inverse)| (symbol(name), symbol(inverse)))
            .collect(),
        None => fallback
            .iter()
            .map(|(name, inverse)| ((*name).to_owned(), (*inverse).to_owned()))
            .collect(),
    };
    for (name, inverse) in table.clone() {
        table.entry(inverse).or_insert(name);
    }
    table
}

fn symbol(name: &str) -> String {
    name.strip_prefix(':').unwrap_or(name).to_owned()
}

/// One method call as upstream's `send` presents it, whichever way the grammar wrote it.
struct Call<'t> {
    /// The span of the call itself, which stops before a block written after it.
    range: Range<usize>,
    receiver: Node<'t>,
    selector: Node<'t>,
    method: String,
    arguments: Vec<Node<'t>>,
    safe_navigation: bool,
    has_block: bool,
}

impl<'t> Call<'t> {
    fn new(context: &RuleContext<'_>, node: Node<'t>) -> Option<Self> {
        match node.kind_str() {
            "call" => {
                let selector = node.field("method")?;
                Some(Self {
                    range: send_node::send_range(node, context),
                    receiver: node.field("receiver")?,
                    selector,
                    method: context.source.node_text(selector).to_owned(),
                    arguments: send_node::arguments(node)
                        .iter()
                        .map(send_node::Argument::first)
                        .collect(),
                    safe_navigation: !send_node::is_plain_send(node, context),
                    has_block: node.field("block").is_some(),
                })
            }
            // `a == b` is a `send` upstream, named after the operator.
            "binary" => {
                let selector = node.field("operator")?;
                Some(Self {
                    range: node.byte_range(),
                    receiver: node.field("left")?,
                    selector,
                    method: context.source.node_text(selector).to_owned(),
                    arguments: vec![node.field("right")?],
                    safe_navigation: false,
                    has_block: false,
                })
            }
            _ => None,
        }
    }
}

/// `on_send`: `!foo.any?`, however the negation and the parentheses were written.
fn inverse_candidate(
    context: &RuleContext<'_>,
    methods: &HashMap<String, String>,
    node: Node<'_>,
    ignored: &[Range<usize>],
) -> Option<Offense> {
    let negation = Negation::new(context, node)?;
    // `(send (begin $(call ...)) :!)`: the parentheses are the `begin` upstream builds.
    let (operand, parenthesized) = match negation.operand.kind_str() == "parenthesized_statements" {
        true => {
            let children = super::nodes::children(negation.operand);
            let [only] = children.as_slice() else {
                return None;
            };
            (*only, true)
        }
        false => (negation.operand, false),
    };
    let call = Call::new(context, operand)?;
    // `(call $(...) $_)`: a call carrying a block takes no arguments of its own in the pattern,
    // and only the unparenthesized spelling has a pattern for a block at all -- upstream's third
    // alternative names a `call` inside the `begin`, which a block is not.
    if call.has_block && (parenthesized || !call.arguments.is_empty()) {
        return None;
    }
    let inverse = methods.get(&call.method)?;
    if negation.is_negated(context)
        || (call.safe_navigation && SAFE_NAVIGATION_INCOMPATIBLE.contains(&call.method.as_str()))
        || ignored
            .iter()
            .any(|range| range.start <= node.start_byte() && node.end_byte() <= range.end)
        || possible_class_hierarchy_check(context, &call)
    {
        return None;
    }
    let mut edits = vec![
        remove(negation.selector.start_byte()..call.range.start),
        Edit {
            start: call.selector.start_byte(),
            end: call.selector.end_byte(),
            replacement: inverse.clone(),
            safe: true,
        },
    ];
    // `remove_end_parenthesis`: the closing parenthesis goes with the opening one.
    if EQUALITY_METHODS.contains(&call.method.as_str()) || parenthesized {
        edits.push(remove(call.range.end..node.end_byte()));
    }
    Some(
        context
            .offense(
                format!("Use `{inverse}` instead of inverting `{}`.", call.method),
                node.byte_range(),
            )
            .corrected_by_all(edits),
    )
}

/// `on_block`: `select { |x| !x.foo }`, which is `reject` with the negation dropped.
fn inverse_block(
    context: &RuleContext<'_>,
    blocks: &HashMap<String, String>,
    node: Node<'_>,
    ignored: &mut Vec<Range<usize>>,
) -> Option<Offense> {
    if node.kind_str() != "call" {
        return None;
    }
    let block = node.field("block")?;
    let call = Call::new(context, node)?;
    // `(call (...) $_)`: the pattern names a receiver and no arguments.
    if !call.arguments.is_empty() {
        return None;
    }
    let inverse = blocks.get(&call.method)?;
    // A block whose result is handed to a `!` twice over is not simply inverted.
    if is_negated(context, node)
        && node
            .parent_of(context)
            .is_some_and(|parent| is_negated(context, parent))
    {
        return None;
    }
    // `next` leaves the block before its last expression, so inverting that one is not enough.
    if send_node::any_descendant(node, &mut |child| child.kind_str() == "next") {
        return None;
    }
    let body = block.field("body")?;
    let negated = *super::nodes::children(body).last()?;
    let negation = negated_expression(context, negated)?;
    ignored.push(negated.byte_range());
    let mut edits = vec![Edit {
        start: call.selector.start_byte(),
        end: call.selector.end_byte(),
        replacement: inverse.clone(),
        safe: true,
    }];
    edits.extend(negation.undo(context));
    Some(
        context
            .offense(
                format!("Use `{inverse}` instead of inverting `{}`.", call.method),
                node.byte_range(),
            )
            .corrected_by_all(edits),
    )
}

/// What the last expression of a block negates with, and how to take the negation back out.
enum Negated<'t> {
    /// `!x`, whose `!` simply goes.
    Not {
        selector: Node<'t>,
        dot: Option<Node<'t>>,
        end: usize,
    },
    /// `x != y`, whose operator loses its leading `!`.
    Comparison {
        selector: Node<'t>,
        replacement: String,
    },
}

impl Negated<'_> {
    /// `correct_inverse_selector`.
    fn undo(&self, context: &RuleContext<'_>) -> Vec<Edit> {
        match self {
            Self::Not { selector, dot, end } => {
                let mut edits = Vec::new();
                if let Some(dot) = dot {
                    edits.push(remove(dot.start_byte()..*end));
                }
                edits.push(remove(selector.byte_range()));
                let _ = context;
                edits
            }
            Self::Comparison {
                selector,
                replacement,
            } => vec![Edit {
                start: selector.start_byte(),
                end: selector.end_byte(),
                replacement: replacement.clone(),
                safe: true,
            }],
        }
    }
}

/// `{ (call ... :!) (send (...) {:!= :!~} ...) }`: the shapes of a negated block result.
fn negated_expression<'t>(context: &RuleContext<'_>, node: Node<'t>) -> Option<Negated<'t>> {
    if let Some(negation) = Negation::new(context, node) {
        return Some(Negated::Not {
            selector: negation.selector,
            dot: negation.dot,
            end: node.end_byte(),
        });
    }
    let call = Call::new(context, node)?;
    if !NEGATED_EQUALITY_METHODS.contains(&call.method.as_str()) {
        return None;
    }
    Some(Negated::Comparison {
        selector: call.selector,
        replacement: format!("={}", &call.method[1..]),
    })
}

/// `(send X :!)`: the negation, however it was written.
struct Negation<'t> {
    node: Node<'t>,
    /// The `!`, `not` or `.!` the negation is spelled with.
    selector: Node<'t>,
    /// The `.` of the `x.!` spelling, which goes with the selector.
    dot: Option<Node<'t>>,
    operand: Node<'t>,
}

impl<'t> Negation<'t> {
    fn new(context: &RuleContext<'_>, node: Node<'t>) -> Option<Self> {
        match node.kind_str() {
            "unary" => {
                let selector = node.field("operator")?;
                let written = context.source.node_text(selector);
                (written == "!" || written == "not").then(|| Self {
                    node,
                    selector,
                    dot: None,
                    operand: node.field("operand").unwrap_or(node),
                })
            }
            "call" => {
                let selector = node.field("method")?;
                (context.source.node_text(selector) == "!").then(|| Self {
                    node,
                    selector,
                    dot: node.field("operator"),
                    operand: node.field("receiver").unwrap_or(node),
                })
            }
            _ => None,
        }
    }

    /// `negated?`: the negation is itself negated, which cancels it out rather than inverting it.
    fn is_negated(&self, context: &RuleContext<'_>) -> bool {
        is_negated(context, self.node)
    }
}

fn is_negated(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.parent_of(context)
        .is_some_and(|parent| Negation::new(context, parent).is_some())
}

/// `possible_class_hierarchy_check?`: `!(Integer < Numeric)` asks about ancestry, not about order.
fn possible_class_hierarchy_check(context: &RuleContext<'_>, call: &Call<'_>) -> bool {
    if !CLASS_COMPARISON_METHODS.contains(&call.method.as_str()) {
        return false;
    }
    if is_camel_case_constant(context, call.receiver) {
        return true;
    }
    match call.arguments.as_slice() {
        [only] => is_camel_case_constant(context, *only),
        _ => false,
    }
}

/// `camel_case_constant?`: a constant whose name is not written in all capitals.
fn is_camel_case_constant(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if !matches!(node.kind_str(), "constant" | "scope_resolution") {
        return false;
    }
    let name = context.source.node_text(node).as_bytes();
    let mut index = 0;
    while index < name.len() {
        if !name[index].is_ascii_uppercase() {
            index += 1;
            continue;
        }
        let mut run = index;
        while run < name.len() && name[run].is_ascii_uppercase() {
            run += 1;
        }
        if run > index && run < name.len() && name[run].is_ascii_lowercase() {
            return true;
        }
        index = run.max(index + 1);
    }
    false
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}
