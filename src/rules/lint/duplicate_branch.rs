use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::node_equality::identical;
use super::statements::{body_children, statements};
use crate::rules::send_node::named_children_of;
use crate::rules::send_node::named_children_iter;

/// One branch, with what upstream would report it at.
struct Branch<'tree> {
    /// The statements the branch holds. Upstream wraps two or more in a `begin`, which is compared
    /// as one node -- so the comparison here is of the whole run.
    nodes: Vec<Node<'tree>>,
    /// `duplicate_branch.parent`, whose span is the offense unless the branch is an `else`.
    owner: Node<'tree>,
    /// The `else` keyword, for the branch that follows it.
    else_keyword: Option<Node<'tree>>,
    /// A ternary reports the branch itself, since it has no `else` keyword to point at.
    ternary: bool,
}

impl<'tree> Branch<'tree> {
    fn range(&self) -> Range<usize> {
        if self.ternary {
            return self.nodes[0].byte_range();
        }
        match self.else_keyword {
            Some(keyword) => keyword.byte_range(),
            None => self.owner.byte_range(),
        }
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_literal: bool = context.setting("IgnoreLiteralBranches").unwrap_or(false);
    let ignore_constant: bool = context.setting("IgnoreConstantBranches").unwrap_or(false);
    let ignore_else: bool = context
        .setting("IgnoreDuplicateElseBranch")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["if", "unless", "conditional", "case", "case_match"]) {
        // `on_if` skips an `elsif`, whose branches the outermost `if` already walked.
        report(
            context,
            offenses,
            branches(node, context),
            (ignore_literal, ignore_constant, ignore_else),
        );
    }
    // `on_rescue`: the clauses of one body, which the grammar keeps as siblings rather than under
    // a node of their own.
    for container in context.nodes_of_any(&["begin", "body_statement", "block_body", "do"]) {
        let clauses: Vec<Node<'_>> = named_children_of(container, context)
            .into_iter()
            .filter(|child| matches!(child.kind_str(), "rescue" | "else"))
            .collect();
        if !clauses.iter().any(|clause| clause.kind_str() == "rescue") {
            continue;
        }
        let mut collected = Vec::new();
        for clause in clauses {
            match clause.kind_str() {
                "rescue" => collected.push(Branch {
                    nodes: clause.field("body").map(statements).unwrap_or_default(),
                    owner: clause,
                    else_keyword: None,
                    ternary: false,
                }),
                _ => collected.push(Branch {
                    nodes: statements(clause),
                    owner: clause,
                    else_keyword: clause.child(0),
                    ternary: false,
                }),
            }
        }
        report(
            context,
            offenses,
            collected,
            (ignore_literal, ignore_constant, ignore_else),
        );
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    branches: Vec<Branch<'_>>,
    (ignore_literal, ignore_constant, ignore_else): (bool, bool, bool),
) {
    // `branches.compact`: a branch with nothing in it is `nil` upstream and never compared.
    let branches: Vec<Branch<'_>> = branches
        .into_iter()
        .filter(|branch| !branch.nodes.is_empty())
        .collect();
    let mut seen: Vec<&Branch<'_>> = Vec::new();
    for (index, branch) in branches.iter().enumerate() {
        if ignore_literal && is_literal_branch(branch, context, ignore_constant) {
            continue;
        }
        if ignore_constant && is_constant_branch(branch) {
            continue;
        }
        // `duplicate_else_branch?`: the last of three or more branches, written as an `else`.
        if ignore_else
            && branches.len() > 2
            && index == branches.len() - 1
            && branch.else_keyword.is_some()
        {
            continue;
        }
        if !seen.iter().any(|other| same_branch(other, branch, context)) {
            seen.push(branch);
            continue;
        }
        offenses.push(context.offense("Duplicate branch body detected.", branch.range()));
    }
}

fn same_branch(left: &Branch<'_>, right: &Branch<'_>, context: &RuleContext<'_>) -> bool {
    left.nodes.len() == right.nodes.len()
        && left
            .nodes
            .iter()
            .zip(&right.nodes)
            .all(|(one, other)| identical(*one, *other, context))
}

/// `IfNode#branches` and its siblings, paired with what each would be reported at.
fn branches<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Vec<Branch<'tree>> {
    let mut collected = Vec::new();
    match node.kind_str() {
        "case" | "case_match" => {
            let _cursor = node.walk();
            for clause in named_children_iter(node, context) {
                match clause.kind_str() {
                    "when" | "in_clause" => collected.push(Branch {
                        nodes: clause.field("body").map(statements).unwrap_or_default(),
                        owner: clause,
                        else_keyword: None,
                        ternary: false,
                    }),
                    "else" => collected.push(Branch {
                        nodes: statements(clause),
                        owner: node,
                        else_keyword: clause.child(0),
                        ternary: false,
                    }),
                    _ => {}
                }
            }
        }
        "conditional" => {
            for (field, is_else) in [("consequence", false), ("alternative", true)] {
                if let Some(branch) = node.field(field) {
                    collected.push(Branch {
                        nodes: vec![branch],
                        owner: node,
                        else_keyword: None,
                        ternary: is_else,
                    });
                }
            }
        }
        _ => collect_conditional(node, context, &mut collected),
    }
    collected
}

