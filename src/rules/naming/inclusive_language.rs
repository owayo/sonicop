use std::ops::Range;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::expand_path;

use super::support::ruby_regex;

const MSG: &str = "Consider replacing '%<term>s'%<suffix>s.";
const MSG_FOR_FILE_PATH: &str = "Consider replacing '%<term>s' in file path%<suffix>s.";

/// The leaves that stand for one of the lexer token types `preprocess_check_config` maps.
const TOKEN_KINDS: &[&str] = &[
    "identifier",
    "constant",
    "instance_variable",
    "class_variable",
    "global_variable",
    "simple_symbol",
    "string_content",
    "heredoc_content",
    "comment",
];

/// One entry of `FlaggedTerms`, in the order the configuration listed it.
struct Term {
    /// How the term is looked for: `extract_regexp` reads a `Regex`, builds a boundary-guarded
    /// pattern for a `WholeWord`, and otherwise takes the term itself as a pattern.
    matcher: Matcher,
    /// `SuggestionString`: the ` with 'a' or 'b'` a message ends in, empty when none were given.
    suggestion: String,
    /// The one suggestion a correction may write, when the term names exactly one.
    sole_suggestion: Option<String>,
}

enum Matcher {
    /// A pattern, compiled twice: once to search with, once anchored to test a single position.
    Pattern {
        search: &'static Regex,
        anchored: &'static Regex,
    },
    /// `(?:\b|(?<=[\W_]))term(?:\b|(?=[\W_]))`, which this engine has no lookaround for. The
    /// boundaries are checked directly instead: what the pattern asks either side is that the
    /// neighbouring character is not a letter or a digit.
    WholeWord(String),
}

/// One flagged word, as `WordLocation` carries it. Only the text is read: upstream recomputes the
/// position from the token's own source, which is what makes a masked earlier occurrence take the
/// range of the one that was flagged.
struct Word {
    text: String,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let terms = flagged_terms(context);
    if terms.is_empty() {
        return;
    }
    let allowed = allowed_regex(context);
    if context.setting("CheckFilepaths").unwrap_or(true) {
        investigate_file_path(&terms, allowed, context, offenses);
    }
    let mut reported: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(TOKEN_KINDS) {
        if !is_checked(node, context) {
            continue;
        }
        let source = context.source.node_text(node);
        // A `tSYMBOL`'s text is the name without its colon, while the range an offense is measured
        // from is the whole literal.
        let text = match node.kind_str() {
            "simple_symbol" => &source[1..],
            _ => source,
        };
        for word in scan_for_words(text, &terms, allowed) {
            let Some(offset) = source.find(word.text.as_str()) else {
                continue;
            };
            let range = node.start_byte() + offset..node.start_byte() + offset + word.text.len();
            // `add_offense` drops a second offense on a range it has already reported, which is what
            // makes a word written twice in one token report once.
            if reported.contains(&range) {
                continue;
            }
            reported.push(range.clone());
            let term = find_term(&word.text, &terms);
            let mut offense = context.offense(message(&word.text, term, MSG), range.clone());
            if let Some(preferred) = term.and_then(|term| term.sole_suggestion.as_ref()) {
                offense = offense.corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: preferred.clone(),
                    safe: true,
                });
            }
            offenses.push(offense);
        }
    }
}

/// `investigate_filepath`: the path itself is scanned, and one `add_global_offense` names every term
/// it holds.
fn investigate_file_path(
    terms: &[Term],
    allowed: Option<&'static Regex>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let path = expand_path(context.source.path());
    let path = path.to_string_lossy();
    let words = scan_for_words(&path, terms, allowed);
    let message = match words.as_slice() {
        [] => return,
        [only] => message(&only.text, find_term(&only.text, terms), MSG_FOR_FILE_PATH),
        several => {
            let names: Vec<&str> = several.iter().map(|word| word.text.as_str()).collect();
            MSG_FOR_FILE_PATH
                .replace("%<term>s", &names.join("', '"))
                .replace("%<suffix>s", " with other terms")
        }
    };
    // `add_global_offense`, which upstream anchors at the head of the file.
    offenses.push(context.offense(message, 0..0));
}

/// `check_token?`: whether the configuration asks for the lexer token this leaf stands for.
fn is_checked(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let setting = |key: &str| context.setting(key).unwrap_or(false);
    match node.kind_str() {
        "identifier" => is_identifier_token(node, context) && setting("CheckIdentifiers"),
        "constant" => setting("CheckConstants"),
        "instance_variable" | "class_variable" | "global_variable" => setting("CheckVariables"),
        "simple_symbol" => setting("CheckSymbols"),
        "string_content" | "heredoc_content" => setting("CheckStrings"),
        "comment" => setting("CheckComments"),
        _ => false,
    }
}

