use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, named_children};

use super::nil_methods::nil_methods;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Do not chain ordinary method call after safe navigation operator.";

/// `minimum_target_ruby_version 2.3`: the operator itself is younger than that.
const MINIMUM_VERSION: RubyVersion = RubyVersion::new(2, 3);

/// `PLUS_MINUS_METHODS`: `-foo&.bar` reads as a negation of the result rather than as a chain.
const PLUS_MINUS_METHODS: &[&str] = &["+@", "-@"];

/// `Node::COMPARISON_OPERATORS`, which decide whether the rewritten call needs parentheses.
const COMPARISON_METHODS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

/// The call the cop reports, in the shapes tree-sitter writes a `send` as.
struct Chain<'tree> {
    /// The node upstream's `send` covers.
    node: Node<'tree>,
    /// The safe-navigating receiver, whatever it was written inside of.
    safe_navigation: Node<'tree>,
    /// The method name, as `method_name` spells it.
    method: String,
    /// `loc.dot`, when the call was written with one.
    dot: Option<Range<usize>>,
    /// The arguments, for the bracket form the correction rewrites.
    arguments: Vec<Range<usize>>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM_VERSION {
        return;
    }
    let allowed = nil_methods(context);
    for node in context.nodes_of_any(&["call", "binary", "unary", "element_reference"]) {
        let Some(chain) = read_chain(node, context) else {
            continue;
        };
        if allowed.contains(&chain.method)
            || PLUS_MINUS_METHODS.contains(&chain.method.as_str())
            || !requires_safe_navigation(node, context)
            || ternary_branch(node, chain.safe_navigation, context) == Some(Branch::If)
        {
            continue;
        }
        let start = chain
            .dot
            .clone()
            .map_or_else(|| chain.safe_navigation.end_byte(), |dot| dot.start);
        let range = start..node.end_byte();
        let offense = context.offense(MSG, range.clone());
        offenses.push(
            if ternary_branch(node, chain.safe_navigation, context) == Some(Branch::Else) {
                offense
            } else {
                offense.corrected_by_all(autocorrect(&chain, &range, context))
            },
        );
    }
}

/// `bad_method?`: a plain send whose receiver safe-navigates, directly, through a block, or through
/// a `(...)` the parser keeps as a `begin`.
fn read_chain<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Chain<'tree>> {
    let (receiver, method, dot, given) = match node.kind_str() {
        "call" => {
            if !is_plain_send(node, context) {
                return None;
            }
            let method = context
                .source
                .node_text(node.field("method")?)
                .to_owned();
            let dot = node
                .field("operator")
                .map(|operator| operator.byte_range());
            (
                node.field("receiver")?,
                method,
                dot,
                arguments(node)
                    .iter()
                    .map(crate::rules::send_node::Argument::range)
                    .collect(),
            )
        }
        "binary" => {
            let operator = node.field("operator")?;
            let method = context.source.node_text(operator).to_owned();
            // `a && b` and `a || b` are `and`/`or` upstream rather than calls.
            if matches!(method.as_str(), "&&" | "and" | "||" | "or") {
                return None;
            }
            let right = node.field("right")?;
            (
                node.field("left")?,
                method,
                None,
                vec![right.byte_range()],
            )
        }
        "unary" => {
            let operator = node.field("operator")?;
            let text = context.source.node_text(operator);
            // `defined?` and `not` are keywords rather than method calls.
            if !matches!(text, "-" | "+" | "!" | "~" | "&") {
                return None;
            }
            let method = match text {
                "-" => "-@".to_owned(),
                "+" => "+@".to_owned(),
                other => other.to_owned(),
            };
            (
                node.field("operand")?,
                method,
                None,
                Vec::new(),
            )
        }
        _ => {
            let children = named_children(node);
            (
                *children.first()?,
                "[]".to_owned(),
                None,
                children[1..].iter().map(Node::byte_range).collect(),
            )
        }
    };
    let safe_navigation = safe_navigation_of(receiver, context)?;
    Some(Chain {
        node,
        safe_navigation,
        method,
        dot,
        arguments: given,
    })
}

