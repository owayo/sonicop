//! `Lint/RedundantCopDisableDirective`.
//!
//! The one cop that cannot take part in the walk over the syntax tree: it reports directives that
//! switch off cops which had nothing to say, so it can only run once every other cop has finished.
//! RuboCop gives it a team of its own after the inspection loop and hands it the offenses that were
//! found, including the ones its own directives suppressed; Sonicop hands it the same list through
//! [`RuleContext::directive_review`].

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::directives::{
    CONFIG_DISABLED_LINE, CommentEntry, CopRegistry, DirectiveComment, DirectiveMode,
    END_OF_FILE_LINE, LineRange,
};
use crate::rules::support::final_pos;
use crate::rules::{DirectiveReview, RuleContext};
use crate::source::SourceFile;

const COP_NAME: &str = "Lint/RedundantCopDisableDirective";

/// What `add_department_marker` prefixes a department with so that a redundant department reads
/// differently from a redundant cop by the time the message is written.
const DEPARTMENT_MARKER: &str = "DEPARTMENT";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(review) = context.directive_review() else {
        return;
    };
    let source = context.source;
    let lines = OffenseLines::new(review.offenses, source);
    // `disabled_ranges`: the spans where this cop is itself switched off, which is what stops it
    // from objecting to the directive that silences it.
    let own_ranges = review
        .comments
        .disabled_line_ranges()
        .get(COP_NAME)
        .cloned()
        .unwrap_or_else(|| vec![LineRange { begin: 0, end: 0 }]);

    // Keyed by where the comment starts, which is the identity RuboCop's `Hash` of comments gives.
    let mut redundant: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    let mut record = |comment: &CommentEntry, cop: String| {
        redundant
            .entry(comment.range().start)
            .or_default()
            .insert(cop);
    };

    for (cop, cop_ranges) in review.comments.disabled_line_ranges() {
        each_already_disabled(review, &own_ranges, &lines, cop, cop_ranges, &mut record);
        each_line_range(review, &own_ranges, &lines, cop, cop_ranges, &mut record);
    }

    for (start, cops) in redundant {
        let Some(comment) = review.comments.comment_at_offset(start) else {
            continue;
        };
        add_offenses(context, review, comment, &cops, offenses);
    }
}

/// The lines the run's offenses were reported on, grouped so that asking "did this cop have
/// anything to say inside this span" does not walk every offense in the file.
struct OffenseLines {
    all: Vec<i64>,
    by_cop: BTreeMap<&'static str, Vec<i64>>,
}

impl OffenseLines {
    fn new(offenses: &[Offense], source: &SourceFile) -> Self {
        let mut all = Vec::with_capacity(offenses.len());
        let mut by_cop: BTreeMap<&'static str, Vec<i64>> = BTreeMap::new();
        for offense in offenses {
            let line = offense.start_position(source).0 as i64;
            all.push(line);
            by_cop.entry(offense.cop_name).or_default().push(line);
        }
        Self { all, by_cop }
    }

    /// `range_with_offense?`: nothing the run reported falls inside the span.
    fn none_in(&self, range: LineRange) -> bool {
        !self.all.iter().any(|line| range.covers(*line))
    }

    fn none_for_cop(&self, cop: &str, range: LineRange) -> bool {
        self.by_cop
            .get(cop)
            .is_none_or(|lines| !lines.iter().any(|line| range.covers(*line)))
    }

    /// `find_redundant_department` selects by prefix rather than by department, so a cop whose name
    /// merely starts with the department's letters counts too.
    fn none_for_department(&self, department: &str, range: LineRange) -> bool {
        !self
            .by_cop
            .iter()
            .filter(|(cop, _)| cop.starts_with(department))
            .any(|(_, lines)| lines.iter().any(|line| range.covers(*line)))
    }
}

