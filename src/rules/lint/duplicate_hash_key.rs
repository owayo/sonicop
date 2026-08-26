use std::collections::HashSet;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::literals::{is_constant, recursive_basic_literal};
use super::node_equality::equality_key;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Duplicated key in hash literal.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // **A trailing `a: 1, b: 2` is one `hash` upstream.** The grammar leaves the pairs directly in
    // the argument list, so a call's keyword arguments are no hash node here -- and every duplicate
    // written that way went unreported.
    for hash in context.nodes_of_any(&["hash", "argument_list"]) {
        let mut cursor = hash.walk();
        // `HashNode#keys`: a `**splat` entry is no pair and contributes no key.
        let keys: Vec<_> = hash
            .named_children(&mut cursor)
            .filter(|child| child.kind_str() == "pair")
            .filter_map(|pair| pair.field("key"))
            .filter(|key| recursive_basic_literal(*key, context) || is_constant(*key, context))
            .collect();
        // `consecutive_duplicates` keeps every key but the first of its group, which is every key an
        // earlier one is equal to. **Asking that pairwise is quadratic**: upstream's `Duplication`
        // mixin groups the collection instead, which is linear because its nodes are hashable. One
        // generated table in `ruby/ruby` holds 7,859 keys in a single literal, where the pairwise
        // form spent longer than every other cop of the run together.
        let mut seen: HashSet<Vec<u8>> = HashSet::with_capacity(keys.len());
        for key in keys {
            if !seen.insert(equality_key(key, context)) {
                offenses.push(context.offense(MSG, key.byte_range()));
            }
        }
    }
}
