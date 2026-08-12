//! `DirectiveComment` and the part of `CommentConfig` the two directive cops read.
//!
//! `src/directives.rs` answers the one question the engine asks -- whether an offense is switched
//! off where it was found -- and answers it per line. These two cops ask about the directives
//! themselves: which comment turned a cop off, whether the switch was ever turned back on, and
//! whether an `enable` had anything to undo. That is a different model of the same comments, kept
//! here rather than folded into the engine's.

use std::ops::Range;

use crate::rules::RuleContext;

/// The departments a cop name can be shortened to. `cop_registry.department?` answers from the
/// registry, which holds exactly the departments the bundled configuration defines.
const DEPARTMENTS: &[&str] = &[
    "Bundler",
    "Gemspec",
    "Layout",
    "Lint",
    "Metrics",
    "Migration",
    "Naming",
    "Security",
    "Style",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Disable,
    Enable,
}

/// One `# rubocop:` comment, as `DirectiveComment` reads it.
pub(super) struct Directive {
    pub comment: Range<usize>,
    pub mode: Mode,
    /// `raw_cop_names`: the names as written, with departments left unexpanded.
    pub names: Vec<String>,
    pub all: bool,
    /// `comment_only_line?`: whether the line holds nothing but the comment.
    pub comment_only_line: bool,
    /// `DirectiveComment#range`: the marker and everything it matched.
    pub range: Range<usize>,
}

impl Directive {
    /// `in_directive_department?`: whether one of the written names is a department the cop is in.
    pub(super) fn department_of(&self, cop: &str) -> Option<&str> {
        self.names
            .iter()
            .find(|name| is_department(name) && cop.starts_with(name.as_str()))
            .map(String::as_str)
            .filter(|_| !self.names.iter().any(|name| name == cop))
    }
}

pub(super) fn is_department(name: &str) -> bool {
    DEPARTMENTS.contains(&name)
}

/// Every `disable`, `todo` and `enable` comment of the file, in source order.
///
/// `push` and `pop` are skipped: neither cop looks at them.
pub(super) fn directives(context: &RuleContext<'_>) -> Vec<Directive> {
    let mut found = Vec::new();
    for comment in context.comment_ranges() {
        let text = context.source.slice(comment.clone());
        let Some((marker, mode, rest)) = header(text) else {
            continue;
        };
        let (names, all, end) = cop_list(rest);
        if names.is_empty() && !all {
            continue;
        }
        let (line, _) = context.source.line_column(comment.start);
        let before = &context.source.line(line)[..comment.start - context.source.line_start(line)];
        found.push(Directive {
            comment: comment.clone(),
            mode,
            names,
            all,
            comment_only_line: before.trim().is_empty(),
            range: comment.start..comment.start + marker + end,
        });
    }
    found
}

/// `DIRECTIVE_HEADER_PATTERN`: the marker, the mode, and where the cop list starts.
fn header(text: &str) -> Option<(usize, Mode, &str)> {
    let bytes = text.as_bytes();
    let mut index = 0;
    if bytes.first() != Some(&b'#') {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if !text[index..].starts_with("rubocop") {
        return None;
    }
    index += "rubocop".len();
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b':') {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    for (keyword, mode) in [
        ("disable", Mode::Disable),
        ("todo", Mode::Disable),
        ("enable", Mode::Enable),
    ] {
        if text[index..].starts_with(keyword) {
            return Some((index + keyword.len(), mode, &text[index + keyword.len()..]));
        }
    }
    None
}

/// `COPS_PATTERN`: `all`, or a comma-separated run of cop names.
fn cop_list(text: &str) -> (Vec<String>, bool, usize) {
    let mut index = text.len() - text.trim_start().len();
    if text[index..].starts_with("all") && !next_is_name_character(text, index + 3) {
        return (Vec::new(), true, index + 3);
    }
    let mut names = Vec::new();
    while let Some(length) = cop_name_length(&text[index..]) {
        names.push(text[index..index + length].to_owned());
        index += length;
        let after = index + (text[index..].len() - text[index..].trim_start().len());
        if text[after..].starts_with(',') {
            let next = after + 1;
            index = next + (text[next..].len() - text[next..].trim_start().len());
        } else {
            break;
        }
    }
    (names, false, index)
}

fn next_is_name_character(text: &str, index: usize) -> bool {
    text.as_bytes()
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

/// `COP_NAME_PATTERN`: one or more `[A-Za-z]\w+` segments joined by slashes.
fn cop_name_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    loop {
        let start = index;
        if !bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            index += 1;
        }
        if index - start < 2 {
            return None;
        }
        if bytes.get(index) != Some(&b'/') {
            return Some(index);
        }
        index += 1;
    }
}
