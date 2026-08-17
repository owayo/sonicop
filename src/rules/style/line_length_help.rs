//! `LineLengthHelp`: what the cops that build or judge one-line forms need to know about
//! `Layout/LineLength`.
//!
//! `Style/IfUnlessModifier` and the other `StatementModifier` cops decide twice over whether a line
//! is too long, and the two questions are not the same one. A line already in the source is judged
//! by `acceptable_line_length?`, which honours every exemption `Layout/LineLength` offers. A line
//! the cop is about to *create* is measured against the bare maximum, because those exemptions
//! describe long lines the user tolerates rather than permission to write new ones.

use std::cell::OnceCell;
use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostic::{Offense, Severity};
use crate::directives::DirectiveState;
use crate::rules::RuleContext;
use crate::rules::support::is_ruby_space_char;

pub(super) struct LineLengthHelp<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    /// `max_line_length`, which is `nil` when `Layout/LineLength` is switched off entirely.
    max: Option<usize>,
    tab_indentation_width: usize,
    allow_uri: bool,
    allow_cop_directives: bool,
    allowed_patterns: Vec<Regex>,
    /// `processed_source.comment_config`, built only for a file that carries a directive at all:
    /// walking every line to fold the directive stack is far more work than the answer is worth
    /// when nothing in the file can turn a cop off.
    directives: OnceCell<Option<DirectiveState>>,
}

impl<'a, 'tree> LineLengthHelp<'a, 'tree> {
    pub(super) fn new(context: &'a RuleContext<'tree>) -> Self {
        let enabled = context
            .setting_of::<bool>("Layout/LineLength", "Enabled")
            .unwrap_or(true);
        Self {
            context,
            max: enabled
                .then(|| {
                    context
                        .setting_of("Layout/LineLength", "Max")
                        .unwrap_or(120)
                })
                .or(None),
            tab_indentation_width: context
                .setting_of("Layout/IndentationStyle", "IndentationWidth")
                .or_else(|| context.setting_of("Layout/IndentationWidth", "Width"))
                .unwrap_or(2),
            allow_uri: context
                .setting_of("Layout/LineLength", "AllowURI")
                .unwrap_or(true),
            allow_cop_directives: allow_cop_directives(context),
            allowed_patterns: allowed_patterns(context),
            directives: OnceCell::new(),
        }
    }

    pub(super) fn max(&self) -> Option<usize> {
        self.max
    }

    /// `line_length`: characters plus the extra columns a leading tab stands in for.
    pub(super) fn line_length(&self, line: &str) -> usize {
        line.chars().count() + self.indentation_difference(line)
    }

    fn indentation_difference(&self, line: &str) -> usize {
        if !line.starts_with('\t') {
            return 0;
        }
        match line.find(|character| character != '\t') {
            Some(offset) => offset * (self.tab_indentation_width - 1),
            None => 0,
        }
    }

    /// `acceptable_line_length?`: whether a line already written in the source is one
    /// `Layout/LineLength` would leave alone, exemptions included.
    pub(super) fn acceptable_line_length(&self, line: &str, line_number: usize) -> bool {
        let Some(max) = self.max else {
            return true;
        };
        if self.line_length(line) <= max {
            return true;
        }
        if !self.line_length_enabled_at_line(line_number) {
            return true;
        }
        if self
            .allowed_patterns
            .iter()
            .any(|pattern| pattern.is_match(line))
        {
            return true;
        }
        if self.allow_cop_directives && self.directive_on_source_line(line_number) {
            return length_without_directive(line) <= max;
        }
        self.allowed_by_uri(line, max)
    }

    fn allowed_by_uri(&self, line: &str, max: usize) -> bool {
        if !self.allow_uri {
            return false;
        }
        let indent = self.indentation_difference(line);
        match excessive_uri_range(line, max, indent) {
            // `allowed_position?`: the URI starts before the limit and runs to the very end, so
            // nothing after it could have been wrapped instead.
            Some((begin, end)) => begin < max && end == self.line_length(line),
            None => false,
        }
    }

    fn line_length_enabled_at_line(&self, line_number: usize) -> bool {
        let directives = self.directives.get_or_init(|| {
            let text = self.context.source.text();
            self.context
                .comment_ranges()
                .iter()
                .any(|range| DIRECTIVE.is_match(&text[range.clone()]))
                .then(|| DirectiveState::parse(self.context.source, self.context.comment_ranges()))
        });
        let Some(directives) = directives else {
            return true;
        };
        let start = self.context.source.line_start(line_number);
        let probe = Offense::new(
            "Layout/LineLength",
            Severity::Convention,
            String::new(),
            start,
            start,
        );
        directives
            .suppression(&probe, self.context.source)
            .is_none()
    }

    fn directive_on_source_line(&self, line_number: usize) -> bool {
        let text = self.context.source.text();
        let range = self.context.source.line_range(line_number);
        self.context
            .comment_ranges()
            .iter()
            .filter(|comment| comment.start >= range.start && comment.start < range.end)
            .any(|comment| DIRECTIVE.is_match(&text[comment.clone()]))
    }
}

fn allow_cop_directives(context: &RuleContext<'_>) -> bool {
    // `IgnoreCopDirectives` is the deprecated spelling and wins outright when it is set at all.
    match context.setting_of::<bool>("Layout/LineLength", "IgnoreCopDirectives") {
        Some(ignore) => ignore,
        None => context
            .setting_of("Layout/LineLength", "AllowCopDirectives")
            .unwrap_or(true),
    }
}

