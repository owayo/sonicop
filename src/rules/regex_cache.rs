//! Patterns a cop builds from its configuration, compiled once for the whole run.
//!
//! A cop whose pattern comes from `.rubocop.yml` cannot put it in a `LazyLock`, so several of them
//! called `Regex::new` on the way into every file. The configuration does not change between
//! files, so the same handful of patterns were being compiled once per file -- `Style/WordArray`'s
//! `WordRegex` alone accounted for 4.5% of a run over RuboCop's own tree.
//!
//! The set of distinct patterns a run sees is bounded by the configuration, so a compiled pattern
//! is kept for the life of the process rather than evicted. That is what lets the answer be a
//! `&'static Regex`: a cop can hold on to it without cloning the compiled automaton.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use regex::Regex;

/// `None` is cached too: a pattern this engine cannot compile would otherwise be retried, and
/// failing to compile is the expensive half of `Regex::new`.
type Cache = HashMap<Box<str>, Option<&'static Regex>>;

static COMPILED: LazyLock<RwLock<Cache>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// The compiled form of `pattern`, or `None` when this engine will not take it.
pub(crate) fn compiled(pattern: &str) -> Option<&'static Regex> {
    if let Some(hit) = COMPILED
        .read()
        .expect("the regex cache is never poisoned")
        .get(pattern)
    {
        return *hit;
    }
    // Compiling outside the lock: a pattern is only ever compiled to the same automaton, so two
    // threads racing on a cold entry cost one wasted compilation rather than a held write lock.
    let built: Option<&'static Regex> = Regex::new(pattern)
        .ok()
        .map(|regex| &*Box::leak(Box::new(regex)));
    COMPILED
        .write()
        .expect("the regex cache is never poisoned")
        .insert(pattern.into(), built);
    built
}

#[cfg(test)]
mod tests {
    use super::compiled;

    #[test]
    fn the_same_pattern_answers_with_the_same_automaton() {
        let first = compiled(r"^\w+$").expect("a valid pattern compiles");
        let second = compiled(r"^\w+$").expect("a valid pattern compiles");
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn an_unusable_pattern_answers_none() {
        assert!(compiled(r"(").is_none());
    }
}