/// `each_already_disabled`: a span that opens on the line the previous one closed on re-states a
/// directive that was already in force, which is redundant whether or not the cop had anything to
/// say there.
fn each_already_disabled(
    review: &DirectiveReview<'_>,
    own_ranges: &[LineRange],
    lines: &OffenseLines,
    cop: &str,
    cop_ranges: &[LineRange],
    record: &mut impl FnMut(&CommentEntry, String),
) {
    for pair in cop_ranges.windows(2) {
        let (previous, range) = (pair[0], pair[1]);
        if ignore_offense(range, own_ranges) || previous.end != range.begin {
            continue;
        }
        let Some(comment) = review.comments.comment_at_line(range.begin) else {
            continue;
        };
        // A comment that switches everything off does not count: turning a few cops off and then
        // the rest further down is a reasonable thing to write.
        if all_disabled(comment) {
            continue;
        }
        let redundant = if department_disabled(cop, comment, review.registry) {
            find_redundant_department(cop, range, lines)
        } else {
            Some(cop.to_owned())
        };
        if let Some(redundant) = redundant {
            record(comment, redundant);
        }
    }
}

/// `each_line_range`: a span whose cop reported nothing inside it.
fn each_line_range(
    review: &DirectiveReview<'_>,
    own_ranges: &[LineRange],
    lines: &OffenseLines,
    cop: &str,
    cop_ranges: &[LineRange],
    record: &mut impl FnMut(&CommentEntry, String),
) {
    for (index, range) in cop_ranges.iter().enumerate() {
        if ignore_offense(*range, own_ranges)
            || expected_final_disable(cop, *range, review.registry)
        {
            continue;
        }
        let Some(comment) = review.comments.comment_at_line(range.begin) else {
            continue;
        };
        // `push`/`pop` do not name the cops they restore, so there is nothing to call unnecessary.
        if comment.directive().is_some_and(|directive| {
            matches!(directive.mode, DirectiveMode::Push | DirectiveMode::Pop)
        }) {
            continue;
        }
        let next_range = cop_ranges.get(index + 1).copied();
        let redundant = if all_disabled(comment) {
            find_redundant_all(*range, next_range, lines)
        } else if department_disabled(cop, comment, review.registry) {
            find_redundant_department(cop, *range, lines)
        } else {
            lines.none_for_cop(cop, *range).then(|| cop.to_owned())
        };
        if let Some(redundant) = redundant {
            record(comment, redundant);
        }
    }
}

/// `ignore_offense?`: the configuration rather than a comment closed the span, or this cop is
/// itself switched off across the whole of it.
fn ignore_offense(range: LineRange, own_ranges: &[LineRange]) -> bool {
    range.begin == CONFIG_DISABLED_LINE || own_ranges.iter().any(|own| own.contains(range))
}

/// `expected_final_disable?`: a cop the configuration already turned off, switched off again for
/// the rest of the file.
fn expected_final_disable(cop: &str, range: LineRange, registry: &CopRegistry) -> bool {
    registry.knows(cop) && registry.config_disabled(cop) && range.end == END_OF_FILE_LINE
}

fn all_disabled(comment: &CommentEntry) -> bool {
    comment
        .directive()
        .is_some_and(DirectiveComment::disabled_all)
}

/// `department_disabled?`: the comment reaches this cop through its department and does not also
/// name the cop itself.
fn department_disabled(cop: &str, comment: &CommentEntry, registry: &CopRegistry) -> bool {
    comment.directive().is_some_and(|directive| {
        directive.in_directive_department(cop, registry)
            && !directive.overridden_by_department(cop, registry)
    })
}

/// `find_redundant_all`. A `disable all` followed directly by a directive naming one cop is left
/// alone: if it really is unnecessary, the span examined for that other cop says so, and it covers
/// the whole of the `all`.
fn find_redundant_all(
    range: LineRange,
    next_range: Option<LineRange>,
    lines: &OffenseLines,
) -> Option<String> {
    let followed = next_range.is_some_and(|next| range.end == next.begin);
    (!followed && lines.none_in(range)).then(|| "all".to_owned())
}

fn find_redundant_department(cop: &str, range: LineRange, lines: &OffenseLines) -> Option<String> {
    let department = cop.split('/').next().unwrap_or(cop);
    lines
        .none_for_department(department, range)
        .then(|| format!("{DEPARTMENT_MARKER}{department}"))
}

