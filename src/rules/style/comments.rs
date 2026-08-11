//! Comments as the cops that read them see them: which ones a definition owns, and which of those
//! say something rather than configure something.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use crate::rules::RuleContext;

/// `DirectiveComment::DIRECTIVE_COMMENT_REGEXP`, reduced to the part that decides whether a comment
/// is one at all. Upstream leaves it unanchored, so a directive appended to prose still counts.
static RUBOCOP_DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#\s*rubocop\s*:\s*(disable-next|todo-next|disable|enable|todo|push|pop)\b")
        .unwrap()
});

/// `Parser::Source::Buffer::ENCODING_RE`, which decides whether the parser's comment associator
/// skips the file's opening comment as an encoding line.
static ENCODING_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\s#](?:en)?coding\s*[:=]\s*[A-Za-z0-9_-]").unwrap());

/// The file's comments, ordered, with the leading directives the parser's associator consumes
/// before it starts walking already dropped.
pub(super) struct PrecedingComments {
    ranges: Vec<Range<usize>>,
}

impl PrecedingComments {
    pub(super) fn new(context: &RuleContext<'_>) -> Self {
        let text = context.source.text();
        let mut ranges: Vec<Range<usize>> = context.comment_ranges().to_vec();
        // `advance_through_directives`: a shebang, then a magic comment, then an encoding line, all
        // taken from the very front of the comment stream and associated with no node at all.
        let mut skipped = 0;
        if ranges
            .first()
            .is_some_and(|range| text[range.clone()].starts_with("#!"))
        {
            skipped += 1;
        }
        if ranges
            .get(skipped)
            .is_some_and(|range| is_magic_comment(&text[range.clone()]))
        {
            skipped += 1;
        }
        if ranges
            .get(skipped)
            .is_some_and(|range| ENCODING_LINE.is_match(&text[range.clone()]))
        {
            skipped += 1;
        }
        ranges.drain(..skipped);
        Self { ranges }
    }

    /// The comments the parser's associator hands to a node starting at `offset` as its leading
    /// ones.
    ///
    /// Those are the comments standing alone on the lines directly above it: a comment sharing a
    /// line with code decorates that code instead, and anything beyond the nearest code has already
    /// been taken by whatever node followed it.
    pub(super) fn above(&self, context: &RuleContext<'_>, offset: usize) -> Vec<Range<usize>> {
        let text = context.source.text();
        let mut owned = Vec::new();
        let mut cursor = offset;
        for range in self
            .ranges
            .iter()
            .rev()
            .skip_while(|range| range.end > offset)
        {
            if !text[range.end..cursor].trim().is_empty() {
                break;
            }
            let (line, _) = context.source.line_column(range.start);
            let line_start = context.source.line_start(line);
            if !text[line_start..range.start].trim().is_empty() {
                break;
            }
            owned.push(range.clone());
            cursor = range.start;
        }
        owned.reverse();
        owned
    }
}

pub(super) fn is_rubocop_directive(comment: &str) -> bool {
    RUBOCOP_DIRECTIVE.is_match(comment)
}

/// `Parser::Source::Comment::Associator::MAGIC_COMMENT_RE`, whose back-reference makes the `-*-`
/// wrapper all-or-nothing.
fn is_magic_comment(comment: &str) -> bool {
    comment.lines().any(|line| {
        let Some(rest) = line.strip_prefix('#') else {
            return false;
        };
        let rest = rest.trim_start();
        let (rest, wrapped) = match rest.strip_prefix("-*-") {
            Some(inner) => (inner.trim_start(), true),
            None => (rest, false),
        };
        let Some(rest) = ["frozen_string_literal", "warn_indent", "warn_past_scope"]
            .iter()
            .find_map(|key| rest.strip_prefix(key))
        else {
            return false;
        };
        let Some(rest) = rest.strip_prefix(':') else {
            return false;
        };
        match wrapped {
            true => rest.ends_with("-*-"),
            false => true,
        }
    })
}

/// `AnnotationComment#annotation?`: the comment opens with one of the annotation keywords and is
/// not merely a sentence that starts with the word.
pub(super) fn is_annotation(comment: &str, keywords: &AnnotationKeywords) -> bool {
    let Some(captures) = keywords
        .regex
        .as_ref()
        .and_then(|regex| regex.captures(comment))
    else {
        return false;
    };
    let Some(keyword) = captures.get(2).map(|group| group.as_str()) else {
        return false;
    };
    let colon = captures.get(3);
    let space = captures.get(4);
    let note = captures.get(5);
    if colon.is_none() && space.is_none() {
        return false;
    }
    // `just_keyword_of_sentence?`: `# Note that ...` is prose, `# NOTE that ...` is an annotation.
    let sentence =
        keyword == capitalize(keyword) && colon.is_none() && space.is_some() && note.is_some();
    !sentence
}

/// The `Style/CommentAnnotation` keywords compiled into the pattern `AnnotationComment` builds from
/// them. Built once per file rather than once per comment.
pub(super) struct AnnotationKeywords {
    regex: Option<Regex>,
}

impl AnnotationKeywords {
    pub(super) fn new(context: &RuleContext<'_>) -> Self {
        let keywords: Vec<String> = context
            .setting_of("Style/CommentAnnotation", "Keywords")
            .unwrap_or_default();
        Self {
            regex: annotation_regex(&keywords),
        }
    }
}

fn capitalize(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => {
            first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
        }
        None => String::new(),
    }
}

/// The keywords come from `Style/CommentAnnotation`, so the pattern is built per configuration
/// rather than once. Upstream sorts them longest first so that a keyword that is a prefix of
/// another cannot win.
fn annotation_regex(keywords: &[String]) -> Option<Regex> {
    if keywords.is_empty() {
        return None;
    }
    let mut sorted: Vec<&str> = keywords.iter().map(String::as_str).collect();
    sorted.sort_by_key(|keyword| std::cmp::Reverse(keyword.len()));
    let alternatives = sorted
        .iter()
        .map(|keyword| regex::escape(keyword))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!(
        r"(?mi)^(# ?)(\b(?:{alternatives})\b)(\s*:)?(\s+)?(\S+)?"
    ))
    .ok()
}
