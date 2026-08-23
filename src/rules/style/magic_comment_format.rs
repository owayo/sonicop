//! `Style/MagicCommentFormat`: one spelling for the directives at the head of a file.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `MagicComment::TOKEN`.
const TOKEN: &str = "[[:alnum:]\\-_]+";

/// `MagicComment::KEYWORDS`, in the order `Regexp.union` puts them.
const KEYWORDS: &[&str] = &[
    "(?:en)?coding",
    "frozen[_-]string[_-]literal",
    "rbs_inline",
    "shareable[_-]constant[_-]value",
    "typed",
];

/// `CommentRange::DIRECTIVE_REGEXP`.
static DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("(?i){}", KEYWORDS.join("|"))).expect("the keyword union is a valid regexp")
});

/// The five anchored patterns `SimpleComment` reads a directive with.
static SIMPLE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let mut patterns = vec![
        Regex::new(&format!(
            "(?i)\\A\\s*#\\s*(frozen_string_literal:\\s*(true|false))?\\s*(?:en)?coding: ({TOKEN})"
        ))
        .expect("valid"),
    ];
    for keyword in &KEYWORDS[1..] {
        patterns.push(
            Regex::new(&format!("(?i)\\A\\s*#\\s*{keyword}:\\s*{TOKEN}\\s*\\z")).expect("valid"),
        );
    }
    patterns
});

/// `EmacsComment::REGEXP` and `VimComment::REGEXP`.
static EMACS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-\*-(.+)-\*-").expect("valid"));
static VIM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#(?-u:\s)*vim:(?-u:\s)*(.+)").expect("valid"));

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let kebab = context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style == "kebab_case");
    // `expected_style` is `[directive_capitalization, style].compact.join(' ')`: an explicit `~`
    // in the configuration means the capitalization is not enforced at all, and the message says
    // "snake" rather than "lower snake". Filling the shipped default back in over a written `null`
    // both enforced a rule the run had turned off and misnamed the style.
    let directive_case = context.setting::<String>("DirectiveCapitalization");
    let value_case = context.setting::<String>("ValueCapitalization");
    let limit = first_non_comment_line(context);
    for comment in context.comment_ranges() {
        let (line, _) = context.source.line_column(comment.start);
        // `leading_comment_lines`: only what stands above the first line of code.
        if line >= limit {
            continue;
        }
        let text = context.source.slice(comment.clone());
        if !is_magic_comment(text) {
            continue;
        }
        for range in directives(text) {
            let source = &text[range.clone()];
            if !offends(source, kebab, directive_case.as_deref()) {
                continue;
            }
            let span = comment.start + range.start..comment.start + range.end;
            offenses.push(
                context
                    .offense(
                        format!(
                            "Prefer {} case for magic comments.",
                            expected_style(kebab, directive_case.as_deref())
                        ),
                        span.clone(),
                    )
                    .corrected_by(Edit {
                        start: span.start,
                        end: span.end,
                        replacement: replace_separator(
                            &replace_capitalization(source, directive_case.as_deref()),
                            kebab,
                        ),
                        safe: true,
                    }),
            );
        }
        let Some(value_case) = value_case.as_deref() else {
            continue;
        };
        for range in values(text) {
            let source = &text[range.clone()];
            if !wrong_capitalization(source, Some(value_case)) {
                continue;
            }
            let span = comment.start + range.start..comment.start + range.end;
            offenses.push(
                context
                    .offense(
                        format!("Prefer {value_case} for magic comment values."),
                        span.clone(),
                    )
                    .corrected_by(Edit {
                        start: span.start,
                        end: span.end,
                        replacement: replace_capitalization(source, Some(value_case)),
                        safe: true,
                    }),
            );
        }
    }
}

/// `directive_offends?`.
fn offends(source: &str, kebab: bool, capitalization: Option<&str>) -> bool {
    source.contains(if kebab { '_' } else { '-' }) || wrong_capitalization(source, capitalization)
}

/// `wrong_capitalization?`.
fn wrong_capitalization(source: &str, capitalization: Option<&str>) -> bool {
    match capitalization {
        Some("lowercase") => source != source.to_lowercase(),
        Some("uppercase") => source != source.to_uppercase(),
        _ => false,
    }
}

fn replace_capitalization(source: &str, capitalization: Option<&str>) -> String {
    match capitalization {
        Some("lowercase") => source.to_lowercase(),
        Some("uppercase") => source.to_uppercase(),
        _ => source.to_owned(),
    }
}

fn replace_separator(source: &str, kebab: bool) -> String {
    let (wrong, right) = if kebab { ('_', '-') } else { ('-', '_') };
    source.replace(wrong, &right.to_string())
}

/// `expected_style`: the two settings joined with the word `case` taken out of each.
fn expected_style(kebab: bool, capitalization: Option<&str>) -> String {
    let style = if kebab { "kebab" } else { "snake" };
    match capitalization {
        Some("lowercase") => format!("lower {style}"),
        Some("uppercase") => format!("upper {style}"),
        _ => style.to_owned(),
    }
}

/// `CommentRange#directives`.
fn directives(text: &str) -> Vec<Range<usize>> {
    DIRECTIVE
        .find_iter(text)
        .map(|found| found.start()..found.end())
        .collect()
}

/// `CommentRange#values`, whose `(.*?)(?=;|$)` reaches to the next `;` or the end of the comment.
fn values(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let Some(directive) = DIRECTIVE.find_at(text, offset) else {
            break;
        };
        let after = directive.end();
        let rest = &text[after..];
        let Some(rest) = rest.strip_prefix(':') else {
            offset = after.max(offset + 1);
            continue;
        };
        let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let start = after + 1 + blanks;
        let end = text[start..]
            .find(';')
            .map_or(text.len(), |position| start + position);
        found.push(start..end);
        offset = end.max(offset + 1);
    }
    found
}

/// `MagicComment.parse(comment.text).valid?`.
fn is_magic_comment(text: &str) -> bool {
    if !text.starts_with('#') {
        return false;
    }
    if let Some(captures) = EMACS.captures(text) {
        return editor_specifies(&captures[1], ';', ':', KEYWORDS);
    }
    if let Some(captures) = VIM.captures(text) {
        // Vim comments only carry an encoding, and only beside another token.
        let tokens: Vec<&str> = captures[1].split(", ").map(str::trim).collect();
        return tokens.len() > 1 && editor_specifies(&captures[1], ',', '=', &["fileencoding"]);
    }
    SIMPLE.iter().any(|pattern| pattern.is_match(text))
}

/// `EditorComment#match`: a token of the comment spelling one of the keywords out.
fn editor_specifies(inner: &str, separator: char, operator: char, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| {
        let pattern = format!("\\A{keyword}\\s*{operator}\\s*{TOKEN}\\z");
        let Ok(pattern) = Regex::new(&pattern) else {
            return false;
        };
        inner
            .split(separator)
            .map(str::trim)
            .any(|token| pattern.is_match(token))
    })
}

/// `leading_comment_lines`: the line the first token that is not a comment sits on.
fn first_non_comment_line(context: &RuleContext<'_>) -> usize {
    let root = context.root_node();
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .find(|child| child.kind_str() != "comment")
        .map_or(usize::MAX, |child| {
            context.source.line_column(child.start_byte()).0
        })
}
