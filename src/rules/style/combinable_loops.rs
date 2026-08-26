//! `Style/CombinableLoops`: two loops over the same collection are one loop.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Combine this loop with the previous loop.";

/// Node kinds that hold a list of statements, which is what upstream's parser builds a `begin`
/// for once more than one was written.
const STATEMENT_LISTS: &[&str] = &[
    "program",
    "body_statement",
    "block_body",
    "then",
    "else",
    "ensure",
    "do",
    "parenthesized_statements",
    "begin_block",
    "end_block",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for parent in context.nodes_of_any(STATEMENT_LISTS) {
        let statements = super::nodes::children(parent);
        // `node.parent&.begin_type?`: one statement on its own is not wrapped in a `begin`.
        if statements.len() < 2 {
            continue;
        }
        for index in 1..statements.len() {
            let node = statements[index];
            let sibling = statements[index - 1];
            let right = statements.get(index + 1).copied();
            let Some(loop_node) = Loop::new(context, node) else {
                continue;
            };
            let Some(previous) = Loop::new(context, sibling) else {
                continue;
            };
            if !loop_node.same_collection_as(context, &previous) {
                continue;
            }
            let offense = context.offense(MSG, node.byte_range());
            // Combining loops whose iteration variables differ would leave the second body
            // referring to a name that is no longer bound.
            offenses.push(match loop_node.binds_like(context, &previous) {
                false => offense,
                true => offense
                    .corrected_by_all(combine(&loop_node, &previous, right))
                    // `insert_before(node.source_range.end, ...)` hangs off an empty range at the
                    // end of the loop rather than off the loop itself.
                    .corrections_anchored_at(node.end_byte()..node.end_byte()),
            });
        }
    }
}

/// One loop: a block over a collection, or a `for`.
struct Loop<'t> {
    node: Node<'t>,
    /// The `}` or `end` that closes it.
    closing: Node<'t>,
    /// What the loop body spans, which is the statements alone.
    body: std::ops::Range<usize>,
    /// Whether the loop is a block, which is what decides whether the closing delimiter of one can
    /// stand in for the other's. A `for` has no delimiter to lend.
    block: bool,
    braces: bool,
    /// What the loop iterates over, and the arguments the call was given.
    receiver: Option<Node<'t>>,
    arguments: Vec<Node<'t>>,
    /// The names the body binds, which the two loops have to agree on to be merged.
    bindings: Option<Node<'t>>,
    method: String,
}

impl<'t> Loop<'t> {
    fn new(context: &RuleContext<'_>, node: Node<'t>) -> Option<Self> {
        if node.kind_str() == "for" {
            // The grammar puts the closing `end` inside the loop body; upstream keeps the two
            // apart, and `node.body` there is the statements alone.
            let body = node.field("body")?;
            return Some(Self {
                node,
                closing: closing(body)?,
                body: statements(body)?,
                block: false,
                braces: false,
                receiver: Some(node.field("value")?),
                arguments: Vec::new(),
                bindings: node.field("pattern"),
                method: "for".to_owned(),
            });
        }
        if node.kind_str() != "call" {
            return None;
        }
        let block = node.field("block")?;
        let method = context.source.node_text(node.field("method")?).to_owned();
        // `collection_looping_method?`.
        if !(method.starts_with("each") || method.ends_with("_each")) {
            return None;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        Some(Self {
            node,
            closing: closing(block)?,
            body: block.field("body")?.byte_range(),
            block: true,
            braces: block.kind_str() == "block",
            receiver: node.field("receiver"),
            arguments,
            bindings: block.field("parameters"),
            method,
        })
    }

    /// `same_collection_looping_block?` / `same_collection_looping_for?`.
    fn same_collection_as(&self, context: &RuleContext<'_>, other: &Self) -> bool {
        self.method == other.method
            && match (self.receiver, other.receiver) {
                (Some(left), Some(right)) => super::nodes::same_tree(context, left, right),
                (None, None) => true,
                _ => false,
            }
            && self.arguments.len() == other.arguments.len()
            && self
                .arguments
                .iter()
                .zip(&other.arguments)
                .all(|(left, right)| super::nodes::same_tree(context, *left, *right))
    }

    /// `node.arguments == node.left_sibling.arguments` / `node.variable == sibling.variable`.
    fn binds_like(&self, context: &RuleContext<'_>, other: &Self) -> bool {
        match (self.bindings, other.bindings) {
            (Some(left), Some(right)) => super::nodes::same_tree(context, left, right),
            (None, None) => true,
            _ => false,
        }
    }
}

/// `combine_with_left_sibling`: the second loop's header goes, and its body joins the first.
fn combine(node: &Loop<'_>, previous: &Loop<'_>, right: Option<Node<'_>>) -> Vec<Edit> {
    let mut edits = vec![
        remove(previous.body.end, previous.closing.end_byte()),
        remove(node.node.start_byte(), node.body.start),
    ];
    // `correct_end_of_block`: the loop that is left has to close the way the first one opened.
    if !previous.block {
        return edits;
    }
    // A third loop follows and will be merged in its own pass, which needs this one left open.
    if right.is_some_and(is_block) {
        return edits;
    }
    edits.push(remove(node.closing.start_byte(), node.closing.end_byte()));
    edits.push(Edit {
        start: node.node.end_byte(),
        end: node.node.end_byte(),
        replacement: match previous.braces {
            true => "}".to_owned(),
            false => " end".to_owned(),
        },
        safe: true,
    });
    edits
}

/// `right_sibling&.any_block_type?`: another block follows, and merging it needs this one open.
fn is_block(node: Node<'_>) -> bool {
    match node.kind_str() {
        "call" => node.field("block").is_some(),
        "lambda" => true,
        _ => false,
    }
}

fn remove(start: usize, end: usize) -> Edit {
    Edit {
        start,
        end,
        replacement: String::new(),
        safe: true,
    }
}

/// The statements a loop body holds, which is all of it but the keywords around it.
fn statements(body: Node<'_>) -> Option<std::ops::Range<usize>> {
    // `node.body` is nil for a loop holding nothing, and the `;` of `for a in [] do; end` is not a
    // statement upstream: it leaves an empty body the cop returns on.
    let statements: Vec<Node<'_>> = super::nodes::children(body)
        .into_iter()
        .filter(|child| child.kind_str() != "empty_statement")
        .collect();
    Some(statements.first()?.start_byte()..statements.last()?.end_byte())
}

fn closing<'t>(block: Node<'t>) -> Option<Node<'t>> {
    let mut cursor = block.walk();
    let children: Vec<Node<'t>> = block.children(&mut cursor).collect();
    children
        .into_iter()
        .rev()
        .find(|child| !child.is_named() && matches!(child.kind_str(), "}" | "end"))
}
