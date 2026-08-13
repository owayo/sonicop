//! The four shapes `Style/HashTransformKeys` and `Style/HashTransformValues` both look for.
//!
//! Upstream writes them once in `HashTransformMethod` and lets each cop say which half of the pair
//! is the one being transformed, which is the only difference between the two.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
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
    /// What `add_offense` is given.
    node: Node<'tree>,
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
                    node.byte_range(),
                )
                .corrected_by_all(corrections(context, &candidate, name)),
        );
    }
}

/// `handle_possible_offense`: the block has to actually rewrite the half the cop is about, and
/// nothing else.
fn is_offensive(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    candidate: &Candidate<'_>,
) -> bool {
    let argname = context.source.node_text(candidate.argname);
    // `noop_transformation?`
    if candidate.transforming.kind() == "identifier"
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
    candidate.transforming.kind() != "splat_argument"
}

/// Whether `name` is read as a local variable anywhere strictly inside `node`.
fn references(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    node: Node<'_>,
    name: &str,
) -> bool {
    super::nodes::children(node).into_iter().any(|child| {
        send_node::any_descendant(child, &mut |inner| {
            inner.kind() == "identifier"
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
    let (block, selector) = call_with_block(context, node, &["each_with_object"])?;
    empty_hash_argument(context, node)?;
    hash_receiver(context, node.child_by_field_name("receiver")?)?;
    let _ = selector;
    let parameters = super::nodes::children(node_parameters(block)?);
    let [destructured, memo] = parameters.as_slice() else {
        return None;
    };
    if destructured.kind() != "destructured_parameter" || memo.kind() != "identifier" {
        return None;
    }
    let pair = super::nodes::children(*destructured);
    let [key, value] = pair.as_slice() else {
        return None;
    };
    if key.kind() != "identifier" || value.kind() != "identifier" {
        return None;
    }
    let body = single_statement(block)?;
    if body.kind() != "assignment" {
        return None;
    }
    let target = body.child_by_field_name("left")?;
    if target.kind() != "element_reference"
        || context
            .source
            .node_text(target.child_by_field_name("object")?)
            != context.source.node_text(*memo)
    {
        return None;
    }
    let indices = super::nodes::children(target);
    let [_, index] = indices.as_slice() else {
        return None;
    };
    let assigned = body.child_by_field_name("right")?;
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
        node,
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
    if node.kind() != "element_reference" {
        return None;
    }
    let object = node.child_by_field_name("object")?;
    if !is_named_constant(context, object, "Hash") {
        return None;
    }
    let indices = super::nodes::children(node);
    let [_, mapping] = indices.as_slice() else {
        return None;
    };
    let (block, argname, transforming, unchanged) = mapping_block(context, *mapping, half)?;
    Some(Candidate {
        node,
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
    if node.kind() != "call"
        || node.child_by_field_name("arguments").is_some()
        || node.child_by_field_name("block").is_some()
    {
        return None;
    }
    if context
        .source
        .node_text(node.child_by_field_name("method")?)
        != "to_h"
    {
        return None;
    }
    let mapping = node.child_by_field_name("receiver")?;
    let (block, argname, transforming, unchanged) = mapping_block(context, mapping, half)?;
    Some(Candidate {
        node,
        call: mapping,
        block,
        leading: 0,
        trailing: node.end_byte() - mapping.end_byte(),
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
    let (block, _) = call_with_block(context, node, &["to_h"])?;
    if node.child_by_field_name("arguments").is_some() {
        return None;
    }
    hash_receiver(context, node.child_by_field_name("receiver")?)?;
    let (argname, transforming, unchanged) = pair_block(context, block, half)?;
    Some(Candidate {
        node,
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
    let (block, _) = call_with_block(context, node, &["map", "collect"])?;
    if node.child_by_field_name("arguments").is_some() {
        return None;
    }
    hash_receiver(context, node.child_by_field_name("receiver")?)?;
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
    if key.kind() != "identifier" || value.kind() != "identifier" {
        return None;
    }
    let body = single_statement(block)?;
    if body.kind() != "array" {
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
) -> Option<(Node<'tree>, Node<'tree>)> {
    if node.kind() != "call" {
        return None;
    }
    let block = node.child_by_field_name("block")?;
    let selector = node.child_by_field_name("method")?;
    methods
        .contains(&context.source.node_text(selector))
        .then_some((block, selector))
}

fn node_parameters<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    block.child_by_field_name("parameters")
}

/// The one expression a block body holds, which is all these patterns allow.
fn single_statement<'tree>(block: Node<'tree>) -> Option<Node<'tree>> {
    let body = block.child_by_field_name("body")?;
    match super::nodes::children(body).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `(hash)`: the single empty-hash argument `each_with_object` is given.
fn empty_hash_argument<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<()> {
    let arguments = super::nodes::children(node.child_by_field_name("arguments")?);
    let [only] = arguments.as_slice() else {
        return None;
    };
    let _ = context;
    (only.kind() == "hash" && super::nodes::children(*only).is_empty()).then_some(())
}

/// `#hash_receiver?`.
fn hash_receiver<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Option<()> {
    if node.kind() == "hash" {
        return Some(());
    }
    if node.kind() != "call" || !send_node::is_plain_send(node, context) {
        return None;
    }
    let selector = context
        .source
        .node_text(node.child_by_field_name("method")?);
    match node.child_by_field_name("block") {
        None => HASH_METHODS.contains(&selector).then_some(()),
        // A call that takes a block builds its hash there, and takes no arguments of its own --
        // except `each_with_object`, which takes the hash it fills in.
        Some(_) if selector == "each_with_object" => empty_hash_argument(context, node),
        Some(_) => (HASH_BLOCK_METHODS.contains(&selector)
            && node.child_by_field_name("arguments").is_none())
        .then_some(()),
    }
}

/// `(const _ :Hash)`: the constant itself, whatever scope it was reached through.
fn is_named_constant(context: &RuleContext<'_>, node: Node<'_>, name: &str) -> bool {
    match node.kind() {
        "constant" => context.source.node_text(node) == name,
        "scope_resolution" => node
            .child_by_field_name("name")
            .is_some_and(|inner| context.source.node_text(inner) == name),
        _ => false,
    }
}

/// Whether `name` is written anywhere in the subtree, which is what `` !`_memo `` rules out.
fn mentions(context: &RuleContext<'_>, node: Node<'_>, name: &str) -> bool {
    send_node::any_descendant(node, &mut |inner| {
        inner.kind() == "identifier" && context.source.node_text(inner) == name
    })
}

/// `execute_correction`: the wrapper goes, the selector is renamed, and the block keeps one
/// parameter and one expression.
fn corrections(context: &RuleContext<'_>, candidate: &Candidate<'_>, name: &str) -> Vec<Edit> {
    let range = candidate.node.byte_range();
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
    if let Some(selector) = candidate.call.child_by_field_name("method") {
        let end = candidate
            .call
            .child_by_field_name("arguments")
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
    if let Some(body) = candidate.block.child_by_field_name("body") {
        let source = context.source.node_text(candidate.transforming);
        let replacement = match candidate.transforming.kind() == "hash" && !source.starts_with('{')
        {
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