fn allowed_patterns(context: &RuleContext<'_>) -> Vec<Regex> {
    let patterns: Vec<String> = context
        .setting_of("Layout/LineLength", "AllowedPatterns")
        .or_else(|| context.setting_of("Layout/LineLength", "IgnoredPatterns"))
        .unwrap_or_default();
    patterns
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
}

/// `DirectiveComment.before_comment(line).rstrip.length`.
fn length_without_directive(line: &str) -> usize {
    DIRECTIVE
        .find(line)
        .map_or(line, |found| &line[..found.start()])
        .trim_end_matches(is_ruby_space_char)
        .chars()
        .count()
}

/// `find_excessive_range(line, :uri)` as a character range.
fn excessive_uri_range(line: &str, max: usize, indent: usize) -> Option<(usize, usize)> {
    // RuboCop drops matches that `URI.parse` rejects, but the scan still consumed them, so
    // filtering after the scan -- not before -- keeps the remaining matches where they were.
    let found = URI
        .find_iter(line)
        .filter(|found| valid_uri(found.as_str()))
        .last()?;
    let begin = line[..found.start()].chars().count() + indent;
    let end = line[..extended_end(line, found.end())].chars().count() + indent;
    (begin >= max || end >= max).then_some((begin, end))
}

fn extended_end(line: &str, mut end: usize) -> usize {
    // A YARD link -- `# {Some Title}[https://example.com/page]` -- is one unit, so a line that
    // closes with `}` carries the end past the last brace before the word extension below.
    if line.ends_with('}') && line[..line.len() - 1].contains('{') {
        if let Some(offset) = line[end..].rfind('}') {
            end += offset + 1;
        }
    }
    let rest = &line[end..];
    if rest.starts_with(|character| !is_ruby_space_char(character)) {
        end += rest.find(is_ruby_space_char).unwrap_or(rest.len());
    }
    end
}

static DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    // Only where the directive begins matters here, so the cop list that may follow is left out.
    // Longest mode first, so `disable-next` is not read as `disable`.
    Regex::new(r"#(?-u:\s)*rubocop(?-u:\s)*:(?-u:\s)*(?:disable-next|todo-next|disable|enable|todo|push|pop)(?-u:\b)")
        .unwrap()
});

/// RFC 2396 absolute URIs limited to `http`/`https`, as `URI::RFC2396_PARSER.make_regexp` builds
/// them for RuboCop.
static URI: LazyLock<Regex> = LazyLock::new(|| {
    const ESCAPED: &str = r"%[a-fA-F\d]{2}";
    let uric_no_slash = format!(r"(?:[\-_.!~*'()a-zA-Z\d;?:@&=+$,]|{ESCAPED})");
    let uric = format!(r"(?:[\-_.!~*'()a-zA-Z\d;/?:@&=+$,\[\]]|{ESCAPED})");
    let userinfo = format!(r"(?:[\-_.!~*'()a-zA-Z\d;:&=+$,]|{ESCAPED})*");
    let host = format!(
        r"(?:(?:[a-zA-Z0-9\-.]|{ESCAPED})+|\d{{1,3}}\.\d{{1,3}}\.\d{{1,3}}\.\d{{1,3}}|\[[a-fA-F\d:.]+\])"
    );
    let reg_name = format!(r"(?:[\-_.!~*'()a-zA-Z\d$,;:@&=+]|{ESCAPED})+");
    let pchar = format!(r"(?:[\-_.!~*'()a-zA-Z\d:@&=+$,]|{ESCAPED})");
    let segment = format!(r"{pchar}*(?:;{pchar}*)*");
    let abs_path = format!(r"/{segment}(?:/{segment})*");
    Regex::new(&format!(
        r"(?:https?):(?:{uric_no_slash}{uric}*|(?:(?://(?:(?:(?:{userinfo}@)?(?:{host}(?::(?-u:\d)*)?))?|{reg_name}))?(?:{abs_path})?)(?:\?{uric}*)?)(?:\#{uric}*)?"
    ))
    .unwrap()
});

/// Whether `URI.parse` accepts the string, which RuboCop uses to weed out RFC 2396 matches that
/// are not URIs after all.
fn valid_uri(text: &str) -> bool {
    RFC3986_URI.is_match(text)
}

static RFC3986_URI: LazyLock<Regex> = LazyLock::new(|| {
    const PCT: &str = r"%[0-9a-fA-F]{2}";
    let segment = format!(r"(?:{PCT}|[!$&-.0-9:;=@A-Z_a-z~/])");
    let segment_start = format!(r"(?:{PCT}|[!$&-.0-9:;=@A-Z_a-z~])");
    let userinfo = format!(r"(?:{PCT}|[!$&-.0-9:;=A-Z_a-z~])*");
    let host = format!(r"(?:\[[0-9a-fA-F:.v]+\]|(?:{PCT}|[!$&-.0-9;=A-Z_a-z~])*)");
    let authority = format!(r"(?:{userinfo}@)?{host}(?::[0-9]*)?");
    let fragment = format!(r"(?:{PCT}|[!$&-.0-9:;=@A-Z_a-z~/?])*");
    Regex::new(&format!(
        r"\A(?:[A-Za-z][+\-.0-9A-Za-z]*):(?://{authority}(?:/{segment}*)?|/(?:{segment_start}{segment}*)?|{segment_start}{segment}*|)(?:\?[^\#]*)?(?:\#{fragment})?\z"
    ))
    .unwrap()
});
