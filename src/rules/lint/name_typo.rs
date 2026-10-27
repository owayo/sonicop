//! 解決できない修飾名だけを索引内の候補と照合する。本家の DidYouMean の順位を保つ。

use super::project_index_help as help;
use crate::diagnostic::Offense;
use crate::name_distance::{jaro_winkler, levenshtein};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use std::collections::HashSet;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(project) = context.project_index() else {
        return;
    };
    let allowed: Vec<String> = context.setting("AllowedNames").unwrap_or_default();
    let literals = help::literal_names(context, false);
    let check = |key| {
        context
            .setting::<serde_yaml_ng::Value>(key)
            .is_none_or(|value| {
                !matches!(
                    value,
                    serde_yaml_ng::Value::Null | serde_yaml_ng::Value::Bool(false)
                )
            })
    };
    project.with(|index| {
        if check("CheckConstants") {
            for node in context.nodes_of("scope_resolution") {
                if !help::constant_reference(node, context) || defined_check(node, context) { continue; }
                let (Some(scope), Some(selector)) = (node.field("scope"), node.field("name")) else { continue; };
                let Some(namespace) = help::constant_name(scope, context) else { continue; };
                let name = context.source.node_text(selector);
                if allowed.iter().any(|allowed| allowed == name) || literals.contains(name) { continue; }
                let Some(owner) = help::resolve_constant(index, scope, context) else { continue; };
                if !index.namespace(owner) || index.find_member(owner, name).is_some() { continue; }
                if let Some(suggestion) = spell_check(name, index.constant_names(owner)) {
                    offenses.push(context.offense(format!("Possible typo: `{name}` is not defined in `{namespace}`. Did you mean `{suggestion}`?"), selector.byte_range()));
                }
            }
        }
        if check("CheckMethods") {
            for node in context.nodes_of("call") {
                let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method")) else { continue; };
                let Some(receiver_name) = help::constant_name(receiver, context) else { continue; };
                if defined_check(node, context) { continue; }
                let written = context.source.node_text(selector);
                let setter = help::setter_call(node, context);
                let name = if setter { format!("{written}=") } else { written.to_owned() };
                if allowed.contains(&name) { continue; }
                let base = name.strip_suffix('=').filter(|_| setter).unwrap_or(&name);
                if literals.contains(base) { continue; }
                let Some(owner) = help::resolve_constant(index, receiver, context) else { continue; };
                if !index.namespace(owner) || [name.as_str(), base].iter().any(|candidate| {
                    let member = format!("{candidate}()");
                    index.singleton_member(owner, &member).is_some() || index.find_member(owner, &member).is_some()
                }) || !index.complete_ancestry(owner) { continue; }
                if let Some(mut suggestion) = spell_check(base, index.method_names(owner)) {
                    if setter { suggestion.push('='); }
                    offenses.push(context.offense(format!("Possible typo: `{receiver_name}` does not respond to `{name}`. Did you mean `{suggestion}`?"), selector.byte_range()));
                }
            }
        }
    });
}

fn defined_check(node: tree_sitter::Node<'_>, context: &RuleContext<'_>) -> bool {
    help::ancestors(node, context).iter().any(|ancestor| {
        ancestor.kind_str() == "unary"
            && ancestor
                .field("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "defined?")
    })
}

fn normalize(name: &str) -> String {
    name.to_lowercase().replace('@', "")
}

fn spell_check(input: &str, dictionary: Vec<String>) -> Option<String> {
    let normalized = normalize(input);
    let length = normalized.chars().count();
    let threshold = if length > 3 { 0.834 } else { 0.77 };
    let mut seen = HashSet::new();
    let mut words: Vec<_> = dictionary
        .into_iter()
        .filter(|word| seen.insert(word.clone()))
        .filter(|word| word != input && jaro_winkler(&normalize(word), &normalized) >= threshold)
        .collect();
    // SpellChecker#correct は絞り込み時だけ候補も正規化し、順位付けには元の大小文字を使う。
    words.sort_by(|left, right| {
        jaro_winkler(left, &normalized).total_cmp(&jaro_winkler(right, &normalized))
    });
    words.reverse();
    words
        .iter()
        .find(|word| levenshtein(&normalize(word), &normalized) <= length.div_ceil(4))
        .cloned()
        .or_else(|| {
            words.into_iter().find(|word| {
                levenshtein(&normalize(word), &normalized)
                    < length.min(normalize(word).chars().count())
            })
        })
}
