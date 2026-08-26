use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The node kinds upstream's parser all builds an `if` node for.
const IF_KINDS: &[&str] = &[
    "if",
    "unless",
    "elsif",
    "if_modifier",
    "unless_modifier",
    "conditional",
];

/// One branch of a conditional as upstream holds it: absent, a single node, or the `begin` that
/// wraps a run of statements.
#[derive(Clone, Copy)]
enum Branch<'tree> {
    Empty,
    One(Node<'tree>),
    Several(usize, usize),
}

impl<'tree> Branch<'tree> {
    fn node(self) -> Option<Node<'tree>> {
        match self {
            Self::One(node) => Some(node),
            _ => None,
        }
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&["if", "unless"]) {
        // `node.parent&.if_type?` skips an `elsif`, which upstream spells as a nested `if`.
        if upstream_parent_is_if(node) {
            continue;
        }
        if ignored
            .iter()
            .any(|range| range.start <= node.start_byte() && node.end_byte() <= range.end)
        {
            continue;
        }
        let Some(begin) = begin_token(context, node) else {
            continue;
        };
        let Some(condition) = node.field("condition") else {
            continue;
        };
        let keyword = node.kind_str();
        let branches = branches(node);
        let newline = branches
            .iter()
            .any(|branch| matches!(branch, Branch::Several(..)))
            // **`begin_type?` is true for a single parenthesized expression too.** `else(1)` is
            // one `begin` upstream, and the message asks for a newline rather than a ternary --
            // the grammar keeps the parentheses as a node of their own.
            || branches.iter().filter_map(|branch| branch.node()).any(|branch| {
                branch.kind_str() == "parenthesized_statements"
            })
            || branches
                .iter()
                .filter_map(|branch| branch.node())
                .any(|branch| returns_a_value(branch));
        let masgn_or_block = branches
            .iter()
            .filter_map(|branch| branch.node())
            .any(is_masgn_or_block);
        let else_branch = else_branch(node);
        let template = if newline {
            "use a newline instead"
        } else if else_branch
            .node()
            .is_some_and(|branch| IF_KINDS.contains(&branch.kind_str()))
            || matches!(else_branch, Branch::Several(..))
            || masgn_or_block
        {
            "use `if/else` instead"
        } else {
            "use a ternary operator instead"
        };
        let message = format!(
            "Do not use `{keyword} {};` - {template}.",
            context.source.node_text(condition)
        );
        let edit = match newline || masgn_or_block {
            true => Edit {
                start: begin.start,
                end: begin.end,
                replacement: "\n".to_owned(),
                safe: true,
            },
            false => Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: replacement(context, node, condition, keyword),
                safe: true,
            },
        };
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by(edit),
        );
        ignored.push(node.byte_range());
    }
}

/// Whether upstream's parser would have made this conditional a direct child of another one, which
/// is what `elsif` and a single-statement branch both come out as.
fn upstream_parent_is_if(node: Node<'_>) -> bool {
    let Some(branch) = node.parent() else {
        return false;
    };
    if !matches!(branch.kind_str(), "then" | "else") {
        return false;
    }
    // A branch holding more than one statement is a `begin` upstream, and that is the parent.
    if super::nodes::children(branch).len() != 1 {
        return false;
    }
    branch
        .parent()
        .is_some_and(|grandparent| IF_KINDS.contains(&grandparent.kind_str()))
}

/// `node.loc.begin`, which is the `;` or `then` that closes the condition.
fn begin_token(context: &RuleContext<'_>, node: Node<'_>) -> Option<Range<usize>> {
    let condition = node.field("condition")?;
    let following = condition.next_sibling()?;
    let token = match following.kind_str() {
        "then" => following.child(0)?,
        _ => following,
    };
    (context.source.node_text(token) == ";").then(|| token.byte_range())
}