/// Whether the name reaches the lexer as `tIDENTIFIER` rather than as one of the types no
/// configuration key covers.
///
/// A name in a label position is a `tLABEL`, and a name ending in `?` or `!` is a `tFID` -- except
/// where a plain `def` puts the lexer in `expr_fname`, which `def self.name?` does not.
fn is_identifier_token(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let parent = node.parent_of(context);
    if parent.is_some_and(|parent| {
        parent.kind_str() == "keyword_parameter" && parent.field("name") == Some(node)
    }) {
        return false;
    }
    let text = context.source.node_text(node);
    if !text.ends_with('?') && !text.ends_with('!') {
        return true;
    }
    parent.is_some_and(|parent| parent.kind_str() == "method" && parent.field("name") == Some(node))
}

/// `scan_for_words`: every flagged word in `input`, after the allowed spans have been masked out.
///
/// Upstream scans with one pattern built by joining every term's with `|`, so at each position the
/// first term that matches wins and the scan carries on past the match. That is what the walk here
/// does, one position at a time, because a `WholeWord` term cannot be joined into a single pattern.
fn scan_for_words(input: &str, terms: &[Term], allowed: Option<&'static Regex>) -> Vec<Word> {
    let masked = match allowed {
        // `'*' * match.size`, which leaves every later position where it was.
        Some(allowed) => allowed
            .replace_all(input, |captured: &regex::Captures<'_>| {
                "*".repeat(captured.get(0).map_or(0, |matched| matched.len()))
            })
            .into_owned(),
        None => input.to_owned(),
    };
    let mut words = Vec::new();
    let mut position = 0;
    while position < masked.len() {
        if !masked.is_char_boundary(position) {
            position += 1;
            continue;
        }
        let matched = terms
            .iter()
            .find_map(|term| match_at(term, &masked, position));
        match matched {
            // A pattern that matched nothing would never advance, which upstream's `scan` answers by
            // stepping one character on.
            Some(0) | None => position += next_boundary(&masked, position),
            Some(length) => {
                words.push(Word {
                    text: masked[position..position + length].to_owned(),
                });
                position += length;
            }
        }
    }
    words
}

/// How far the next character starts.
fn next_boundary(text: &str, position: usize) -> usize {
    text[position..]
        .chars()
        .next()
        .map_or(1, |character| character.len_utf8())
}

/// The length of the term's match at `position`, or `None` when it does not match there.
fn match_at(term: &Term, text: &str, position: usize) -> Option<usize> {
    match &term.matcher {
        Matcher::Pattern { anchored, .. } => anchored.find(&text[position..]).map(|m| m.end()),
        Matcher::WholeWord(word) => {
            let end = position + word.len();
            if end > text.len() || !text.is_char_boundary(end) {
                return None;
            }
            if !text[position..end].eq_ignore_ascii_case(word) {
                return None;
            }
            let first = word.chars().next()?;
            let last = word.chars().next_back()?;
            (boundary_before(text, position, first) && boundary_after(text, end, last))
                .then_some(word.len())
        }
    }
}

/// `(?:\b|(?<=[\W_]))` read at `position`, where `first` is the first character of the term.
fn boundary_before(text: &str, position: usize, first: char) -> bool {
    let Some(previous) = text[..position].chars().next_back() else {
        return is_word(first);
    };
    is_word(first) != is_word(previous) || !is_word(previous) || previous == '_'
}

/// `(?:\b|(?=[\W_]))` read at `position`, where `last` is the last character of the term.
fn boundary_after(text: &str, position: usize, last: char) -> bool {
    let Some(next) = text[position..].chars().next() else {
        return is_word(last);
    };
    is_word(last) != is_word(next) || !is_word(next) || next == '_'
}

/// Ruby's `\w`, which stays ASCII whatever the input holds.
fn is_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// `find_flagged_term`: the first term whose own pattern matches the word that was found.
fn find_term<'a>(word: &str, terms: &'a [Term]) -> Option<&'a Term> {
    terms.iter().find(|term| match &term.matcher {
        Matcher::Pattern { search, .. } => search.is_match(word),
        Matcher::WholeWord(_) => (0..word.len()).any(|position| {
            word.is_char_boundary(position) && match_at(term, word, position).is_some()
        }),
    })
}

/// `create_message`.
fn message(word: &str, term: Option<&Term>, template: &str) -> String {
    let suffix = match term.map(|term| term.suggestion.as_str()) {
        Some(suggestion) if !suggestion.is_empty() => suggestion,
        _ => " with another term",
    };
    template
        .replace("%<term>s", word)
        .replace("%<suffix>s", suffix)
}

