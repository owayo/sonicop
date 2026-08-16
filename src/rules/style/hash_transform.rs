//! The four shapes `Style/HashTransformKeys` and `Style/HashTransformValues` both look for.
//!
//! Upstream writes them once in `HashTransformMethod` and lets each cop say which half of the pair
//! is the one being transformed, which is the only difference between the two.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// Which half of the pair the block rewrites.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Half {
    Key,
    Value,
}

/// `#hash_receiver?`: the calls whose result is a hash, which is what makes `transform_keys` a
/// replacement at all.
const HASH_METHODS: &[&str] = &[
    "to_h", "to_hash", "merge", "merge!", "update", "invert", "except", "tally",
];

/// The same for a call that takes a block, where the block is what builds the hash.
const HASH_BLOCK_METHODS: &[&str] = &[
    "group_by",
    "to_h",
    "tally",
    "transform_keys",
    "transform_keys!",
    "transform_values",
    "transform_values!",
];

/// One recognized shape, and everything the correction needs from it.
struct Candidate<'tree> {
    /// The span `add_offense` is given.
    ///
    /// For `map { ... }.to_h { ... }` it is **not** the whole grammar node: upstream reports the
    /// `send`, and a block hung on `to_h` belongs to the node above it.
    range: Range<usize>,
    /// The call the block hangs off, whose selector becomes `transform_keys`.
    call: Node<'tree>,
    block: Node<'tree>,
    /// How many characters of the reported node are the wrapper that goes away.
    leading: usize,
    trailing: usize,
    /// The block parameter the rewritten block keeps.
    argname: Node<'tree>,
    transforming: Node<'tree>,
    unchanged: Node<'tree>,
    description: &'static str,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>, half: Half) {
    let minimum = match half {
        Half::Key => RubyVersion::new(2, 5),
        Half::Value => RubyVersion::new(2, 4),
    };
    if context.target_ruby_version() < minimum {
        return;
    }
    let locals = LocalVariables::new(context);
    let name = match half {
        Half::Key => "transform_keys",
        Half::Value => "transform_values",
    };
    for node in context.nodes_of_any(&["call", "element_reference"]) {
        let Some(candidate) = each_with_object(context, node, half)
            .or_else(|| hash_brackets_map(context, node, half))
            .or_else(|| map_to_h(context, node, half))
            .or_else(|| to_h_block(context, node, half))
        else {
            continue;
        };
        if !is_offensive(context, &locals, &candidate) {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!("Prefer `{name}` over `{}`.", candidate.description),
                    candidate.range.clone(),
                )
                .corrected_by_all(corrections(context, &candidate, name)),
        );
    }
}

/// `handle_possible_offense`: the block has to actually rewrite the half the cop is about, and
/// nothing else.
fn is_offensive(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    candidate: &Candidate<'_>,
) -> bool {
    let argname = context.source.node_text(candidate.argname);
    // `noop_transformation?`
    if candidate.transforming.kind_str() == "identifier"
        && context.source.node_text(candidate.transforming) == argname
    {
        return false;
    }
    // `transformation_uses_both_args?`
    let unchanged = context.source.node_text(candidate.unchanged);
    if references(context, locals, candidate.transforming, unchanged) {
        return false;
    }
    // `use_transformed_argname?`
    if !references(context, locals, candidate.transforming, argname) {
        return false;
    }
    candidate.transforming.kind_str() != "splat_argument"
}

/// Whether `name` is read as a local variable anywhere strictly inside `node`.
fn references(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    node: Node<'_>,
    name: &str,
) -> bool {
    super::nodes::children(node).into_iter().any(|child| {
        send_node::any_descendant(child, &mut |inner| {
            inner.kind_str() == "identifier"
                && context.source.node_text(inner) == name
                && locals.is_lvar(inner)
        })
    })
}

/// `(block (call #hash_receiver? :each_with_object (hash)) (args (mlhs (arg _) (arg _)) (arg _memo))
/// (call (lvar _memo) :[]= _ _))`.
fn each_with_object<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    half: Half,
) -> Option<Candidate<'tree>> {
    let block = call_with_block(context, node, &["each_with_object"])?;
    empty_hash_argument(node)?;
    hash_receiver(context, node.field("receiver")?)?;
    let parameters = super::nodes::children(node_parameters(block)?);
    let [destructured, memo] = parameters.as_slice() else {
        return None;
    };
    if destructured.kind_str() != "destructured_parameter" || memo.kind_str() != "identifier" {
        return None;
    }
    let pair = super::nodes::children(*destructured);
    let [key, value] = pair.as_slice() else {
        return None;
    };
    if key.kind_str() != "identifier" || value.kind_str() != "identifier" {
        return None;
    }
    let body = single_statement(block)?;
    if body.kind_str() != "assignment" {
        return None;
    }
    let target = body.field("left")?;
    if target.kind_str() != "element_reference"
        || context.source.node_text(target.field("object")?) != context.source.node_text(*memo)
    {
        return None;
    }
    let indices = super::nodes::children(target);
    let [_, index] = indices.as_slice() else {
        return None;
    };
    let assigned = body.field("right")?;
    let memo_name = context.source.node_text(*memo);
    let (argname, transforming, unchanged) = match half {
        // `(call (lvar _memo) :[]= $!`_memo $(lvar _val))`
        Half::Key => {
            if context.source.node_text(assigned) != context.source.node_text(*value) {
                return None;
            }
            (*key, *index, assigned)
        }
        // `(call (lvar _memo) :[]= $(lvar _key) $!`_memo)`
        Half::Value => {
            if context.source.node_text(*index) != context.source.node_text(*key) {
                return None;
            }
            (*value, assigned, *index)
        }
    };
    if mentions(context, transforming, memo_name) {
        return None;
    }
    Some(Candidate {
        range: node.byte_range(),
        call: node,
        block,
        leading: 0,
        trailing: 0,
        argname,
        transforming,
        unchanged,
        description: "each_with_object",
    })
}

