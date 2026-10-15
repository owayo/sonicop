//! 解決できない修飾名だけを索引内の候補と照合する。本家の DidYouMean の順位を保つ。

use super::project_index_help as help;
use crate::diagnostic::Offense;
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

fn jaro_winkler(left: &str, right: &str) -> f64 {
    let (mut short, mut long): (Vec<_>, Vec<_>) = (left.chars().collect(), right.chars().collect());
    if short.len() > long.len() {
        std::mem::swap(&mut short, &mut long);
    }
    let radius = if long.len() > 3 {
        long.len() / 2 - 1
    } else {
        0
    };
    let mut short_matches = vec![false; short.len()];
    let mut long_matches = vec![false; long.len()];
    let mut count = 0;
    for (position, character) in short.iter().enumerate() {
        for candidate in position.saturating_sub(radius)..(position + radius + 1).min(long.len()) {
            if !long_matches[candidate] && *character == long[candidate] {
                short_matches[position] = true;
                long_matches[candidate] = true;
                count += 1;
                break;
            }
        }
    }
    if count == 0 {
        return 0.0;
    }
    let short_order = short
        .iter()
        .zip(short_matches)
        .filter_map(|(character, found)| found.then_some(character));
    let long_order = long
        .iter()
        .zip(long_matches)
        .filter_map(|(character, found)| found.then_some(character));
    let transpositions = short_order
        .zip(long_order)
        .filter(|(left, right)| left != right)
        .count()
        / 2;
    let matches = count as f64;
    let distance = (matches / short.len() as f64
        + matches / long.len() as f64
        + (matches - transpositions as f64) / matches)
        / 3.0;
    if distance > 0.7 {
        let prefix = left
            .chars()
            .zip(right.chars())
            .take(4)
            .take_while(|(left, right)| left == right)
            .count();
        distance + prefix as f64 * 0.1 * (1.0 - distance)
    } else {
        distance
    }
}

fn levenshtein(left: &str, right: &str) -> usize {
    let right: Vec<_> = right.chars().collect();
    let mut costs: Vec<_> = (0..=right.len()).collect();
    for (row, left_character) in left.chars().enumerate() {
        let mut diagonal = costs[0];
        costs[0] = row + 1;
        for (column, right_character) in right.iter().enumerate() {
            let previous = costs[column + 1];
            costs[column + 1] = (costs[column] + 1)
                .min(previous + 1)
                .min(diagonal + usize::from(left_character != *right_character));
            diagonal = previous;
        }
    }
    costs[right.len()]
}
