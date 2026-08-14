//! `Style/ModuleMemberExistenceCheck`: asking a list of members for one name has a predicate.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, arguments, send_range};

/// `METHOD_REPLACEMENTS`.
const METHOD_REPLACEMENTS: &[(&str, &str)] = &[
    ("class_variables", "class_variable_defined?"),
    ("instance_methods", "method_defined?"),
    ("private_instance_methods", "private_method_defined?"),
    ("protected_instance_methods", "protected_method_defined?"),
    ("public_instance_methods", "public_method_defined?"),
];

/// `METHODS_WITHOUT_INHERIT_PARAM`: the one listing that takes no `inherit` argument, so no
/// argument of it can be carried into the predicate.
const WITHOUT_INHERIT_PARAM: &[&str] = &["class_variables"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    // The cop is entered on the listing call, but everything it decides hangs off the `include?`
    // above it, so the walk starts there instead.
    for outer in context.nodes_of("call") {
        let (Some(selector), Some(receiver)) = (outer.field("method"), outer.field("receiver"))
        else {
            continue;
        };
        if !matches!(context.source.node_text(selector), "include?" | "member?") {
            continue;
        }
        let wanted = arguments(outer);
        let [name] = wanted.as_slice() else {
            continue;
        };
        if !is_simple(&wanted) {
            continue;
        }
        let Some(listing) = Listing::of(receiver, context, &locals) else {
            continue;
        };
        if !is_simple(&listing.inherit) {
            continue;
        }
        let name = context.source.slice(name.range());
        // The `inherit` argument only survives where it says something: a listing that takes none,
        // and an explicit `true`, both mean the predicate's own default.
        let replacement = match listing.inherit.first() {
            Some(argument) if !is_true(argument.first(), context) => {
                let inherit = context.source.slice(argument.range());
                format!("{}({name}, {inherit})", listing.replacement_method)
            }
            _ => format!("{}({name})", listing.replacement_method),
        };
        let range = listing.selector.start..send_range(outer, context).end;
        offenses.push(
            context
                .offense(format!("Use `{replacement}` instead."), range.clone())
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement,
                    safe: true,
                }),
        );
    }
}

/// The call that lists members, as the two patterns
/// `(call _ %METHODS_WITH_INHERIT_PARAM _?)` and `(call _ %METHODS_WITHOUT_INHERIT_PARAM)` read it.
struct Listing<'tree> {
    selector: Range<usize>,
    replacement_method: &'static str,
    inherit: Vec<Argument<'tree>>,
}

impl<'tree> Listing<'tree> {
    fn of(
        node: Node<'tree>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) -> Option<Self> {
        // A receiverless call of no arguments is written as a bare identifier here and as a `send`
        // upstream; a name the enclosing scope assigned is a variable read instead, and no call at
        // all.
        let (selector, inherit) = match node.kind_str() {
            "identifier" if !locals.is_lvar(node) => (node.byte_range(), Vec::new()),
            // A block makes the receiver a `block` node upstream, which the pattern does not name.
            "call" if node.field("block").is_none() => {
                (node.field("method")?.byte_range(), arguments(node))
            }
            _ => return None,
        };
        let name = context.source.slice(selector.clone());
        let (_, replacement_method) = METHOD_REPLACEMENTS
            .iter()
            .find(|(known, _)| *known == name)?;
        let limit = usize::from(!WITHOUT_INHERIT_PARAM.contains(&name));
        if inherit.len() > limit {
            return None;
        }
        Some(Self {
            selector,
            replacement_method,
            inherit,
        })
    }
}

/// `simple_method_argument?`: no splat, no block pass, and no hash in first position.
fn is_simple(list: &[Argument<'_>]) -> bool {
    if list.iter().any(|argument| {
        matches!(
            argument.first().kind_str(),
            "splat_argument" | "block_argument" | "hash_splat_argument"
        )
    }) {
        return false;
    }
    list.first().is_none_or(|first| !is_hash(first))
}

/// Whether the argument is the `hash` upstream's parser builds, braces or not.
fn is_hash(argument: &Argument<'_>) -> bool {
    argument.parts().len() > 1
        || matches!(
            argument.first().kind_str(),
            "hash" | "pair" | "hash_splat_argument"
        )
}

fn is_true(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "true" && context.source.node_text(node) == "true"
}