/// The `csend` the receiver is, holds as its block's call, or wraps in parentheses.
fn safe_navigation_of<'tree>(
    receiver: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if is_safe_navigation(receiver, context) {
        return Some(receiver);
    }
    // `(begin (csend ...))`: a parenthesized sequence of exactly one safe call.
    if receiver.kind_str() == "parenthesized_statements" {
        let inner = super::statements::statements(receiver);
        return match inner.as_slice() {
            [only] if is_safe_navigation(*only, context) => Some(receiver),
            _ => None,
        };
    }
    None
}

fn is_safe_navigation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    // `obj&.foo = 3` is one `csend` upstream, where the grammar writes the setter as an assignment
    // whose target is the safe call.
    let call = if node.kind_str() == "assignment" {
        match node.field("left") {
            Some(left) => left,
            None => return false,
        }
    } else {
        node
    };
    call.kind_str() == "call"
        && call
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "&.")
}

/// `require_safe_navigation?`: `foo&.bar && foo.bar.baz` already guards the chain.
fn requires_safe_navigation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    if parent.kind_str() != "binary"
        || parent
            .field("operator")
            .is_none_or(|operator| !matches!(context.source.node_text(operator), "&&" | "and"))
    {
        return true;
    }
    if parent
        .field("right")
        .is_none_or(|right| right.id() != node.id())
    {
        return true;
    }
    let left = parent.field("left");
    let left_receiver = left.and_then(|left| left.field("receiver"));
    let right_receiver = node.field("receiver");
    match (left_receiver, right_receiver) {
        (Some(left), Some(right)) => {
            context.source.node_text(left) != context.source.node_text(right)
        }
        _ => true,
    }
}

#[derive(PartialEq, Eq)]
enum Branch {
    If,
    Else,
}

/// Whether the call is one of the branches of `foo&.bar ? foo.bar.baz : foo.bar.qux`, where the
/// condition already established that the chain is safe.
fn ternary_branch(
    node: Node<'_>,
    safe_navigation: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<Branch> {
    let parent = node.parent()?;
    if parent.kind_str() != "conditional" {
        return None;
    }
    let condition = parent.field("condition")?;
    if condition.id() != safe_navigation.id() {
        return None;
    }
    let _ = context;
    if parent
        .field("consequence")
        .is_some_and(|branch| branch.id() == node.id())
    {
        return Some(Branch::If);
    }
    parent
        .field("alternative")
        .filter(|branch| branch.id() == node.id())
        .map(|_| Branch::Else)
}

fn autocorrect(chain: &Chain<'_>, range: &Range<usize>, context: &RuleContext<'_>) -> Vec<Edit> {
    let mut source = if matches!(chain.method.as_str(), "[]" | "[]=") {
        let given: Vec<&str> = chain
            .arguments
            .iter()
            .map(|argument| context.source.slice(argument.clone()))
            .collect();
        format!("{}({})", chain.method, given.join(", "))
    } else {
        context.source.slice(range.clone()).to_owned()
    };
    if !source.starts_with('.') {
        source.insert(0, '.');
    }
    source.insert(0, '&');
    let mut edits = vec![Edit {
        start: range.start,
        end: range.end,
        replacement: source,
        safe: false,
    }];
    if requires_parentheses(chain, context) {
        edits.push(Edit {
            start: chain.node.start_byte(),
            end: chain.node.start_byte(),
            replacement: "(".to_owned(),
            safe: false,
        });
        edits.push(Edit {
            start: chain.node.end_byte(),
            end: chain.node.end_byte(),
            replacement: ")".to_owned(),
            safe: false,
        });
    }
    edits
}

/// `require_parentheses?`: an operator written inside a collection literal, or a comparison whose
/// parent would bind the rewritten call the wrong way.
fn requires_parentheses(chain: &Chain<'_>, context: &RuleContext<'_>) -> bool {
    let parent = chain.node.parent();
    if chain.dot.is_none() && parent.is_some_and(|parent| matches!(parent.kind_str(), "array" | "pair"))
    {
        return true;
    }
    if !COMPARISON_METHODS.contains(&chain.method.as_str()) {
        return false;
    }
    parent.is_some_and(|parent| match parent.kind_str() {
        "binary" => parent
            .field("operator")
            .is_some_and(|operator| {
                let text = context.source.node_text(operator);
                matches!(text, "&&" | "and" | "||" | "or") || COMPARISON_METHODS.contains(&text)
            }),
        _ => false,
    })
}