/// `(send (const _ :Hash) :[] (block (call #hash_receiver? {:map :collect}) ...))`.
fn hash_brackets_map<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    half: Half,
) -> Option<Candidate<'tree>> {
    if node.kind_str() != "element_reference" {
        return None;
    }
    let object = node.field("object")?;
    if !is_named_constant(context, object, "Hash") {
        return None;
    }
    let indices = super::nodes::children(node);
    let [_, mapping] = indices.as_slice() else {
        return None;
    };
    let (block, argname, transforming, unchanged) = mapping_block(context, *mapping, half)?;
    Some(Candidate {
        range: node.byte_range(),
        call: *mapping,
        block,
        leading: "Hash[".len(),
        trailing: "]".len(),
        argname,
        transforming,
        unchanged,
        description: "Hash[_.map {...}]",
    })
}

/// `(call (block (call #hash_receiver? {:map :collect}) ...) :to_h)`.
fn map_to_h<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    half: Half,
) -> Option<Candidate<'tree>> {
    if node.kind_str() != "call" || node.field("arguments").is_some() {
        return None;
    }
    if context.source.node_text(node.field("method")?) != "to_h" {
        return None;
    }
    // `(call (block ...) :to_h)`: upstream's `send` for `.to_h` **does not include a block hung on
    // it** -- the block is the node above. So `x.map { ... }.to_h { ... }` still matches, and only
    // the `map { ... }.to_h` part is rewritten. The grammar hangs the block off the call, so
    // refusing a call that has one loses the whole shape.
    let send = crate::rules::send_node::send_range(node, context);
    let mapping = node.field("receiver")?;
    let (block, argname, transforming, unchanged) = mapping_block(context, mapping, half)?;
    Some(Candidate {
        range: node.start_byte()..send.end,
        call: mapping,
        block,
        leading: 0,
        // `if node.parent&.block_type? && node.parent.send_node == node then 0`: with a block
        // hung on `to_h`, the `.to_h` has to stay -- removing it would leave the block with
        // nothing to hang off. Upstream strips the suffix only when `to_h` stands alone.
        trailing: match node.field("block").is_some() {
            true => 0,
            false => send.end - mapping.end_byte(),
        },
        argname,
        transforming,
        unchanged,
        description: "map {...}.to_h",
    })
}

/// `(block (call #hash_receiver? :to_h) (args (arg _) (arg _)) (array _ _))`.
fn to_h_block<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    half: Half,
) -> Option<Candidate<'tree>> {
    if context.target_ruby_version() < RubyVersion::new(2, 6) {
        return None;
    }
    let block = call_with_block(context, node, &["to_h"])?;
    if node.field("arguments").is_some() {
        return None;
    }
    hash_receiver(context, node.field("receiver")?)?;
    let (argname, transforming, unchanged) = pair_block(context, block, half)?;
    Some(Candidate {
        range: node.byte_range(),
        call: node,
        block,
        leading: 0,
        trailing: 0,
        argname,
        transforming,
        unchanged,
        description: "to_h {...}",
    })
}

/// A `map`/`collect` block over a hash, and the halves of the pair it builds.
fn mapping_block<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    half: Half,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>, Node<'tree>)> {
    let block = call_with_block(context, node, &["map", "collect"])?;
    if node.field("arguments").is_some() {
        return None;
    }
    hash_receiver(context, node.field("receiver")?)?;
    let (argname, transforming, unchanged) = pair_block(context, block, half)?;
    Some((block, argname, transforming, unchanged))
}

/// `(args (arg $_) (arg _val)) (array $_ $(lvar _val))`: the two parameters and the two-element
/// array the block answers with.
fn pair_block<'tree>(
    context: &RuleContext<'_>,
    block: Node<'tree>,
    half: Half,
) -> Option<(Node<'tree>, Node<'tree>, Node<'tree>)> {
    let parameters = super::nodes::children(node_parameters(block)?);
    let [key, value] = parameters.as_slice() else {
        return None;
    };
    if key.kind_str() != "identifier" || value.kind_str() != "identifier" {
        return None;
    }
    let body = single_statement(block)?;
    if body.kind_str() != "array" {
        return None;
    }
    let elements = super::nodes::children(body);
    let [first, second] = elements.as_slice() else {
        return None;
    };
    match half {
        Half::Key => (context.source.node_text(*second) == context.source.node_text(*value))
            .then_some((*key, *first, *second)),
        Half::Value => (context.source.node_text(*first) == context.source.node_text(*key))
            .then_some((*value, *second, *first)),
    }
}

