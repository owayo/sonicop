//! Reading a conditional the way upstream's `IfNode` presents it, plus the tree walks the cops
//! around it share.

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use tree_sitter::Node;

/// Node kinds whose named children are the statements upstream folds into one `begin` node when
/// there is more than one of them.
///
/// `begin ... end` is missing on purpose: its parser counterpart is `kwbegin`, which holds its
/// statements itself rather than wrapping them in a `begin`.
const STATEMENT_CONTAINERS: &[&str] = &[
    "program",
    "then",
    "else",
    "body_statement",
    "block_body",
    "do",
    // A `rescue` or an `ensure` clause holds statements of its own; the parser wraps several of
    // them in a `begin` there, which is what makes the clause a statement container here.
    "rescue",
    "ensure",
];

/// Clause kinds a body list holds that are not statements of it.
const BODY_CLAUSES: &[&str] = &["rescue", "ensure", "else"];

/// A keyword written as an anonymous token, such as the `if` an offense is reported on or the
/// `end` a correction removes.
pub(super) fn token<'t>(node: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && kinds.contains(&child.kind_str()))
}

pub(super) fn first_line(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

pub(super) fn last_line(node: Node<'_>) -> usize {
    node.end_position().row + 1
}

/// The node and everything beneath it, in depth-first pre-order.
///
/// That is the order the index records its nodes in, so the answer is a copy of one run of it
/// rather than a stack-driven walk that opens a cursor at every node.
pub(super) fn descendants<'t>(node: Node<'t>, context: &'t RuleContext<'_>) -> Vec<Node<'t>> {
    if let Some(found) = context.named_descendants(node) {
        return found.to_vec();
    }
    let mut stack = vec![node];
    let mut found = Vec::new();
    while let Some(current) = stack.pop() {
        found.push(current);
        crate::rules::push_named_children(current, &mut stack);
    }
    found
}

/// The statements a container holds, with the clauses that are not statements of it dropped.
pub(super) fn self_statements<'t>(container: Node<'t>) -> Vec<Node<'t>> {
    super::nodes::children(container)
        .into_iter()
        .filter(|child| !BODY_CLAUSES.contains(&child.kind_str()))
        .collect()
}

pub(super) enum UpstreamParent<'t> {
    Begin(Node<'t>),
    Node(Node<'t>),
}

/// `node.parent` as upstream's parser builds it: the wrappers the grammar adds for statement lists
/// and argument lists have no counterpart there, and a list of more than one statement is a `begin`.
pub(super) fn upstream_parent<'t>(node: Node<'t>) -> Option<UpstreamParent<'t>> {
    let mut current = node;
    loop {
        let parent = current.parent()?;
        if parent.kind_str() == "parenthesized_statements" {
            return Some(UpstreamParent::Begin(parent));
        }
        if STATEMENT_CONTAINERS.contains(&parent.kind_str()) {
            if self_statements(parent).len() > 1 {
                return Some(UpstreamParent::Begin(parent));
            }
            current = parent;
            continue;
        }
        if parent.kind_str() == "argument_list" {
            current = parent;
            continue;
        }
        return Some(UpstreamParent::Node(parent));
    }
}

/// Upstream's `body` of a statement list: nothing, the one statement, or the `begin` holding them
/// all.
pub(super) enum Body<'t> {
    Missing,
    One(Node<'t>),
    Begin(Vec<Node<'t>>),
}

/// The body a `then`/`else` clause or a definition's body list stands for.
///
/// A list carrying a `rescue` or `ensure` clause is one of those nodes upstream rather than a
/// `begin`, and a lone parenthesized statement is a `begin` all the same.
pub(super) fn body_of<'t>(container: Node<'t>) -> Body<'t> {
    let children = super::nodes::children(container);
    if children
        .iter()
        .any(|child| BODY_CLAUSES.contains(&child.kind_str()))
    {
        return Body::Missing;
    }
    match children.as_slice() {
        [] => Body::Missing,
        [only] if only.kind_str() == "parenthesized_statements" => {
            Body::Begin(super::nodes::children(*only))
        }
        [only] => Body::One(*only),
        several => Body::Begin(several.to_vec()),
    }
}

impl<'t> Body<'t> {
    pub(super) fn single(&self) -> Option<Node<'t>> {
        match self {
            Self::One(node) => Some(*node),
            _ => None,
        }
    }

    /// The last statement, which is the one a trailing conditional would be.
    pub(super) fn last(&self) -> Option<Node<'t>> {
        match self {
            Self::Missing => None,
            Self::One(node) => Some(*node),
            Self::Begin(statements) => statements.last().copied(),
        }
    }

    pub(super) fn is_begin(&self) -> bool {
        matches!(self, Self::Begin(_))
    }
}
