use regex::Regex;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let keywords: Vec<String> = context.setting("Keywords").unwrap_or_default();
    let requires_colon = context.setting("RequireColon").unwrap_or(true);
    let Some(pattern) = keyword_pattern(&keywords) else {
        return;
    };
    let mut previous_line = None;
    for range in context.comment_ranges() {
        let line = context.source.line_column(range.start).0;
        let first_of_block = previous_line.is_none_or(|previous| previous < line - 1);
        previous_line = Some(line);
        // A keyword further down a comment block is prose rather than an annotation, unless the
        // comment trails code on its own line.
        if !first_of_block && !trails_code(context, line) {
            continue;
        }
        let text = context.source.slice(range.clone());
        let Some(annotation) = Annotation::read(&pattern, text) else {
            continue;
        };
        if !annotation.is_annotation() || annotation.is_correct(requires_colon) {
            continue;
        }
        let start = range.start + annotation.margin.len();
        let end =
            start + annotation.keyword.len() + annotation.colon.len() + annotation.space.len();
        let keyword = annotation.keyword.to_uppercase();
        let message = match annotation.note.is_empty() {
            true => format!("Annotation comment, with keyword `{keyword}`, is missing a note.",),
            false if requires_colon => format!(
                "Annotation keywords like `{keyword}` should be all upper case, followed by a \
                 colon, and a space, then a note describing the problem."
            ),
            false => format!(
                "Annotation keywords like `{keyword}` should be all upper case, followed by a \
                 space, then a note describing the problem."
            ),
        };
        let offense = context.offense(message, start..end);
        offenses.push(match annotation.note.is_empty() {
            true => offense,
            false => offense.corrected_by(Edit {
                start,
                end,
                replacement: match requires_colon {
                    true => format!("{keyword}: "),
                    false => format!("{keyword} "),
                },
                safe: true,
            }),
        });
    }
}

/// `inline_comment?`: whether the line holds code before the comment, which makes the comment its
/// own annotation rather than a continuation of the block above it.
fn trails_code(context: &RuleContext<'_>, line: usize) -> bool {
    !context.source.line(line).trim_start().starts_with('#')
}

/// The message keyword is upper-cased for display, so the offense message names `TODO` however the
/// comment spelled it.
struct Annotation<'a> {
    margin: &'a str,
    keyword: &'a str,
    colon: &'a str,
    space: &'a str,
    note: &'a str,
}

impl<'a> Annotation<'a> {
    fn read(pattern: &Regex, text: &'a str) -> Option<Self> {
        let captures = pattern.captures(text)?;
        let group = |index: usize| captures.get(index).map_or("", |found| found.as_str());
        Some(Self {
            margin: group(1),
            keyword: group(2),
            colon: group(3),
            space: group(4),
            note: group(5),
        })
    }

    /// `annotation?`: a keyword followed by a colon or a space, and not merely the first word of a
    /// sentence -- `# Review the docs` is prose, `# Review: the docs` is an annotation.
    fn is_annotation(&self) -> bool {
        if self.keyword.is_empty() || (self.colon.is_empty() && self.space.is_empty()) {
            return false;
        }
        let capitalized = self.colon.is_empty() && !self.space.is_empty() && !self.note.is_empty();
        !(capitalized && self.keyword == capitalize(self.keyword))
    }

    fn is_correct(&self, requires_colon: bool) -> bool {
        if self.keyword.is_empty() || self.space.is_empty() || self.note.is_empty() {
            return false;
        }
        if self.keyword != self.keyword.to_uppercase() {
            return false;
        }
        self.colon.is_empty() == !requires_colon
    }
}

fn capitalize(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first
            .to_uppercase()
            .chain(characters.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// `/^(# ?)(\b#{keywords}\b)(\s*:)?(\s+)?(\S+)?/i`, with the keywords longest first so that a
/// keyword that is also the start of a phrase does not win over the phrase.
fn keyword_pattern(keywords: &[String]) -> Option<Regex> {
    if keywords.is_empty() {
        return None;
    }
    let mut sorted: Vec<&String> = keywords.iter().collect();
    sorted.sort_by_key(|keyword| std::cmp::Reverse(keyword.len()));
    let alternatives: Vec<String> = sorted
        .iter()
        .map(|keyword| regex::escape(keyword))
        .collect();
    Regex::new(&format!(
        r"(?i)^(# ?)(\b(?:{})\b)([[:space:]]*:)?([[:space:]]+)?([^[:space:]]+)?",
        alternatives.join("|")
    ))
    .ok()
}
