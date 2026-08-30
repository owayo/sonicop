//! `Style/MapCompactWithConditionalBlock`: mapping to `nil` and dropping it is `select`/`reject`.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// What a branch of the conditional holds, as the patterns spell the four possibilities.
enum Branch<'tree> {
    /// `nil?`: the branch was not written.
    Absent,
    /// `nil`: the literal, which only the `next`-carrying patterns accept.
    Nil,
    /// `next` on its own.
    BareNext,
    /// `next value`.
    NextValue(Node<'tree>),
    /// Anything else, which the patterns only accept as the `(lvar _)` being returned.
    Value(Node<'tree>),
}

/// Where the returned value sits, which decides which branch `truthy_branch?` compares it against.
enum Position {
    /// Directly in a branch of the conditional.
    Branch,
    /// Inside a `next` in a branch of the conditional.
    InNext,
    /// The second statement of a guard clause. The flag is whether the guard's `next` carries a
    /// value, which is what decides `select` from `reject` there.
    AfterGuard { next_has_value: bool },
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut ignored: HashSet<usize> = HashSet::new();
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        // `map { ... }.compact` reaches the cop on the `compact`, `filter_map { ... }` on itself.
        let (mapping, range, current) = match name {
            "compact" if arguments(node).is_empty() && node.field("block").is_none() => {
                let Some(mapping) = node.field("receiver") else {
                    continue;
                };
                let Some(inner) = mapping.field("method") else {
                    continue;
                };
                (
                    mapping,
                    inner.start_byte()..node.end_byte(),
                    format!("{} {{ ... }}.compact", context.source.node_text(inner)),
                )
            }
            "filter_map" => (
                node,
                selector.start_byte()..node.end_byte(),
                "filter_map { ... }".to_owned(),
            ),
            _ => continue,
        };
        let Some((parameter, condition, value, position, is_unless)) =
            conditional_block(mapping, context)
        else {
            continue;
        };
        // `returns_block_argument?`.
        if context.source.node_text(value) != context.source.node_text(parameter) {
            continue;
        }
        let method = if truthy_branch(position, value, condition, is_unless, context) {
            "select"
        } else {
            "reject"
        };
        let offense = context.offense(
            format!("Replace `{current}` with `{method}`."),
            range.clone(),
        );
        // `part_of_ignored_node?`: a chain folded from further out has already been rewritten.
        //
        // 本家の `add_offense` はブロックを走らせてから offense を積むので、ブロックの中の
        // `return` は**記録される前に**中断する。報告だけ残るのではなく、offense ごと出ない。
        // `filter_map { ... }.compact` は外側の `.compact` で 1 件報告したあと、内側の
        // `filter_map` がここに来る。
        if has_ignored_ancestor(node, context, &ignored) {
            continue;
        }
        offenses.push({
            ignored.insert(node.id());
            offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: format!(
                    "{method} {{ |{}| {} }}",
                    context.source.node_text(parameter),
                    context.source.node_text(condition)
                ),
                safe: true,
            })
        });
    }
}