/// The call's block, when the call names one of `methods` and takes one.
fn call_with_block<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
    methods: &[&str],
) -> Option<Node<'tree>> {
    if node.kind_str() != "call" {
        return None;
    }
    let block = node.field("block")?;
    let selector = node.field("method")?;
    methods
        .contains(&context.source.node_text(selector))
        .then_some(block)
}

fn node_parameters<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    block.field("parameters")
}

/// The one expression a block body holds, which is all these patterns allow.
fn single_statement<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    let body = block.field("body")?;
    match super::nodes::children(body).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `(hash)`: the single empty-hash argument `each_with_object` is given.
fn empty_hash_argument(node: Node<'_>) -> Option<()> {
    let arguments = super::nodes::children(node.field("arguments")?);
    let [only] = arguments.as_slice() else {
        return None;
    };
    (only.kind_str() == "hash" && super::nodes::children(*only).is_empty()).then_some(())
}

/// `#hash_receiver?`.
fn hash_receiver<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<()> {
    if node.kind_str() == "hash" {
        return Some(());
    }
    if node.kind_str() != "call" || !send_node::is_plain_send(node, context) {
        return None;
    }
    let selector = context.source.node_text(node.field("method")?);
    match node.field("block") {
        None => HASH_METHODS.contains(&selector).then_some(()),
        // A call that takes a block builds its hash there, and takes no arguments of its own --
        // except `each_with_object`, which takes the hash it fills in.
        Some(_) if selector == "each_with_object" => empty_hash_argument(node),
        Some(_) => (HASH_BLOCK_METHODS.contains(&selector) && node.field("arguments").is_none())
            .then_some(()),
    }
}

/// `(const _ :Hash)`: the constant itself, whatever scope it was reached through.
fn is_named_constant(context: &RuleContext<'_>, node: Node<'_>, name: &str) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == name,
        "scope_resolution" => node
            .field("name")
            .is_some_and(|inner| context.source.node_text(inner) == name),
        _ => false,
    }
}

/// Whether `name` is written anywhere in the subtree, which is what `` !`_memo `` rules out.
fn mentions(context: &RuleContext<'_>, node: Node<'_>, name: &str) -> bool {
    send_node::any_descendant(node, &mut |inner| {
        inner.kind_str() == "identifier" && context.source.node_text(inner) == name
    })
}

/// `execute_correction`: the wrapper goes, the selector is renamed, and the block keeps one
/// parameter and one expression.
fn corrections(context: &RuleContext<'_>, candidate: &Candidate<'_>, name: &str) -> Vec<Edit> {
    let range = candidate.range.clone();
    let mut edits = Vec::new();
    if candidate.leading > 0 {
        edits.push(Edit {
            start: range.start,
            end: range.start + candidate.leading,
            replacement: String::new(),
            safe: true,
        });
    }
    if candidate.trailing > 0 {
        edits.push(Edit {
            start: range.end - candidate.trailing,
            end: range.end,
            replacement: String::new(),
            safe: true,
        });
    }
    // The selector takes the argument list with it, so `each_with_object({})` becomes the new name.
    if let Some(selector) = candidate.call.field("method") {
        let end = candidate
            .call
            .field("arguments")
            .filter(|arguments| context.source.node_text(*arguments).ends_with(')'))
            .map_or(selector.end_byte(), |arguments| arguments.end_byte());
        edits.push(Edit {
            start: selector.start_byte(),
            end,
            replacement: name.to_owned(),
            safe: true,
        });
    }
    if let Some(parameters) = node_parameters(candidate.block) {
        edits.push(Edit {
            start: parameters.start_byte(),
            end: parameters.end_byte(),
            replacement: format!("|{}|", context.source.node_text(candidate.argname)),
            safe: true,
        });
    }
    if let Some(body) = candidate.block.field("body") {
        let source = context.source.node_text(candidate.transforming);
        // `transforming_body_expr.hash_type? && !transforming_body_expr.braces?`: a hash written
        // without braces has to gain them once it becomes the block's whole body.
        //
        // The grammar writes **no `hash` node at all** for the braceless form -- `[key, value: val]`
        // holds a bare `pair` -- so asking only about `hash` misses exactly the case the guard
        // exists for, and the correction comes out as `{ |val| value: val }`, which is not Ruby.
        let braceless_hash = matches!(candidate.transforming.kind_str(), "hash" | "pair")
            && !source.starts_with('{');
        let replacement = match braceless_hash {
            true => format!("{{ {source} }}"),
            false => source.to_owned(),
        };
        edits.push(Edit {
            start: body.start_byte(),
            end: body.end_byte(),
            replacement,
            safe: true,
        });
    }
    edits
}