/// `IfNode#branches`, which walks an `elsif` chain rather than reporting it as one branch.
fn branches(node: Node<'_>) -> Vec<Branch<'_>> {
    let mut out = vec![if_branch(node)];
    let mut current = node;
    loop {
        let Some(alternative) = current.field("alternative") else {
            return out;
        };
        if alternative.kind_str() != "elsif" {
            out.push(statements(alternative));
            return out;
        }
        out.push(if_branch(alternative));
        current = alternative;
    }
}

fn if_branch(node: Node<'_>) -> Branch<'_> {
    match node.field("consequence") {
        Some(consequence) => statements(consequence),
        None => Branch::Empty,
    }
}

/// `node.else_branch`: an `elsif` is one node there, however long the chain runs.
fn else_branch(node: Node<'_>) -> Branch<'_> {
    match node.field("alternative") {
        Some(alternative) if alternative.kind_str() == "elsif" => Branch::One(alternative),
        Some(alternative) => statements(alternative),
        None => Branch::Empty,
    }
}

/// What a `then` or `else` clause holds, grouped the way upstream's parser does: nothing, one node,
/// or a `begin` around the run.
fn statements(clause: Node<'_>) -> Branch<'_> {
    let written = super::nodes::children(clause);
    match written.as_slice() {
        [] => Branch::Empty,
        [only] => Branch::One(*only),
        [first, .., last] => Branch::Several(first.start_byte(), last.end_byte()),
    }
}

/// `use_return_with_argument?`: a `return` that carries a value cannot become a ternary arm.
fn returns_a_value(node: Node<'_>) -> bool {
    node.kind_str() == "return" && node.named_child_count() > 0
}

fn is_masgn_or_block(node: Node<'_>) -> bool {
    if node.kind_str() == "call" && node.field("block").is_some() {
        return true;
    }
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| left.kind_str() == "left_assignment_list")
}

fn replacement(
    context: &RuleContext<'_>,
    node: Node<'_>,
    condition: Node<'_>,
    keyword: &str,
) -> String {
    let else_branch = else_branch(node);
    if let Some(branch) = else_branch.node()
        && IF_KINDS.contains(&branch.kind_str())
    {
        return correct_elsif(context, node, condition, branch);
    }
    let then_code = branch_source(context, if_branch(node))
        .map_or_else(|| "nil".to_owned(), |source| expression(context, source));
    let else_code = branch_source(context, else_branch)
        .map_or_else(|| "nil".to_owned(), |source| expression(context, source));
    let (then_code, else_code) = match keyword {
        "unless" => (else_code, then_code),
        _ => (then_code, else_code),
    };
    format!(
        "{} ? {then_code} : {else_code}",
        ternary_condition(context, condition)
    )
}

/// `correct_elsif`: the whole conditional written out over several lines.
fn correct_elsif(
    context: &RuleContext<'_>,
    node: Node<'_>,
    condition: Node<'_>,
    else_branch: Node<'_>,
) -> String {
    let if_branch = branch_source(context, if_branch(node))
        .map(|branch| branch.0)
        .unwrap_or_default();
    format!(
        "if {}\n  {if_branch}\n{}\nend",
        context.source.node_text(condition),
        build_else_branch(context, else_branch)
            .trim_end_matches('\n')
            .to_owned()
    )
}

fn build_else_branch(context: &RuleContext<'_>, conditional: Node<'_>) -> String {
    let condition = conditional
        .field("condition")
        .map_or_else(String::new, |node| {
            context.source.node_text(node).to_owned()
        });
    let if_branch = branch_source(context, if_branch(conditional))
        .map(|branch| branch.0)
        .unwrap_or_default();
    let mut result = format!("elsif {condition}\n  {if_branch}\n");
    match else_branch(conditional) {
        Branch::Empty => {}
        branch => {
            let nested = branch
                .node()
                .filter(|node| IF_KINDS.contains(&node.kind_str()))
                .map(|node| build_else_branch(context, node));
            result.push_str(&match nested {
                Some(nested) => nested,
                None => format!(
                    "else\n  {}\n",
                    branch_source(context, branch)
                        .map(|branch| branch.0)
                        .unwrap_or_default()
                ),
            });
        }
    }
    result
}

/// The source of a branch, and the node it was written as when it was a single one.
fn branch_source<'tree>(
    context: &RuleContext<'_>,
    branch: Branch<'tree>,
) -> Option<(String, Option<Node<'tree>>)> {
    match branch {
        Branch::Empty => None,
        Branch::One(node) => Some((context.source.node_text(node).to_owned(), Some(node))),
        Branch::Several(start, end) => Some((context.source.slice(start..end).to_owned(), None)),
    }
}

/// `build_expression`: a call written without parentheses gets them, so the ternary arm parses.
fn expression(context: &RuleContext<'_>, branch: (String, Option<Node<'_>>)) -> String {
    let (source, node) = branch;
    let Some(node) = node else {
        return source;
    };
    if node.kind_str() != "call" || node.field("block").is_some() {
        return source;
    }
    let Some(selector) = node.field("method") else {
        return source;
    };
    // `arithmetic_operation?` and `:[]` / `:[]=` are all spelled as operators, which never need the
    // parentheses added.
    if selector.kind_str() == "operator" {
        return source;
    }
    let Some(list) = node.field("arguments") else {
        return source;
    };
    let arguments = super::nodes::children(list);
    let (Some(first), Some(_)) = (arguments.first(), arguments.last()) else {
        return source;
    };
    if context.source.node_text(list).starts_with('(') {
        return source;
    }
    format!(
        "{}({})",
        context.source.slice(node.start_byte()..selector.end_byte()),
        context.source.slice(first.start_byte()..node.end_byte())
    )
}

/// An assignment used as the condition has to keep its parentheses, or the ternary would be
/// assigned instead of the condition's value.
fn ternary_condition(context: &RuleContext<'_>, condition: Node<'_>) -> String {
    let source = context.source.node_text(condition);
    match matches!(condition.kind_str(), "assignment" | "operator_assignment") {
        true => format!("({source})"),
        false => source.to_owned(),
    }
}