/// The chain an `if` opens: its own branch, then whatever the `elsif` and `else` below it hold.
fn collect_conditional<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
    collected: &mut Vec<Branch<'tree>>,
) {
    // An `elsif` is a nested `if` upstream, which `on_if` refuses to start from.
    if node.kind_str() != "elsif"
        && node
            .parent_of(context)
            .is_some_and(|parent| parent.kind_str() == "elsif")
    {
        return;
    }
    collected.push(Branch {
        nodes: node
            .field("consequence")
            .map(branch_nodes)
            .unwrap_or_default(),
        owner: node,
        else_keyword: None,
        ternary: false,
    });
    let Some(alternative) = node.field("alternative") else {
        return;
    };
    match alternative.kind_str() {
        "elsif" => collect_conditional(alternative, context, collected),
        _ => collected.push(Branch {
            nodes: branch_nodes(alternative),
            owner: node,
            else_keyword: alternative.child(0),
            ternary: false,
        }),
    }
}

fn branch_nodes<'tree>(container: Node<'tree>) -> Vec<Node<'tree>> {
    body_children(container)
}

/// `const_branch?`.
fn is_constant_branch(branch: &Branch<'_>) -> bool {
    matches!(branch.nodes.as_slice(), [only]
        if matches!(only.kind_str(), "constant" | "scope_resolution"))
}

/// `literal_branch?`: a branch built only out of values written down.
fn is_literal_branch(
    branch: &Branch<'_>,
    context: &RuleContext<'_>,
    ignore_constant: bool,
) -> bool {
    let [only] = branch.nodes.as_slice() else {
        return false;
    };
    let only = *only;
    if !is_literal(only) || only.kind_str() == "subshell" {
        return false;
    }
    if is_basic_literal(only) {
        return true;
    }
    let mut all = true;
    crate::rules::walk_named(only, context, &mut |node| {
        if node.id() == only.id() || !all {
            return;
        }
        all = is_basic_literal(node)
            || node.kind_str() == "pair"
            || (ignore_constant && matches!(node.kind_str(), "constant" | "scope_resolution"));
    });
    let _ = context;
    all
}

/// `literal?`.
fn is_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "string"
            | "chained_string"
            | "subshell"
            | "integer"
            | "float"
            | "complex"
            | "rational"
            | "simple_symbol"
            | "delimited_symbol"
            | "hash_key_symbol"
            | "character"
            | "array"
            | "string_array"
            | "symbol_array"
            | "hash"
            | "regex"
            | "true"
            | "false"
            | "nil"
            | "range"
    )
}

/// `basic_literal?`: a literal with nothing nested inside it.
fn is_basic_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "integer"
            | "float"
            | "complex"
            | "rational"
            | "simple_symbol"
            | "hash_key_symbol"
            | "character"
            | "true"
            | "false"
            | "nil"
    ) || (node.kind_str() == "string"
        && node.named_child_count() <= 1
        // `basic_literal?` is `str`, not `dstr`: an interpolated string holds a call whose value
        // nobody knows until run time. Counting the parts alone let `"#{foo}"` through, because a
        // lone `#{…}` is one child just as `foo` is.
        && !crate::rules::send_node::has_interpolation(node))
        // A regexp's parts are `str` nodes upstream; the grammar names the text inside one
        // `string_content` and puts the `/x` flags in a `regopt` of its own -- neither of which is
        // a kind this list had, so `/foo/` was never a literal branch.
        || matches!(node.kind_str(), "string_content" | "regex_flags" | "bare_string")
}