fn add_offenses(
    context: &RuleContext<'_>,
    review: &DirectiveReview<'_>,
    comment: &CommentEntry,
    cops: &BTreeSet<String>,
    offenses: &mut Vec<Offense>,
) {
    let Some(directive) = comment.directive() else {
        return;
    };
    if all_disabled(comment) || directive.directive_count() == cops.len() {
        add_offense_for_entire_comment(context, review, comment, directive, cops, offenses);
    } else {
        add_offense_for_some_cops(context, review, comment, cops, offenses);
    }
}

fn add_offense_for_entire_comment(
    context: &RuleContext<'_>,
    review: &DirectiveReview<'_>,
    comment: &CommentEntry,
    directive: &DirectiveComment,
    cops: &BTreeSet<String>,
    offenses: &mut Vec<Offense>,
) {
    let source = context.source;
    let names: Vec<String> = cops
        .iter()
        .map(|cop| describe(cop, review.registry))
        .collect();
    let removal = comment_range_with_surrounding_space(source, &directive.range, comment);
    // `leave_free_comment?`: whatever the comment said besides the directive has to stay, and if
    // it is not itself a comment it needs a `#` put back in front of it.
    let replacement = leave_free_comment(source, comment, &removal).then(|| " # ".to_owned());
    offenses.push(
        context
            .offense(message(&names.join(", ")), directive.range.clone())
            .corrected_by(Edit {
                start: removal.start,
                end: removal.end,
                replacement: replacement.unwrap_or_default(),
                safe: true,
            }),
    );
}

