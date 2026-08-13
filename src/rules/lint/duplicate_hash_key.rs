use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::literals::{is_constant, recursive_basic_literal};
use super::node_equality::identical;

const MSG: &str = "Duplicated key in hash literal.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for hash in context.nodes_of("hash") {
        let mut cursor = hash.walk();
        // `HashNode#keys`: a `**splat` entry is no pair and contributes no key.
        let keys: Vec<_> = hash
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "pair")
            .filter_map(|pair| pair.child_by_field_name("key"))
            .filter(|key| recursive_basic_literal(*key, context) || is_constant(*key, context))
            .collect();
        // `consecutive_duplicates` keeps every key but the first of its group, which is every key
        // an earlier one is equal to.
        for (index, key) in keys.iter().enumerate() {
            if keys[..index]
                .iter()
                .any(|earlier| identical(*earlier, *key, context))
            {
                offenses.push(context.offense(MSG, key.byte_range()));
            }
        }
    }
}