/// `conditional_block`: the six shapes of a block that maps to `nil` for what it wants dropped.
fn conditional_block<'tree>(
    mapping: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>, Position, bool)> {
    if mapping.kind_str() != "call" {
        return None;
    }
    if !matches!(
        context.source.node_text(mapping.field("method")?),
        "map" | "filter_map"
    ) {
        return None;
    }
    let block = mapping.field("block")?;
    let parameters = super::nodes::children_in(block.field("parameters")?, context);
    let [parameter] = parameters.as_slice() else {
        return None;
    };
    if parameter.kind_str() != "identifier" {
        return None;
    }
    let statements = super::nodes::children_in(block.field("body")?, context);
    match statements.as_slice() {
        [only] => {
            let (condition, if_branch, else_branch, is_unless) = conditional(*only, context)?;
            let (value, position) = match (if_branch, else_branch) {
                // `(if $_ $(lvar _) {next nil?})`.
                (Branch::Value(value), Branch::BareNext | Branch::Absent) => {
                    (value, Position::Branch)
                }
                // `(if $_ {next nil?} $(lvar _))`.
                (Branch::BareNext | Branch::Absent, Branch::Value(value)) => {
                    (value, Position::Branch)
                }
                // `(if $_ (next $(lvar _)) {next nil nil?})`.
                (Branch::NextValue(value), Branch::BareNext | Branch::Nil | Branch::Absent) => {
                    (value, Position::InNext)
                }
                // `(if $_ {next nil nil?} (next $(lvar _)))`.
                (Branch::BareNext | Branch::Nil | Branch::Absent, Branch::NextValue(value)) => {
                    (value, Position::InNext)
                }
                _ => return None,
            };
            if value.kind_str() != "identifier" {
                return None;
            }
            Some((*parameter, condition, value, position, is_unless))
        }
        [guard, last] => {
            let (condition, if_branch, else_branch, is_unless) = conditional(*guard, context)?;
            match (if_branch, else_branch) {
                // `(begin {(if $_ next nil?) (if $_ nil? next)} $(lvar _))`.
                //
                // 本家の `next` は節の種別だけを見るので、`next nil` のように引数があっても
                // 当たる。`NextValue` を除くと `next nil if cond` / 空行 / 値 の形で黙る。
                (guard_branch @ (Branch::BareNext | Branch::NextValue(_)), Branch::Absent)
                | (Branch::Absent, guard_branch @ (Branch::BareNext | Branch::NextValue(_)))
                    if last.kind_str() == "identifier" =>
                {
                    let next_has_value = matches!(guard_branch, Branch::NextValue(_));
                    Some((
                        *parameter,
                        condition,
                        *last,
                        Position::AfterGuard { next_has_value },
                        is_unless,
                    ))
                }
                // `(begin {(if $_ (next $(lvar _)) nil?) (if $_ nil? (next $(lvar _)))} (nil))`.
                (Branch::NextValue(value), Branch::Absent)
                | (Branch::Absent, Branch::NextValue(value)) => (last.kind_str() == "nil"
                    && value.kind_str() == "identifier")
                    .then_some((*parameter, condition, value, Position::InNext, is_unless)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The conditional as upstream's `(if condition if_branch else_branch)`.
///
/// An `unless` keeps its own body as the `if_branch` there -- the keyword is recorded rather than
/// the branches swapped -- which is why `truthy_branch?` asks about it separately.
fn conditional<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Branch<'tree>, Branch<'tree>, bool)> {
    // 本家の `if` 節にはパーサが三項演算子も畳み込む。文法は `conditional` に分けるので、
    // 落とすと `item.bar? ? item : next` の形でまるごと黙る。
    let is_unless = match node.kind_str() {
        "if" | "if_modifier" | "conditional" => false,
        "unless" | "unless_modifier" => true,
        // `condition_node.parent.elsif?`.
        _ => return None,
    };
    let condition = node.field("condition")?;
    let (if_branch, else_branch) = match node.kind_str() {
        "if_modifier" | "unless_modifier" => (branch(node.field("body")), Branch::Absent),
        _ => (
            branch(clause(node.field("consequence"))),
            branch(clause(node.field("alternative"))),
        ),
    };
    let _ = context;
    Some((condition, if_branch, else_branch, is_unless))
}

/// The one statement a `then`/`else` clause holds.
fn clause<'tree>(clause: Option<Node<'tree>>) -> Option<Node<'tree>> {
    let clause = clause?;
    match clause.kind_str() {
        "then" | "else" => match super::nodes::children(clause).as_slice() {
            [only] => Some(*only),
            _ => None,
        },
        _ => Some(clause),
    }
}

fn branch<'tree>(node: Option<Node<'tree>>) -> Branch<'tree> {
    let Some(node) = node else {
        return Branch::Absent;
    };
    match node.kind_str() {
        "nil" => Branch::Nil,
        "next" => match super::nodes::children(node)
            .into_iter()
            .flat_map(|list| super::nodes::children(list))
            .collect::<Vec<_>>()
            .as_slice()
        {
            [only] => Branch::NextValue(*only),
            [] => Branch::BareNext,
            _ => Branch::Value(node),
        },
        _ => Branch::Value(node),
    }
}

/// `truthy_branch?`: whether the value is returned when the condition holds.
fn truthy_branch(
    position: Position,
    value: Node<'_>,
    condition: Node<'_>,
    is_unless: bool,
    context: &RuleContext<'_>,
) -> bool {
    match position {
        // `truthy_branch_for_guard?`:
        //
        //     if_node.if?  ->  if_node.if_branch.arguments.any?
        //     else         ->  if_node.if_branch.arguments.none?
        //
        // つまり `if` の番なら `next` が値を持つときが truthy、`unless` の番なら持たないときが
        // truthy。値の有無を見ないと `next nil if cond` で select と reject が入れ替わる。
        Position::AfterGuard { next_has_value } => match is_unless {
            false => next_has_value,
            true => !next_has_value,
        },
        Position::Branch | Position::InNext => {
            let Some(conditional) = context.parent(condition) else {
                return false;
            };
            let branch_holding = |node: Node<'_>| {
                node.start_byte() <= value.start_byte() && value.end_byte() <= node.end_byte()
            };
            let (if_branch, else_branch) = match conditional.kind_str() {
                "if_modifier" | "unless_modifier" => (conditional.field("body"), None),
                _ => (
                    clause(conditional.field("consequence")),
                    clause(conditional.field("alternative")),
                ),
            };
            if is_unless {
                else_branch.is_some_and(branch_holding)
            } else {
                if_branch.is_some_and(branch_holding)
            }
        }
    }
}

/// Whether an enclosing chain has already been folded.
fn has_ignored_ancestor(
    node: Node<'_>,
    context: &RuleContext<'_>,
    ignored: &HashSet<usize>,
) -> bool {
    let mut current = context.parent(node);
    while let Some(parent) = current {
        if ignored.contains(&parent.id()) {
            return true;
        }
        current = context.parent(parent);
    }
    false
}
