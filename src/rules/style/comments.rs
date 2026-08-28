//! Comments as the cops that read them see them: which ones a definition owns, and which of those
//! say something rather than configure something.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use crate::rules::RuleContext;

/// `DirectiveComment::DIRECTIVE_COMMENT_REGEXP`, reduced to the part that decides whether a comment
/// is one at all. Upstream leaves it unanchored, so a directive appended to prose still counts.
static RUBOCOP_DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#(?-u:\s)*rubocop(?-u:\s)*:(?-u:\s)*(disable-next|todo-next|disable|enable|todo|push|pop)(?-u:\b)")
        .unwrap()
});

/// `Parser::Source::Buffer::ENCODING_RE`, which decides whether the parser's comment associator
/// skips the file's opening comment as an encoding line.
static ENCODING_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[[:space:]#](?:en)?coding(?-u:\s)*[:=](?-u:\s)*[A-Za-z0-9_-]").unwrap()
});

/// The file's comments, ordered, with the leading directives the parser's associator consumes
/// before it starts walking already dropped.
/// What may stand before a comment and still leave it to the node that follows.
///
/// The associator gives a comment to the node that *ends* on its line, and to the following node
/// where nothing does. These keywords open or divide a body without being nodes of their own, so
/// `else # note` decorates whatever comes after it while `x = 1 # note` decorates the assignment.
const KEYWORD_ONLY_LINES: &[&str] = &["else", "begin", "ensure", "do", "then", "rescue", "in"];

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
            let before = text[line_start..range.start].trim();
            if !before.is_empty() && !KEYWORD_ONLY_LINES.contains(&before) {
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
    regex: Option<&'static Regex>,
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
fn annotation_regex(keywords: &[String]) -> Option<&'static Regex> {
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
    crate::rules::regex_cache::compiled(&format!(
        r"(?mi)^(# ?)(\b(?:{alternatives})\b)(\s*:)?(\s+)?(\S+)?"
    ))
}

/// The comments of the file indexed by line, as `processed_source.comment_index` holds them.
///
/// The cops built on `StatementModifier` all ask the same three questions of it: whether a line
/// carries a comment, which comment that is, and whether any line of a span does.
pub(super) struct CommentIndex {
    by_line: HashMap<usize, Range<usize>>,
}

impl CommentIndex {
    pub(super) fn new(context: &RuleContext<'_>) -> Self {
        Self {
            by_line: context
                .comment_ranges()
                .iter()
                .map(|range| (context.source.line_column(range.start).0, range.clone()))
                .collect(),
        }
    }

    pub(super) fn at_line(&self, line: usize) -> Option<Range<usize>> {
        self.by_line.get(&line).cloned()
    }

    pub(super) fn on_line(&self, line: usize) -> bool {
        self.by_line.contains_key(&line)
    }

    /// `contains_comment?`, which asks about whole lines rather than the range itself.
    pub(super) fn in_lines(&self, lines: Range<usize>) -> bool {
        lines.into_iter().any(|line| self.on_line(line))
    }
}

/// The range upstream's `Parser::Source::Comment` covers.
///
/// For a `#` comment the grammar and the parser agree. For a `=begin` block the parser runs to the
/// end of the line `=end` sits on, newline included, while the grammar stops at the last character
/// of that line -- so a cop that reports "the comment" has to add the rest of the line back.
pub(super) fn parser_range(range: &Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    let text = context.source.text();
    if !text[range.clone()].starts_with("=begin") {
        return range.clone();
    }
    let end = text[range.end..].find('\n').map_or_else(
        // **A file that does not end in a newline still has one to the parser**, and a block
        // comment's range runs one character past even that.
        || text.len() + 2,
        |offset| range.end + offset + 1,
    );
    range.start..end
}
