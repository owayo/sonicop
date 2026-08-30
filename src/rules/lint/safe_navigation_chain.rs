use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send};

use super::nil_methods::nil_methods;
use super::node_equality;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

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
            || !requires_safe_navigation(chain.node, context)
            || ternary_branch(chain.node, chain.safe_navigation, context) == Some(Branch::If)
        {
            continue;
        }
        let start = chain
            .dot
            .clone()
            .map_or_else(|| chain.safe_navigation.end_byte(), |dot| dot.start);
        // `node.source_range.end`: **the send upstream reports, not the node the loop is on.** For
        // `x&.foo[bar] = baz` those differ -- upstream's send covers the assignment, so the range
        // reaches past `= baz` and the rewrite replaces it. Ending at the reference instead leaves
        // the old `= baz` behind and writes `x&.foo&.[]=(bar, baz) = baz`.
        // **`node` is the `send`, and a `send` does not hold its block.** The grammar hangs the
        // block off the call, so `Hash&.new.select { … }` ended the range past the closing brace
        // and reported -- and rewrote -- the whole block along with it.
        let end = match chain.node.field("block") {
            // The `send` ends where its own text does, not where the block begins -- the space
            // written between them belongs to neither.
            Some(block) => {
                let upto = &context.source.text()[..block.start_byte()];
                upto.trim_end().len()
            }
            None => chain.node.end_byte(),
        };
        let range = start..end;
        let offense = context.offense(MSG, range.clone());
        offenses.push(
            if ternary_branch(chain.node, chain.safe_navigation, context) == Some(Branch::Else) {
                offense
            } else {
                offense.corrected_by_all(autocorrect(&chain, &range, context))
            },
        );
    }
}

/// `bad_method?`: a plain send whose receiver safe-navigates, directly, through a block, or through
/// a `(...)` the parser keeps as a `begin`.
fn read_chain<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Chain<'tree>> {
    let (receiver, method, dot, given) = match node.kind_str() {
        "call" => {
            if !is_plain_send(node, context) {
                return None;
            }
            let method = context.source.node_text(node.field("method")?).to_owned();
            let dot = node.field("operator").map(|operator| operator.byte_range());
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
            (node.field("left")?, method, None, vec![right.byte_range()])
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
            (node.field("operand")?, method, None, Vec::new())
        }
        _ => {
            let children = named_children_of(node, context);
            (
                *children.first()?,
                "[]".to_owned(),
                None,
                children[1..].iter().map(Node::byte_range).collect(),
            )
        }
    };
    let safe_navigation = safe_navigation_of(receiver, context)?;
    // **An index being written to is one `[]=` send upstream and two nodes here.** The grammar
    // splits the assignment off the reference, so reading the reference alone rewrites the call
    // but leaves the `= baz` that upstream had folded into it.
    match index_target(node, context) {
        Some(IndexTarget::Assignment(assignment)) => {
            let mut arguments = given;
            arguments.push(assignment.field("right")?.byte_range());
            return Some(Chain {
                node: assignment,
                safe_navigation,
                method: "[]=".to_owned(),
                dot,
                arguments,
            });
        }
        Some(IndexTarget::Target) => {
            return Some(Chain {
                node,
                safe_navigation,
                method: "[]=".to_owned(),
                dot,
                arguments: given,
            });
        }
        None => {}
    }
    Some(Chain {
        node,
        safe_navigation,
        method,
        dot,
        arguments: given,
    })
}

/// How an index reference is being written to, when it is.
enum IndexTarget<'tree> {
    /// `x&.foo[bar] = baz`: one send whose range and arguments both take in the right-hand side,
    /// so the correction is `x&.foo&.[]=(bar, baz)`.
    Assignment(Node<'tree>),
    /// `x&.foo[bar], y = 1, 2`: a `[]=` send that stops at the reference, because the value it is
    /// given belongs to the multiple assignment rather than to this send.
    Target,
}