/// `preprocess_flagged_terms`: the configured terms, in the order they were written.
fn flagged_terms(context: &RuleContext<'_>) -> Vec<Term> {
    let Some(serde_yaml_ng::Value::Mapping(configured)) = context.setting("FlaggedTerms") else {
        return Vec::new();
    };
    let mut terms = Vec::new();
    for (name, definition) in &configured {
        let Some(name) = name.as_str() else {
            continue;
        };
        let serde_yaml_ng::Value::Mapping(definition) = definition else {
            continue;
        };
        let read = |key: &str| definition.get(serde_yaml_ng::Value::String(key.to_owned()));
        let matcher = match read("Regex") {
            Some(pattern) => match compile(pattern) {
                Some(matcher) => matcher,
                None => continue,
            },
            None => match read("WholeWord").and_then(serde_yaml_ng::Value::as_bool) {
                Some(true) => Matcher::WholeWord(name.to_owned()),
                _ => match compile(&serde_yaml_ng::Value::String(name.to_owned())) {
                    Some(matcher) => matcher,
                    None => continue,
                },
            },
        };
        let suggestions = suggestion_list(read("Suggestions"));
        terms.push(Term {
            matcher,
            suggestion: suggestion_string(&suggestions),
            sole_suggestion: match suggestions.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            },
        });
    }
    terms
}

/// A pattern from the configuration, compiled both to search with and anchored to a position.
fn compile(value: &serde_yaml_ng::Value) -> Option<Matcher> {
    let search = ruby_regex(&ignorecase(value))?;
    let anchored = ruby_regex(&anchored_ignorecase(value))?;
    Some(Matcher::Pattern { search, anchored })
}

/// The pattern with `Regexp::IGNORECASE` added, which is how upstream compiles every term.
fn ignorecase(value: &serde_yaml_ng::Value) -> serde_yaml_ng::Value {
    tagged_regexp(&format!("/{}/i", regex_source(value)))
}

fn anchored_ignorecase(value: &serde_yaml_ng::Value) -> serde_yaml_ng::Value {
    tagged_regexp(&format!(r"/\A(?:{})/i", regex_source(value)))
}

fn tagged_regexp(literal: &str) -> serde_yaml_ng::Value {
    serde_yaml_ng::Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
        tag: serde_yaml_ng::value::Tag::new("!ruby/regexp"),
        value: serde_yaml_ng::Value::String(literal.to_owned()),
    }))
}

/// `ensure_regex_string`: a `Regexp` gives up its source, a string is one already.
fn regex_source(value: &serde_yaml_ng::Value) -> String {
    match value {
        serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "!ruby/regexp" => {
            let literal = tagged.value.as_str().unwrap_or_default();
            match literal
                .strip_prefix('/')
                .and_then(|rest| rest.rsplit_once('/'))
            {
                Some((body, _flags)) => body.to_owned(),
                None => literal.to_owned(),
            }
        }
        other => other.as_str().unwrap_or_default().to_owned(),
    }
}

/// `Array(suggestions)` with the blank forms upstream reads as "none" dropped.
fn suggestion_list(value: Option<&serde_yaml_ng::Value>) -> Vec<String> {
    match value {
        Some(serde_yaml_ng::Value::Sequence(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Some(serde_yaml_ng::Value::String(only)) if !only.trim().is_empty() => {
            vec![only.clone()]
        }
        _ => Vec::new(),
    }
}

/// `format_suggestions`.
fn suggestion_string(suggestions: &[String]) -> String {
    let quoted: Vec<String> = suggestions
        .iter()
        .map(|suggestion| format!("'{suggestion}'"))
        .collect();
    let joined = match quoted.as_slice() {
        [] => return String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} or {two}"),
        many => {
            let (last, rest) = many.split_last().expect("more than two entries");
            format!("{}, or {last}", rest.join(", "))
        }
    };
    format!(" with {joined}")
}

/// The `AllowedRegex` of every term, joined into the one pattern `mask_input` blanks out.
fn allowed_regex(context: &RuleContext<'_>) -> Option<&'static Regex> {
    let serde_yaml_ng::Value::Mapping(configured) = context.setting("FlaggedTerms")? else {
        return None;
    };
    let mut sources = Vec::new();
    for (_name, definition) in &configured {
        let serde_yaml_ng::Value::Mapping(definition) = definition else {
            continue;
        };
        let allowed = definition.get(serde_yaml_ng::Value::String("AllowedRegex".to_owned()));
        let entries = match allowed {
            Some(serde_yaml_ng::Value::Sequence(items)) => items.clone(),
            Some(other) => vec![other.clone()],
            None => Vec::new(),
        };
        for entry in entries {
            let source = regex_source(&entry);
            if source.trim().is_empty() {
                continue;
            }
            sources.push(source);
        }
    }
    if sources.is_empty() {
        return None;
    }
    ruby_regex(&tagged_regexp(&format!("/{}/i", sources.join("|"))))
}
