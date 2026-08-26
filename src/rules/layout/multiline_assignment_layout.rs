use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::blocks::BlockArgs;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const NEW_LINE_OFFENSE: &str = "Right hand side of multi-line assignment is on the same line as \
                                the assignment operator `=`.";
const SAME_LINE_OFFENSE: &str = "Right hand side of multi-line assignment is not on the same line \
                                 as the assignment operator `=`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let types: Vec<String> = context.setting("SupportedTypes").unwrap_or_default();
    let new_line = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "new_line".to_owned())
        != "same_line";
    let locals = LocalVariables::new(context);
    // `on_send` also reaches every plain call, whose `loc.operator` is nil and which the
    // `node.loc.operator&.source != '='` guard then drops. Only the assignments are left, and every
    // one of those the grammar writes carries its operator.
    for node in context.nodes_of_any(&["assignment", "operator_assignment"]) {
        // **`on_csend` is not aliased.** `foo&.bar = x` is a `csend` upstream and outside the
        // mixin's callbacks, while the grammar spells the assignment the same either way.
        if node
            .field("left")
            .is_some_and(|left| is_safe_navigation(left, context))
        {
            continue;
        }
        let Some(right) = node.field("right") else {
            continue;
        };
        let Some(operator) = operator(node, context) else {
            continue;
        };
        if !types.iter().any(|kind| is_type(right, kind)) {
            continue;
        }
        // `rhs.single_line?`: a block is measured by its own delimiters rather than by the span of
        // the whole expression, so `foo { 1 }\n  .bar { 2 }` counts as one line.
        let block = block_of(right, context, &locals);
        let single_line = match &block {
            Some(block) => line(block.begin, context) == line(block.end, context),
            None => line(right.start_byte(), context) == line(right.end_byte(), context),
        };
        // `return if rhs.single_line? && (!rhs.block_type? || same_line?(node, rhs.loc.begin))`: a
        // `numblock` or an `itblock` is not a `block`, so a single-line one is always let through.
        if single_line
            && match &block {
                None => true,
                Some(block) if !block.plain => true,
                Some(block) => line(block.begin, context) == line(node.start_byte(), context),
            }
        {
            continue;
        }
        let operator_line = line(operator.start_byte(), context);
        let offense = match new_line {
            // `check_new_line_offense`: the right-hand side must not open on the operator's line.
            true => {
                if operator_line != line(right.start_byte(), context) {
                    continue;
                }
                context
                    .offense(NEW_LINE_OFFENSE, node.byte_range())
                    .corrected_by(Edit {
                        start: operator.end_byte(),
                        end: operator.end_byte(),
                        replacement: "\n".to_owned(),
                        safe: true,
                    })
            }
            // `check_same_line_offense`: it must open on it, and the gap becomes one space.
            false => {
                if operator_line == line(right.start_byte(), context) {
                    continue;
                }
                context
                    .offense(SAME_LINE_OFFENSE, node.byte_range())
                    .corrected_by(Edit {
                        start: operator.end_byte(),
                        end: right.start_byte(),
                        replacement: " ".to_owned(),
                        safe: true,
                    })
            }
        };
        offenses.push(offense);
    }
}

/// Whether the assignment target is reached through `&.`, which makes the write a `csend`.
fn is_safe_navigation(left: Node<'_>, context: &RuleContext<'_>) -> bool {
    left.kind_str() == "call" && !crate::rules::send_node::is_plain_send(left, context)
}

/// Whether the right-hand side is of the named `SupportedTypes` kind.
///
/// `block` stands for the three block types upstream has, and a lambda literal is one of them. The
/// grammar spells a ternary and the modifier forms of `if` differently, but all of them are `if`
/// there.
fn is_type(node: Node<'_>, kind: &str) -> bool {
    let actual = node.kind_str();
    match kind {
        "block" => {
            (actual == "call" && node.field("block").is_some())
                || matches!(actual, "lambda" | "block" | "do_block")
        }
        "case" => actual == "case",
        "class" => actual == "class",
        "if" => matches!(
            actual,
            "if" | "unless" | "elsif" | "conditional" | "if_modifier" | "unless_modifier"
        ),
        "kwbegin" => actual == "begin",
        "module" => actual == "module",
        other => actual == other,
    }
}

/// What the right-hand side is when it is a block: where its delimiters stand, and whether upstream
/// reads it as a `block` rather than as a `numblock` or an `itblock`.
struct Block {
    /// `rhs.loc.begin`: the `{` or `do`.
    begin: usize,
    /// `rhs.loc.end`: the `}` or `end`.
    end: usize,
    /// `rhs.block_type?`, which a numbered or an `it` block does not satisfy.
    plain: bool,
}

fn block_of(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Block> {
    let delimited = match node.kind_str() {
        "call" => node.field("block")?,
        "lambda" => node.field("body")?,
        "block" | "do_block" => node,
        _ => return None,
    };
    Some(Block {
        begin: delimited.start_byte(),
        end: delimited.end_byte(),
        plain: matches!(
            BlockArgs::of(delimited, context, locals),
            BlockArgs::Written(_)
        ),
    })
}

/// `node.loc.operator`: the `=` or `+=` the assignment was written with.
fn operator<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let left = node.field("left")?;
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| !child.is_named() && child.start_byte() >= left.end_byte())
        .find(|child| context.source.node_text(*child).ends_with('='))
}

fn line(offset: usize, context: &RuleContext<'_>) -> usize {
    context.source.line_column(offset).0
}
