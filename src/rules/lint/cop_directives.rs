//! `DirectiveComment` and the part of `CommentConfig` the two directive cops read.
//!
//! `src/directives.rs` answers the one question the engine asks -- whether an offense is switched
//! off where it was found -- and answers it per line. These two cops ask about the directives
//! themselves: which comment turned a cop off, whether the switch was ever turned back on, and
//! whether an `enable` had anything to undo. That is a different model of the same comments, kept
//! here rather than folded into the engine's.
//!
//! Only the model differs. Finding the directive inside the comment is `DirectiveComment`'s job in
//! both, so this module asks `crate::directives` for it rather than keeping a second reading of
//! the same pattern.

use std::ops::Range;

use crate::directives::cop_name_length;
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
    /// `# rubocop:push`: saves the state the directives have built so far.
    Push,
    /// `# rubocop:pop`: restores it, closing every range `push` did not already hold open.
    Pop,
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
    /// `DirectiveComment#single_line?`: the directive does not open the comment, so it applies to
    /// its own line only.
    pub single_line: bool,
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

    /// `parsed_cop_names`: the names the directive acts on, with a department standing for every
    /// cop in it.
    ///
    /// **A department is one word to the reader and a hundred names to the counting**, which is the
    /// whole difference between the two lists: [`Self::names`] is what the comment says, this is
    /// what it does. `handle_switch` counts one outstanding disable per name here, so a
    /// `# rubocop:disable Layout/LineLength` followed by a `# rubocop:enable Layout` undoes that
    /// one cop and enables the other hundred-odd for nothing.
    ///
    /// Upstream expands from `Cop::Registry.global` -- the whole registry rather than the run's --
    /// so `--only` does not narrow what a department stands for.
    pub(super) fn parsed_names<'d>(&'d self) -> Vec<&'d str> {
        if self.all {
            // `parsed_cop_names` reads `raw_cop_names`, which for a blanket directive is the one
            // word `all`. Only `cop_names` expands it, and `match?` does not go through that.
            return vec![ALL];
        }
        let mut names: Vec<&'d str> = Vec::new();
        for name in &self.names {
            if is_department(name) {
                for cop in cop_names_for_department(name) {
                    names.push(cop);
                }
            } else {
                names.push(name.as_str());
            }
        }
        // `parsed_cop_names`: `cops - [LINT_SYNTAX_COP]`. A directive may name it, and nothing
        // counts it.
        names.retain(|name| *name != LINT_SYNTAX_COP);
        names
    }

    /// `match?`: whether the directive acts on exactly these names.
    ///
    /// Both sides are `uniq.sort`ed, so a name written twice matches once -- and the comparison is
    /// against [`Self::parsed_names`], the expanded list, which is why a department directive that
    /// undid even one cop's disable does not match the names it enabled for nothing.
    pub(super) fn matches(&self, names: &[&str]) -> bool {
        uniq(&self.parsed_names()) == uniq(names)
    }
}

pub(super) fn is_department(name: &str) -> bool {
    DEPARTMENTS.contains(&name)
}

/// `uniq.sort`, which is how `match?` compares its two lists.
fn uniq<'n>(names: &[&'n str]) -> Vec<&'n str> {
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

/// The word that stands for every cop at once.
pub(super) const ALL: &str = "all";

const LINT_SYNTAX_COP: &str = "Lint/Syntax";
const LINT_REDUNDANT_DIRECTIVE_COP: &str = "Lint/RedundantCopDisableDirective";

/// `exclude_lint_department_cops`: the two names neither an `all` nor a `Lint` department directive
/// stands for. Both are cops a directive must not be able to switch off.
pub(super) fn reached_by_all(name: &str) -> bool {
    name != LINT_REDUNDANT_DIRECTIVE_COP && name != LINT_SYNTAX_COP
}

/// `cop_names_for_department`: every cop the department holds.
///
/// The registry walked here is the static one, matching upstream's `Cop::Registry.global`: what a
/// department stands for is a property of the cops that exist, not of the cops this run selected.
fn cop_names_for_department(department: &str) -> impl Iterator<Item = &'static str> {
    let prefix = format!("{department}/");
    let lint = department == "Lint";
    crate::rules::rule_names()
        .filter(move |name| name.starts_with(&prefix))
        .filter(move |name| !lint || reached_by_all(name))
}

/// Every `disable`, `todo`, `enable`, `push` and `pop` comment of the file, in source order.
///
/// `push` and `pop` name no cops, so the emptiness check below has to let them through -- a range
/// opened inside a `push` is closed by the matching `pop` rather than by an `enable`.
pub(super) fn directives(context: &RuleContext<'_>) -> Vec<Directive> {
    let mut found = Vec::new();
    for comment in context.comment_ranges() {
        let text = context.source.slice(comment.clone());
        let Some((start, mode_end, mode, rest)) = header(text) else {
            continue;
        };
        let (names, all, end) = cop_list(rest);
        if names.is_empty() && !all && !matches!(mode, Mode::Push | Mode::Pop) {
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
            single_line: start != 0,
            // `DirectiveComment#range` spans the match, which need not open the comment.
            range: comment.start + start..comment.start + mode_end + end,
        });
    }
    found
}

/// Where the marker matched, where the cop list starts, and the mode.
///
/// The pattern matches the *first* marker in the comment whichever mode it names, so a `push` or
/// `pop` is the mode of that comment rather than something to search past for a later `disable`.
fn header(text: &str) -> Option<(usize, usize, Mode, &str)> {
    let header = crate::directives::directive_header(text)?;
    let mode = match header.mode {
        "disable" | "todo" => Mode::Disable,
        "enable" => Mode::Enable,
        "push" => Mode::Push,
        "pop" => Mode::Pop,
        _ => return None,
    };
    Some((
        header.start,
        header.mode_end,
        mode,
        &text[header.mode_end..],
    ))
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
