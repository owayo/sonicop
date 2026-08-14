use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::blocks::{BLOCK_KINDS, BlockArgs};
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// `RESTRICT_ON_SEND`.
const PREDICATES: [&str; 4] = ["any?", "all?", "none?", "one?"];

/// `KIND_METHODS`.
const KIND_METHODS: [&str; 3] = ["is_a?", "kind_of?", "instance_of?"];

/// `any? { |x| x.is_a?(Foo) }`, which the predicate can take a class for directly.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        if !PREDICATES.contains(&method) {
            continue;
        }
        let Some(block) = node
            .field("block")
            .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
        else {
            continue;
        };
        // `block_node.body&.begin_type?`: more than one statement is not just a kind check.
        let Some(body) = sole_statement(block) else {
            continue;
        };
        // The name the block's single parameter goes by, in each of the three block types.
        let Some(name) = parameter_name(block, context, &locals) else {
            continue;
        };
        let Some(klass) = kind_check(body, &name, context) else {
            continue;
        };
        let replacement = format!("{method}({})", context.source.node_text(klass));
        offenses.push(
            context
                .offense(
                    format!("Prefer `{replacement}` to `{method} {{ ... }}` with a kind check."),
                    block_range(node, block),
                )
                // `range_between(node.loc.selector.begin_pos, block_node.loc.end.end_pos)`.
                .corrected_by(Edit {
                    start: selector.start_byte(),
                    end: block.end_byte(),
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// The range upstream reports: the whole `block` node, which starts at the call it wraps.
fn block_range(node: Node<'_>, block: Node<'_>) -> std::ops::Range<usize> {
    node.start_byte()..block.end_byte()
}

/// `(send (lvar %1) %KIND_METHODS _)`: the class the block checks for.
fn kind_check<'tree>(
    body: Node<'tree>,
    name: &str,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if body.kind_str() != "call" {
        return None;
    }
    let receiver = body.field("receiver")?;
    if receiver.kind_str() != "identifier" || context.source.node_text(receiver) != name {
        return None;
    }
    if !KIND_METHODS.contains(&context.source.node_text(body.field("method")?)) {
        return None;
    }
    match super::nodes::children(body.field("arguments")?).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The single parameter's name: what was written, `_1` for a numbered block, or `it`.
fn parameter_name(
    block: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<String> {
    match BlockArgs::of(block, context, locals) {
        BlockArgs::Written(params) => match params.as_slice() {
            [only] if only.kind_str() == "identifier" => {
                Some(context.source.node_text(*only).to_owned())
            }
            _ => None,
        },
        BlockArgs::Numbered(1) => Some("_1".to_owned()),
        BlockArgs::Numbered(_) => None,
        BlockArgs::It => Some("it".to_owned()),
    }
}

/// The block's body when it holds exactly one statement.
fn sole_statement<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    let body = block.field("body")?;
    let statements: Vec<Node<'tree>> = super::nodes::children(body)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    match statements.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}
