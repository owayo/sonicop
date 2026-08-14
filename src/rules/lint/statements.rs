//! Statement sequences, the way upstream's parser groups them.
//!
//! Upstream has no node for "the body of a method": a body holding one statement *is* that
//! statement, and a body holding several is a `begin` wrapped around them. tree-sitter instead
//! gives every body a node of its own -- `body_statement` under a `def`, `then` under an `if`, a
//! bare `do` under a `while` -- so the sequence a cop wants is the container's children, and the
//! `begin` it looks for exists only once there are two of them.
//!
//! Three sequences are wrapped even when they hold one statement: `begin ... end` and `(...)`,
//! which the parser keeps as `kwbegin` and `begin` nodes, and the parts a `rescue` or an `ensure`
//! splits a body into.

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The node kinds that hold a statement sequence.
const CONTAINERS: &[&str] = &[
    "program",
    "body_statement",
    "block_body",
    "then",
    "else",
    // The body of a `while`, `until` or `for`, which the grammar names after the optional `do`.
    "do",
    "ensure",
    "begin",
    "parenthesized_statements",
    // `BEGIN { }` and `END { }` hold their statements directly, as `preexe` and `postexe` do.
    "begin_block",
    "end_block",
    // `"#{a; b}"` puts a sequence inside the string, which the parser wraps just the same.
    "interpolation",
];

/// The kinds a container holds that are not statements of its own: a `rescue`, `else` or `ensure`
/// clause splits the body rather than joining it.
const CLAUSES: &[&str] = &["rescue", "else", "ensure"];

/// The statements a container holds, as upstream's parser would list them.
///
/// A `;` is not a statement upstream, a comment never reaches the tree at all, and the body of a
/// heredoc belongs to the string that opened it rather than to the line it was written after.
pub(super) fn statements<'tree>(container: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = container.walk();
    container
        .named_children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind_str(),
                "empty_statement" | "comment" | "heredoc_body"
            ) && !CLAUSES.contains(&child.kind_str())
        })
        .collect()
}

/// The statements a body hands out, which is not always the statements it holds.
///
/// Without a clause that is the statements themselves, however many. With one, the parser puts a
/// `rescue` or an `ensure` node between the body and them, and *that* node is what the body is --
/// which is why a `break` written inside `begin ... rescue ... end` is not one the enclosing loop
/// can see.
pub(super) fn body_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    if let Some(clause) = node
        .named_children(&mut cursor)
        .find(|child| CLAUSES.contains(&child.kind_str()))
    {
        return vec![clause];
    }
    statements(node)
}

/// Every `begin` and `kwbegin` node upstream builds, as the statements it holds.
///
/// This is what a cop written as `on_begin` walks. Only a sequence of two or more statements
/// becomes a node, except inside `begin ... end` and `(...)`, which are nodes however little they
/// hold.
pub(super) fn begin_groups<'tree>(context: &'tree RuleContext<'_>) -> Vec<Vec<Node<'tree>>> {
    begin_containers(context)
        .into_iter()
        .map(|(_, statements)| statements)
        .collect()
}

/// The same, paired with the node that holds the sequence, for the cops that ask what the `begin`
/// upstream builds is written inside of.
pub(super) fn begin_containers<'tree>(
    context: &'tree RuleContext<'_>,
) -> Vec<(Node<'tree>, Vec<Node<'tree>>)> {
    context
        .nodes_of_any(CONTAINERS)
        .filter_map(|container| {
            let statements = statements(container);
            let always = matches!(container.kind_str(), "begin" | "parenthesized_statements");
            (always || statements.len() > 1).then_some((container, statements))
        })
        .collect()
}

/// Whether the body was split by a `rescue` or an `ensure`, which puts that clause between the
/// container and the sequence upstream.
pub(super) fn has_clause(container: Node<'_>) -> bool {
    let mut cursor = container.walk();
    container
        .named_children(&mut cursor)
        .any(|child| CLAUSES.contains(&child.kind_str()))
}

/// A body as `if body.begin_type? then body.children else [body]` reads it: the statements of the
/// sequence, or the single expression the body is.
///
/// `begin ... end` is a `kwbegin` rather than a `begin` upstream, so a body written that way is one
/// expression however much it holds -- which is what tells `Lint/UnreachableLoop` apart on the
/// `begin ... end while` form it documents.
pub(super) fn body_statements<'tree>(body: Option<Node<'tree>>) -> Vec<Node<'tree>> {
    let Some(body) = body else {
        return Vec::new();
    };
    if body.kind_str() == "begin" || !CONTAINERS.contains(&body.kind_str()) {
        return vec![body];
    }
    body_children(body)
}

/// One branch of an `if` or a `case`, as the single node upstream's parser puts there.
///
/// A branch holding nothing is `nil` there and matches no pattern; a branch holding one statement
/// *is* that statement; a branch holding several is a `begin` around them, and every cop that
/// tests a branch tests such a `begin` by asking the same of its children.
pub(super) enum Branch<'tree> {
    Missing,
    One(Node<'tree>),
    Sequence(Vec<Node<'tree>>),
}

impl<'tree> Branch<'tree> {
    pub(super) fn of(container: Option<Node<'tree>>) -> Self {
        let Some(container) = container else {
            return Self::Missing;
        };
        // `elsif` is a nested `if` upstream rather than a branch body, and `begin ... end` is one
        // expression rather than a sequence.
        if !CONTAINERS.contains(&container.kind_str()) || container.kind_str() == "begin" {
            return Self::One(container);
        }
        let mut statements = statements(container);
        match statements.len() {
            0 => Self::Missing,
            1 => Self::One(statements.remove(0)),
            _ => Self::Sequence(statements),
        }
    }

    pub(super) fn exists(&self) -> bool {
        !matches!(self, Self::Missing)
    }

    /// Whether the branch satisfies a test that a `begin` passes when any of its children does,
    /// which is how both unreachable-code cops read a sequence.
    pub(super) fn any(&self, test: &mut impl FnMut(Node<'tree>) -> bool) -> bool {
        match self {
            Self::Missing => false,
            Self::One(node) => test(*node),
            Self::Sequence(nodes) => nodes.iter().any(|node| test(*node)),
        }
    }
}
