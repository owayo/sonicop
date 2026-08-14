use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::blocks::{BLOCK_KINDS, BlockArgs};
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 2.6`: `to_h` took a block from then on.
const MINIMUM: RubyVersion = RubyVersion::new(2, 6);

/// `RESTRICT_ON_SEND`.
const BUILDERS: [&str; 3] = ["each_with_object", "inject", "reduce"];

/// A fold that only ever fills a hash, which `to_h { ... }` says directly.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        if !BUILDERS.contains(&method) {
            continue;
        }
        let Some(block) = node
            .field("block")
            .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
        else {
            continue;
        };
        let Some(built) = hash_builder(node, block, context, &locals) else {
            continue;
        };
        // `accumulator_used_in_expressions?`: a key or value that reads the hash back is not a
        // plain mapping.
        if reads(built.key, &built.accumulator, context)
            || reads(built.value, &built.accumulator, context)
        {
            continue;
        }
        // `nested_match?`: an inner fold of the same shape is corrected on its own pass.
        if nested_match(built.key, context, &locals) || nested_match(built.value, context, &locals)
        {
            continue;
        }
        let body = format!(
            "[{}, {}]",
            adjusted(built.key, &built, context),
            adjusted(built.value, &built, context)
        );
        let braces = context.source.node_text(block).starts_with('{');
        let replacement = match (built.numbered, braces) {
            (true, true) => format!("to_h {{ {body} }}"),
            (true, false) => do_end(node, &body, None, context),
            (false, true) => format!("to_h {{ |{}| {body} }}", built.element),
            (false, false) => do_end(node, &body, Some(&built.element), context),
        };
        let range = selector.start_byte()..block.end_byte();
        offenses.push(
            context
                .offense(
                    format!("Use `to_h {{ ... }}` instead of `{method}`."),
                    selector.byte_range(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// What the two patterns capture.
struct Built<'tree> {
    key: Node<'tree>,
    value: Node<'tree>,
    /// The name the block gave the hash it is filling.
    accumulator: String,
    /// The name the block gave the element, which becomes `to_h`'s one parameter.
    element: String,
    numbered: bool,
    /// Whether the fold was written with `inject` / `reduce`, whose numbered parameters are the
    /// other way round.
    folding: bool,
}

/// `each_with_object_to_hash?` and `inject_to_hash?`.
fn hash_builder<'tree>(
    node: Node<'tree>,
    block: Node<'tree>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Built<'tree>> {
    let folding = context.source.node_text(node.field("method")?) != "each_with_object";
    // `(hash)`: the seed has to be an empty literal.
    let arguments = super::nodes::children(node.field("arguments")?);
    match arguments.as_slice() {
        [only] if only.kind_str() == "hash" && only.named_child_count() == 0 => {}
        _ => return None,
    }
    let (accumulator, element, numbered) = match BlockArgs::of(block, context, locals) {
        BlockArgs::Written(params) => match params.as_slice() {
            [first, second]
                if first.kind_str() == "identifier" && second.kind_str() == "identifier" =>
            {
                let (first, second) = (
                    context.source.node_text(*first).to_owned(),
                    context.source.node_text(*second).to_owned(),
                );
                if folding {
                    (first, second, false)
                } else {
                    (second, first, false)
                }
            }
            _ => return None,
        },
        BlockArgs::Numbered(2) => {
            let (accumulator, element) = if folding { ("_1", "_2") } else { ("_2", "_1") };
            (accumulator.to_owned(), element.to_owned(), true)
        }
        _ => return None,
    };
    let statements = block_statements(block);
    let assignment = match (folding, statements.as_slice()) {
        // `(begin (send (lvar _hash) :[]= key value) (lvar _hash))`: the fold has to hand the hash
        // back for the next round.
        (true, [assignment, returned]) if context.source.node_text(*returned) == accumulator => {
            *assignment
        }
        (false, [assignment]) => *assignment,
        _ => return None,
    };
    let (key, value) = subscript_assignment(assignment, &accumulator, context)?;
    Some(Built {
        key,
        value,
        accumulator,
        element,
        numbered,
        folding,
    })
}

/// `(send (lvar _hash) :[]= $_key $_value)`.
fn subscript_assignment<'tree>(
    node: Node<'tree>,
    accumulator: &str,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    if node.kind_str() != "assignment" {
        return None;
    }
    let target = node.field("left")?;
    if target.kind_str() != "element_reference" {
        return None;
    }
    let object = target.field("object")?;
    if context.source.node_text(object) != accumulator {
        return None;
    }
    let indices = super::nodes::children(target);
    let [_, key] = indices.as_slice() else {
        return None;
    };
    Some((*key, node.field("right")?))
}

/// `references_variable?`.
fn reads(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "identifier" && context.source.node_text(node) == name {
        return true;
    }
    super::nodes::children(node)
        .into_iter()
        .any(|child| reads(child, name, context))
}

/// `nested_match?`.
fn nested_match(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    if node.kind_str() == "call"
        && node
            .field("method")
            .is_some_and(|selector| BUILDERS.contains(&context.source.node_text(selector)))
        && let Some(block) = node
            .field("block")
            .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
        && hash_builder(node, block, context, locals).is_some()
    {
        return true;
    }
    super::nodes::children(node)
        .into_iter()
        .any(|child| nested_match(child, context, locals))
}

/// `adjusted_source`: a numbered `inject` calls the element `_2`, and `to_h` calls it `_1`.
fn adjusted(node: Node<'_>, built: &Built<'_>, context: &RuleContext<'_>) -> String {
    let source = context.source.node_text(node);
    if built.numbered && built.folding {
        return source.replace("_2", "_1");
    }
    source.to_owned()
}

/// `do_end_replacement`.
///
/// The indentation comes from `node.source_range.column` of upstream's `block` node, which starts at
/// the receiver rather than at the `do` -- and counts from zero, while `line_column` counts from one.
fn do_end(call: Node<'_>, body: &str, argument: Option<&str>, context: &RuleContext<'_>) -> String {
    let column = context.source.line_column(call.start_byte()).1;
    let indent = " ".repeat(column.saturating_sub(1));
    let arguments = argument.map_or_else(String::new, |name| format!(" |{name}|"));
    format!("to_h do{arguments}\n{indent}  {body}\n{indent}end")
}

/// The statements a block body holds.
fn block_statements<'tree>(block: Node<'tree>) -> Vec<Node<'tree>> {
    block.field("body").map_or_else(Vec::new, |body| {
        super::nodes::children(body)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .collect()
    })
}
