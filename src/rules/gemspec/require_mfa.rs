use std::collections::HashSet;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::push_named_children;
use crate::rules::send_node::{
    has_interpolation, is_plain_send, is_string, named_children, pair_key_symbol, string_text,
    symbol_name,
};

use super::support::{local_variables, specification_variable};

const KEY: &str = "rubygems_mfa_required";
const MSG: &str = "`metadata['rubygems_mfa_required']` must be set to `'true'`.";
const DIRECTIVE: &str = "'rubygems_mfa_required' => 'true'";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = local_variables(context);
    // `on_block` runs on every block, and `gem_specification` then *searches* that block for a
    // specification. A specification written inside another block is therefore read twice: once
    // against the block it was found in, and once against itself.
    for block in context.nodes_of("call") {
        if block.field("block").is_none() {
            continue;
        }
        for variable in specifications_within(block, context) {
            report(block, variable, &locals, context, offenses);
        }
    }
}

/// The parameter of every `Gem::Specification.new do |spec|` in `block`'s subtree, `block` itself
/// included, in the order upstream's search reaches them.
fn specifications_within<'a>(block: Node<'_>, context: &'a RuleContext<'_>) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut stack = vec![block];
    while let Some(node) = stack.pop() {
        if let Some(variable) = specification_variable(node, context) {
            found.push(variable);
        }
        push_named_children(node, &mut stack);
    }
    found
}

fn report(
    block: Node<'_>,
    variable: &str,
    locals: &HashSet<&str>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let metadata = metadata_value(block, locals, context);
    match mfa_value(metadata, context) {
        // The setting is there: it only has to say `'true'`.
        Some(value) => {
            if is_string(value, context) && string_text(value, context) == "true" {
                return;
            }
            offenses.push(context.offense(MSG, value.byte_range()).corrected_by(Edit {
                start: value.start_byte(),
                end: value.end_byte(),
                replacement: "'true'".to_owned(),
                safe: true,
            }));
        }
        // Nothing requires MFA, so the whole specification is reported and the setting written in.
        None => {
            let mut offense = context.offense(MSG, block.byte_range());
            if let Some((anchor, edit)) = insertion(block, metadata, variable, locals, context) {
                offense = offense.corrections_anchored_at(anchor).corrected_by(edit);
            }
            offenses.push(offense);
        }
    }
}

/// The value assigned to the metadata, as `metadata` captures it: the hash of a `spec.metadata =`,
/// or the value of a `spec.metadata['rubygems_mfa_required'] =`. The first match in a pre-order walk
/// of the block wins.
fn metadata_value<'tree>(
    block: Node<'tree>,
    locals: &HashSet<&str>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut stack = vec![block];
    while let Some(node) = stack.pop() {
        if node.kind_str() == "assignment"
            && let Some(left) = node.field("left")
        {
            // `(send _ :metadata= $_)`
            if is_metadata_call(left, context) {
                return node.field("right");
            }
            // `(send (send _ :metadata) :[]= {(str "rubygems_mfa_required") (sym :...)} $_)`
            if let Some(index) = metadata_index(left, locals, context)
                && is_mfa_name(index, context)
            {
                return node.field("right");
            }
        }
        push_named_children(node, &mut stack);
    }
    None
}

/// `mfa_value`: the metadata value itself when it is a string, and otherwise the value the metadata
/// hash gives the `rubygems_mfa_required` key.
fn mfa_value<'tree>(
    metadata: Option<Node<'tree>>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let metadata = metadata?;
    if is_string(metadata, context) {
        return Some(metadata);
    }
    let mut stack = vec![metadata];
    while let Some(node) = stack.pop() {
        if node.kind_str() == "pair" && is_mfa_pair(node, context) {
            return node.field("value");
        }
        push_named_children(node, &mut stack);
    }
    None
}