/// The `[]=` send upstream reads an index reference as, when the reference is being assigned to.
fn index_target<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<IndexTarget<'tree>> {
    if node.kind_str() != "element_reference" {
        return None;
    }
    let parent = node.parent_of(context)?;
    match parent.kind_str() {
        // `x&.foo[bar] += 1` stays two sends upstream -- `[]=` over `[]` -- and the cop reports the
        // read, so only the plain assignment collapses into one send here.
        "assignment" => parent
            .field("left")
            .filter(|left| left.id() == node.id())
            .map(|_| IndexTarget::Assignment(parent)),
        // A list the grammar invented for `foo(a[b], c = 1)` is not a multiple assignment, and the
        // reference inside it is being read.
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
            (!crate::rules::support::spurious_assignment_list(parent)).then_some(IndexTarget::Target)
        }
        _ => None,
    }
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
    let Some(parent) = node.parent_of(context) else {
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
    // `node.parent`: the grammar interposes a `then`/`else` between the `if` and its body, and
    // upstream's AST has neither -- the body hangs off the `if` directly.
    let mut parent = node.parent_of(context)?;
    if matches!(parent.kind_str(), "then" | "else") {
        parent = parent.parent_of(context)?;
    }
    // **`if_type?` is one node upstream and two in the grammar.** The ternary and the `if`
    // statement are both `if` there, so `if foo&.bar` guards its own body exactly as
    // `foo&.bar ? … : …` guards its branches. Reading only `conditional` reported
    // `if foo&.bar\n  foo&.bar.baz\nend`, which upstream is silent about.
    if !matches!(parent.kind_str(), "conditional" | "if" | "elsif") {
        return None;
    }
    // **The two checks upstream makes here are not the same kind of comparison.**
    //
    //   parent.condition == safe_nav        Node#== -- structural
    //   node.equal?(parent.if_branch)       identity
    //
    // The safe navigation in the branch is a *different node* that happens to be written the same
    // way as the one in the condition, so comparing it by identity never matches and neither
    // branch is ever recognised. `foo&.bar ? foo&.bar - 1 : baz` was reported where upstream says
    // nothing, and the `else` side was corrected where upstream only reports.
    let condition = parent.field("condition")?;
    if !node_equality::identical(condition, safe_navigation, context) {
        return None;
    }
    // `node.equal?(parent.if_branch)`: upstream's accessor sees through the `then`/`else`
    // wrappers the grammar interposes, so the branch is compared to what they hold.
    if parent.field("consequence").is_some_and(|branch| holds(branch, node)) {
        return Some(Branch::If);
    }
    parent
        .field("alternative")
        .filter(|branch| holds(*branch, node))
        .map(|_| Branch::Else)
}

/// Whether `branch` *is* the node, or is the single-statement `then`/`else` that wraps it.
fn holds(branch: Node<'_>, node: Node<'_>) -> bool {
    if branch.id() == node.id() {
        return true;
    }
    if !matches!(branch.kind_str(), "then" | "else") {
        return false;
    }
    let mut cursor = branch.walk();
    let mut children = branch.named_children(&mut cursor);
    matches!((children.next(), children.next()), (Some(only), None) if only.id() == node.id())
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
    let parent = chain.node.parent_of(context);
    if chain.dot.is_none()
        && parent.is_some_and(|parent| matches!(parent.kind_str(), "array" | "pair"))
    {
        return true;
    }
    if !COMPARISON_METHODS.contains(&chain.method.as_str()) {
        return false;
    }
    parent.is_some_and(|parent| match parent.kind_str() {
        "binary" => parent.field("operator").is_some_and(|operator| {
            let text = context.source.node_text(operator);
            // `logical_operator?` is true for `&&` / `||` and **false for the keyword forms**:
            // `and` binds looser than a comparison, so the rewrite needs no brackets there.
            matches!(text, "&&" | "||") || COMPARISON_METHODS.contains(&text)
        }),
        _ => false,
    })
}