fn add_offense_for_some_cops(
    context: &RuleContext<'_>,
    review: &DirectiveReview<'_>,
    comment: &CommentEntry,
    cops: &BTreeSet<String>,
    offenses: &mut Vec<Offense>,
) {
    let source = context.source;
    let mut cop_ranges: Vec<(&String, Range<usize>)> = cops
        .iter()
        .filter_map(|cop| Some((cop, cop_range(source, comment, cop)?)))
        .collect();
    cop_ranges.sort_by_key(|(_, range)| range.start);
    let ranges: Vec<Range<usize>> = cop_ranges.iter().map(|(_, range)| range.clone()).collect();

    for (cop, range) in &cop_ranges {
        let removal = directive_range_in_list(source, range, &ranges);
        offenses.push(
            context
                .offense(message(&describe(cop, review.registry)), range.clone())
                .corrected_by(Edit {
                    start: removal.start,
                    end: removal.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

fn message(cop_names: &str) -> String {
    format!("Unnecessary disabling of {cop_names}.")
}

/// `describe`: how one entry of the message names what it found.
fn describe(cop: &str, registry: &CopRegistry) -> String {
    if cop == "all" {
        return "all cops".to_owned();
    }
    if let Some(department) = cop.strip_prefix(DEPARTMENT_MARKER) {
        return format!("`{department}` department");
    }
    if registry.knows(cop) {
        return format!("`{cop}`");
    }
    match crate::directives::find_similar_name(cop, registry.names()) {
        Some(similar) => format!("`{cop}` (did you mean `{similar}`?)"),
        None => format!("`{cop}` (unknown cop)"),
    }
}

/// `cop_range`: where the comment spells the cop out, matched as a whole token so that a shorter
/// name is not found inside a longer one sharing its prefix.
fn cop_range(source: &SourceFile, comment: &CommentEntry, cop: &str) -> Option<Range<usize>> {
    let cop = cop.strip_prefix(DEPARTMENT_MARKER).unwrap_or(cop);
    matching_range(source, comment, cop).or_else(|| {
        let unqualified = cop.rsplit('/').next().unwrap_or(cop);
        matching_range(source, comment, unqualified)
    })
}

fn matching_range(
    source: &SourceFile,
    comment: &CommentEntry,
    needle: &str,
) -> Option<Range<usize>> {
    let range = comment.range();
    let haystack = source.slice(range.clone());
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let followed_by_word = haystack[end..]
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if !followed_by_word {
            return Some(range.start + start..range.start + end);
        }
        from = start + 1;
    }
    None
}

/// `comment_range_with_surrounding_space`: what taking the whole comment out has to take with it.
fn comment_range_with_surrounding_space(
    source: &SourceFile,
    directive: &Range<usize>,
    comment: &CommentEntry,
) -> Range<usize> {
    if previous_line_blank(source, comment.line())
        && comment.comment_only_line()
        && directive.start == comment.range().start
    {
        // A blank line before the comment is worth keeping, so only what follows is eaten.
        return grow_right(source, directive.clone(), true);
    }
    // Otherwise the comment goes along with the space and the newline in front of it, and with the
    // newline behind it when the comment opened the file.
    let leading_newline = directive.start == 0;
    let range = grow_left(source, directive.clone());
    grow_right(source, range, leading_newline)
}

/// `previous_line_blank?`. `Buffer#source_line` indexes an array, so the line before the first one
/// is the last line of the file -- which for a file ending in a newline is the empty string the
/// split leaves behind.
fn previous_line_blank(source: &SourceFile, line: usize) -> bool {
    let previous = match line {
        1 => source.line_count(),
        _ => line - 1,
    };
    source.line(previous).trim_start().is_empty()
}

/// `leave_free_comment?`.
fn leave_free_comment(source: &SourceFile, comment: &CommentEntry, removal: &Range<usize>) -> bool {
    let text = source.slice(comment.range());
    let removed = source.slice(removal.clone()).trim();
    let free = match removed.is_empty() {
        true => text.to_owned(),
        false => text.replace(removed, ""),
    };
    !free.is_empty() && !free.starts_with('#')
}

/// `directive_range_in_list`: what taking one cop out of a list has to take with it.
fn directive_range_in_list(
    source: &SourceFile,
    range: &Range<usize>,
    ranges: &[Range<usize>],
) -> Range<usize> {
    let mut range = range.clone();
    // Eat the comma on the left when nothing between this cop and the end of the line survives.
    if ranges
        .last()
        .is_some_and(|last| ends_its_line(source, last))
        && trailing_range(source, ranges, &range)
    {
        range = grow_left(source, range);
        range = grow_comma_left(source, range);
    }
    range = grow_comma_right(source, range);
    grow_right_spaces_only(source, range)
}

/// `ends_its_line?`: nothing but whitespace follows the range on its line.
fn ends_its_line(source: &SourceFile, range: &Range<usize>) -> bool {
    let (line, _) = source.line_column(range.end);
    // `Buffer#source_line` hands back the line without its newline, so the run of trailing
    // whitespace the comparison looks for stops before it.
    let text = source.line(line);
    let text = text.strip_suffix('\n').unwrap_or(text);
    let trimmed = text.trim_end_matches([' ', '\t', '\r', '\x0b', '\x0c']);
    source.line_start(line) + trimmed.len() == range.end
}

/// `trailing_range?`: everything between this cop and the last one being taken out is just commas
/// and spaces, so no cop that stays behind sits after it on the line.
fn trailing_range(source: &SourceFile, ranges: &[Range<usize>], range: &Range<usize>) -> bool {
    let Some(position) = ranges.iter().position(|other| other == range) else {
        return true;
    };
    ranges[position..].windows(2).all(|pair| {
        let between = source.slice(pair[0].end..pair[1].start);
        let trimmed = between.trim_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c']);
        trimmed == ","
    })
}

/// `range_with_surrounding_space(side: :left, newlines: true)`.
fn grow_left(source: &SourceFile, range: Range<usize>) -> Range<usize> {
    final_pos(source.text(), range.start, false, true, false)..range.end
}

/// `range_with_surrounding_space(side: :right)`, with the newline eaten only when asked.
fn grow_right(source: &SourceFile, range: Range<usize>, newlines: bool) -> Range<usize> {
    range.start..final_pos(source.text(), range.end, true, newlines, false)
}

fn grow_right_spaces_only(source: &SourceFile, range: Range<usize>) -> Range<usize> {
    grow_right(source, range, false)
}

/// `range_with_surrounding_comma(:left)`.
fn grow_comma_left(source: &SourceFile, range: Range<usize>) -> Range<usize> {
    let bytes = source.text().as_bytes();
    let mut start = range.start;
    while start > 0 && bytes[start - 1] == b',' {
        start -= 1;
    }
    start..range.end
}

/// `range_with_surrounding_comma(:right)`.
fn grow_comma_right(source: &SourceFile, range: Range<usize>) -> Range<usize> {
    let bytes = source.text().as_bytes();
    let mut end = range.end;
    while end < bytes.len() && bytes[end] == b',' {
        end += 1;
    }
    range.start..end
}