/// The edit that writes the missing setting, and the range upstream hands its corrector.
fn insertion(
    block: Node<'_>,
    metadata: Option<Node<'_>>,
    variable: &str,
    locals: &HashSet<&str>,
    context: &RuleContext<'_>,
) -> Option<(std::ops::Range<usize>, Edit)> {
    match metadata {
        // `correct_metadata`: a new pair goes after the last one, or between the braces of an empty
        // hash. Metadata assigned anything other than a hash literal cannot be corrected at all.
        Some(hash) if hash.kind_str() == "hash" => {
            let pairs: Vec<Node<'_>> = named_children(hash)
                .into_iter()
                .filter(|child| child.kind_str() == "pair")
                .collect();
            match pairs.last() {
                Some(last) => Some((
                    last.byte_range(),
                    insert(last.end_byte(), format!(",\n{DIRECTIVE}")),
                )),
                None => {
                    let brace = closing_delimiter(hash)?;
                    Some((
                        brace.byte_range(),
                        insert(brace.start_byte(), DIRECTIVE.to_owned()),
                    ))
                }
            }
        }
        Some(_) => None,
        // `insert_mfa_required`: after the last metadata assignment the block already makes, or
        // before the `end` that closes it.
        None => {
            let directive = format!("{variable}.metadata['{KEY}'] = 'true'");
            match last_metadata_assignment(block, locals, context) {
                Some(assignment) => Some((
                    assignment.byte_range(),
                    insert(assignment.end_byte(), format!("\n{directive}")),
                )),
                None => {
                    let end = closing_delimiter(block.field("block")?)?;
                    Some((
                        end.byte_range(),
                        insert(end.start_byte(), format!("{directive}\n")),
                    ))
                }
            }
        }
    }
}

/// The last `spec.metadata =` or `spec.metadata[key] =` in the block, whatever key it names.
fn last_metadata_assignment<'tree>(
    block: Node<'tree>,
    locals: &HashSet<&str>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut last = None;
    let mut stack = vec![block];
    while let Some(node) = stack.pop() {
        if node.kind_str() == "assignment"
            && let Some(left) = node.field("left")
            && (is_metadata_call(left, context)
                || metadata_index(left, locals, context)
                    .is_some_and(|index| is_name_literal(index, context)))
        {
            last = Some(node);
        }
        push_named_children(node, &mut stack);
    }
    last
}

/// `(send _ :metadata=)` read off the target of an assignment. Upstream's `_` matches a missing
/// receiver too, but a receiverless `metadata = x` is a local variable assignment rather than a
/// call, so only a target written as a call qualifies.
fn is_metadata_call(target: Node<'_>, context: &RuleContext<'_>) -> bool {
    target.kind_str() == "call"
        && is_plain_send(target, context)
        && target
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "metadata")
}

/// The single index of a `metadata[...] =`, when the target is one. `(send _ :metadata)` takes no
/// arguments of its own, and a receiverless `metadata` is a call only while nothing bound it as a
/// local variable.
fn metadata_index<'tree>(
    target: Node<'tree>,
    locals: &HashSet<&str>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if target.kind_str() != "element_reference" {
        return None;
    }
    let object = target.field("object")?;
    let named = match object.kind_str() {
        "call" => {
            is_plain_send(object, context)
                && object.field("arguments").is_none()
                && object
                    .field("method")
                    .is_some_and(|method| context.source.node_text(method) == "metadata")
        }
        "identifier" => {
            let name = context.source.node_text(object);
            name == "metadata" && !locals.contains(name)
        }
        _ => false,
    };
    if !named {
        return None;
    }
    let indices = named_children(target);
    match indices.get(1..)? {
        [index] => Some(*index),
        _ => None,
    }
}

/// Whether an index names `rubygems_mfa_required`, as either a string or a symbol.
fn is_mfa_name(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if let Some(name) = symbol_name(node, context) {
        return name == KEY;
    }
    is_plain_string(node) && string_text(node, context) == KEY
}

/// `{str sym}`: any string or symbol, which is all `metadata_assignment` asks of an index.
fn is_name_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    symbol_name(node, context).is_some() || is_plain_string(node)
}

/// `(pair {(str "rubygems_mfa_required") (sym :rubygems_mfa_required)} $_)`.
fn is_mfa_pair(pair: Node<'_>, context: &RuleContext<'_>) -> bool {
    if pair_key_symbol(pair, context) == Some(KEY) {
        return true;
    }
    pair.field("key")
        .is_some_and(|key| is_plain_string(key) && string_text(key, context) == KEY)
}

/// `(str ...)`: a string literal with nothing interpolated into it, which is the only shape a
/// `str` in a node pattern matches.
fn is_plain_string(node: Node<'_>) -> bool {
    node.kind_str() == "string" && !has_interpolation(node)
}

/// `node.loc.end`: the `}` or `end` a hash or a block closes with.
fn closing_delimiter<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let last = node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)?;
    matches!(last.kind_str(), "}" | "end").then_some(last)
}

fn insert(offset: usize, replacement: String) -> Edit {
    Edit {
        start: offset,
        end: offset,
        replacement,
        safe: true,
    }
}
