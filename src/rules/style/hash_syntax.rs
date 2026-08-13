use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The symbol names the new syntax can spell without quoting them, as
/// `acceptable_19_syntax_symbol?` matches them. A trailing `?` or `!` is fine; a trailing `=` is
/// not, because `{ foo=: 1 }` is not valid Ruby.
static PLAIN_SYMBOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)^[_a-z][A-Za-z0-9_]*[?!]?$").unwrap());

/// The quoted form only became a legal hash key in Ruby 2.2.
const QUOTED_KEY_SINCE: RubyVersion = RubyVersion::new(2, 2);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "ruby19".to_owned());
    if style != "ruby19" && style != "ruby19_no_mixed_keys" {
        return;
    }
    let quoted_keys_allowed = context.target_ruby_version() >= QUOTED_KEY_SINCE;

    for node in context.nodes_of("pair") {
        let Some(operator) = hash_rocket(node, context) else {
            continue;
        };
        // A hash is left alone unless every one of its keys can take the new syntax, so that one
        // rocket that has to stay does not leave the hash written in two styles at once.
        if !every_key_takes_the_new_syntax(node, context, quoted_keys_allowed) {
            continue;
        }

        let start = node.start_byte();
        offenses.push(
            context
                .offense("Use the new Ruby 1.9 hash syntax.", start..operator.end)
                .corrected_by(Edit {
                    start,
                    end: whitespace_end(context, operator.end),
                    replacement: format!("{}: ", key_name(node, context)),
                    safe: true,
                }),
        );
    }
}

/// The span of the pair's `=>`, or `None` when the pair is already written with a colon.
fn hash_rocket(node: Node<'_>, context: &RuleContext<'_>) -> Option<std::ops::Range<usize>> {
    let key = node.field("key")?;
    let value = node.field("value")?;
    let between = context.source.slice(key.end_byte()..value.start_byte());
    let offset = between.find("=>")?;
    Some(key.end_byte() + offset..key.end_byte() + offset + "=>".len())
}

/// The key without the leading `:`, which is what the new syntax puts in front of the colon.
fn key_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    let text = node
        .field("key")
        .map_or("", |key| context.source.node_text(key));
    text.strip_prefix(':').unwrap_or(text)
}

/// `sym_indices?`: whether every key of the hash this pair belongs to is a symbol the new syntax
/// can spell.
fn every_key_takes_the_new_syntax(
    node: Node<'_>,
    context: &RuleContext<'_>,
    quoted_keys_allowed: bool,
) -> bool {
    let Some(container) = node.parent() else {
        return false;
    };
    let mut cursor = container.walk();
    let pairs: Vec<Node<'_>> = container
        .named_children(&mut cursor)
        .filter(|child| child.kind_str() == "pair")
        .collect();
    !pairs.is_empty()
        && pairs
            .iter()
            .all(|pair| word_symbol_pair(*pair, context, quoted_keys_allowed))
}

/// `word_symbol_pair?`: a symbol key whose name the new syntax accepts.
///
/// A key written with a colon is already a symbol whatever it looks like -- `"a b": 1` reaches
/// RuboCop as a `dsym` -- so only the rocket form has to have its node kind checked.
fn word_symbol_pair(node: Node<'_>, context: &RuleContext<'_>, quoted_keys_allowed: bool) -> bool {
    let Some(key) = node.field("key") else {
        return false;
    };
    let is_symbol = hash_rocket(node, context).is_none()
        || matches!(key.kind_str(), "simple_symbol" | "delimited_symbol");
    is_symbol && acceptable_19_syntax_symbol(context.source.node_text(key), quoted_keys_allowed)
}

fn acceptable_19_syntax_symbol(text: &str, quoted_keys_allowed: bool) -> bool {
    let name = text.strip_prefix(':').unwrap_or(text);
    if PLAIN_SYMBOL.is_match(name) {
        return true;
    }
    quoted_keys_allowed
        && name.len() >= 2
        && ((name.starts_with('\'') && name.ends_with('\''))
            || (name.starts_with('"') && name.ends_with('"')))
}

/// The end of the run of blanks after `offset`, which the correction swallows so that the colon it
/// leaves behind keeps a single space in front of the value.
///
/// RuboCop's `range_with_surrounding_space` takes the spaces and tabs first and only then the line
/// breaks, so the indentation of a following line is left where it is.
fn whitespace_end(context: &RuleContext<'_>, offset: usize) -> usize {
    let text = context.source.text().as_bytes();
    let mut end = offset;
    while text
        .get(end)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        end += 1;
    }
    while text.get(end) == Some(&b'\n') {
        end += 1;
    }
    end
}
