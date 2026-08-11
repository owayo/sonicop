use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// `Layout/IndentationStyle`'s `IndentationWidth` is unset by default, so RuboCop falls back to
/// `Layout/IndentationWidth`'s `Width`, which is 2. A cop only ever sees its own configuration
/// here, so that fallback is spelled out: one leading tab is worth two columns, i.e. one extra
/// column per tab.
const TAB_INDENTATION_WIDTH: usize = 2;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(120);
    let allow_heredoc: bool = context.setting("AllowHeredoc").unwrap_or(true);
    let allow_uri: bool = context.setting("AllowURI").unwrap_or(true);
    let allow_qualified_name: bool = context.setting("AllowQualifiedName").unwrap_or(true);
    let allow_directives = allow_cop_directives(context);
    let break_edits = line_break_edits(context, max);
    let directive_lines = directive_lines(context);
    let endless_method_lines = endless_method_lines(context);

    for line_number in 1..=context.source.line_count() {
        let raw = context.source.line(line_number);
        // RuboCop chomps only the newline, so a CRLF file counts its `\r` as one more character.
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let indent = indentation_difference(line);
        let length = line.chars().count() + indent;
        let line_start = context.source.line_start(line_number);

        if length <= max
            || (line_number == 1 && line.starts_with("#!"))
            || (allow_heredoc && context.in_heredoc(line_start..line_start + line.len()))
        {
            continue;
        }

        // An endless method has a way out of being long -- it can be rewritten as a regular
        // method -- so RuboCop reports it before any exemption gets a say, and reports the whole
        // line even when a cop directive is what pushed it over.
        let (start_column, reported) = if endless_method_lines.contains(&line_number) {
            (max.saturating_sub(indent), length)
        }
        // A cop directive is measured without the directive, so a line that is only long because
        // of `# rubocop:disable ...` is not reported -- but the code before it still is.
        else if allow_directives && directive_lines.contains(&line_number) {
            let without = length_without_directive(line);
            if without <= max {
                continue;
            }
            (max, without)
        } else if allow_uri || allow_qualified_name {
            let uri = allow_uri
                .then(|| excessive_range(line, MatchKind::Uri, max, indent))
                .flatten();
            let name = allow_qualified_name
                .then(|| excessive_range(line, MatchKind::QualifiedName, max, indent))
                .flatten();
            if exempted(uri, name, length, max) {
                continue;
            }
            (excessive_position(uri.or(name), max, indent), length)
        } else {
            (max.saturating_sub(indent), length)
        };

        let offense = context.offense(
            format!("Line is too long. [{reported}/{max}]"),
            line_start + byte_offset(line, start_column)..line_start + byte_offset(line, reported),
        );
        offenses.push(match break_edits.get(&line_number) {
            Some(edit) => offense.corrected_by(edit.clone()),
            None => offense,
        });
    }
}

/// The byte offset of the `column`-th character, clamped to the end of the line.
///
/// Columns are counted the way RuboCop counts them -- characters plus the width a leading tab
/// stands in for -- so a tab-indented line can name a column past its own last character. RuboCop
/// then builds a range that runs into the following line; clamping keeps the reported range inside
/// the line it describes.
fn byte_offset(line: &str, column: usize) -> usize {
    line.char_indices()
        .nth(column)
        .map_or(line.len(), |(offset, _)| offset)
}

fn allow_cop_directives(context: &RuleContext<'_>) -> bool {
    // `IgnoreCopDirectives` is the deprecated spelling and wins outright when it is set at all.
    match context.setting::<bool>("IgnoreCopDirectives") {
        Some(ignore) => ignore,
        None => context.setting("AllowCopDirectives").unwrap_or(true),
    }
}

/// Ruby's `\s`, which is ASCII-only. Using `char::is_whitespace` here would let a non-breaking
/// space end a word that RuboCop keeps going through.
fn is_ruby_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n' | '\u{b}' | '\u{c}')
}

fn indentation_difference(line: &str) -> usize {
    if !line.starts_with('\t') {
        return 0;
    }
    match line.find(|character| character != '\t') {
        Some(offset) => offset * (TAB_INDENTATION_WIDTH - 1),
        // A line of nothing but tabs has no indentation to measure against.
        None => 0,
    }
}

/// Lines holding a comment that RuboCop reads as a `# rubocop:...` directive.
///
/// The directive has to start the comment: `## rubocop:disable Foo` is prose about a directive,
/// not one, and RuboCop drops the match when everything before it is just the comment marker.
fn directive_lines(context: &RuleContext<'_>) -> HashSet<usize> {
    context
        .comment_ranges()
        .iter()
        .filter(|range| {
            let text = context.source.slice((*range).clone());
            DIRECTIVE
                .find(text)
                .is_some_and(|found| !marker_only(&text[..found.start()]))
        })
        .map(|range| context.source.line_column(range.start).0)
        .collect()
}

/// Lines an endless method definition starts on.
///
/// The grammar tells the two forms apart by the closing keyword: `def foo = bar` has no `end`.
fn endless_method_lines(context: &RuleContext<'_>) -> HashSet<usize> {
    context
        .nodes_of_any(&["method", "singleton_method"])
        .filter(|node| {
            let mut cursor = node.walk();
            !node
                .children(&mut cursor)
                .any(|child| child.kind() == "end")
        })
        .map(|node| node.start_position().row + 1)
        .collect()
}

fn marker_only(prefix: &str) -> bool {
    prefix
        .strip_prefix('#')
        .is_some_and(|rest| rest.chars().all(is_ruby_space))
}

fn length_without_directive(line: &str) -> usize {
    DIRECTIVE
        .find(line)
        .map_or(line, |found| &line[..found.start()])
        .trim_end_matches(is_ruby_space)
        .chars()
        .count()
}

#[derive(Clone, Copy)]
enum MatchKind {
    Uri,
    QualifiedName,
}

/// The last URI or qualified name on the line, as a character range, when it reaches past `max`.
///
/// RuboCop pushes the end of the match to the end of the word it sits in, so a URI wrapped in
/// quotes or parens still counts as reaching the end of the line.
fn excessive_range(
    line: &str,
    kind: MatchKind,
    max: usize,
    indent: usize,
) -> Option<(usize, usize)> {
    let found = match kind {
        // RuboCop drops matches that `URI.parse` rejects, but the scan still consumed them, so
        // filtering after the scan -- not before -- keeps the remaining matches where they were.
        MatchKind::Uri => URI
            .find_iter(line)
            .filter(|found| valid_uri(found.as_str()))
            .last()?,
        MatchKind::QualifiedName => QUALIFIED_NAME.find_iter(line).last()?,
    };
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
    if rest.starts_with(|character| !is_ruby_space(character)) {
        end += rest.find(is_ruby_space).unwrap_or(rest.len());
    }
    end
}

/// A URI or qualified name excuses the line only when it starts before the limit and runs to the
/// very end: anything after it could have been wrapped instead.
fn allowed_position(range: (usize, usize), length: usize, max: usize) -> bool {
    range.0 < max && range.1 == length
}

fn exempted(
    uri: Option<(usize, usize)>,
    name: Option<(usize, usize)>,
    length: usize,
    max: usize,
) -> bool {
    match (uri, name) {
        (Some(uri), Some(name)) => {
            allowed_position(uri, length, max) && allowed_position(name, length, max)
        }
        (Some(range), None) | (None, Some(range)) => allowed_position(range, length, max),
        (None, None) => false,
    }
}

/// Where the highlight starts: just past the URI or qualified name that was allowed to overrun,
/// otherwise the limit itself.
fn excessive_position(range: Option<(usize, usize)>, max: usize, indent: usize) -> usize {
    match range {
        Some(range) if range.0 < max => range.1,
        _ => max.saturating_sub(indent),
    }
}

static DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    // Only where the directive begins matters here, so the cop list that may follow is left out.
    // Longest mode first, so `disable-next` is not read as `disable`.
    Regex::new(r"#\s*rubocop\s*:\s*(?:disable-next|todo-next|disable|enable|todo|push|pop)\b")
        .unwrap()
});

static QUALIFIED_NAME: LazyLock<Regex> = LazyLock::new(|| {
    // `(?-u:\b)` keeps Ruby's ASCII-only word boundary: with Unicode boundaries a name butting
    // against a Japanese comment would stop matching.
    Regex::new(r"(?-u:\b)(?:[A-Z][A-Za-z0-9_]*::)+[A-Za-z_][A-Za-z0-9_]*(?-u:\b)").unwrap()
});

/// RFC 2396 absolute URIs limited to `http`/`https`, as `URI::RFC2396_PARSER.make_regexp` builds
/// them for RuboCop.
///
/// Two constructs of the original have no counterpart here. The scheme is written out instead of
/// the lookahead that pins it to `http`/`https`, which comes to the same thing because the scheme
/// production stops at the `:` either way. And the `(?!//)` guarding the authority-less branch
/// becomes an optional group: the authority branch itself can match nothing, so it never fails on
/// a `//` and the negative lookahead was only ever reachable without one.
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
        r"(?:https?):(?:{uric_no_slash}{uric}*|(?:(?://(?:(?:(?:{userinfo}@)?(?:{host}(?::\d*)?))?|{reg_name}))?(?:{abs_path})?)(?:\?{uric}*)?)(?:\#{uric}*)?"
    ))
    .unwrap()
});

/// Whether `URI.parse` accepts the string, which RuboCop uses to weed out RFC 2396 matches that
/// are not URIs after all.
///
/// Ruby parses with the RFC 3986 grammar, which is stricter about brackets: `[` and `]` are legal
/// in an RFC 2396 path or fragment but only inside an RFC 3986 host, so a doc link such as
/// `{Title}[https://example.com/page]` swallows the closing bracket and stops being a URI.
fn valid_uri(text: &str) -> bool {
    RFC3986_URI.is_match(text)
}

static RFC3986_URI: LazyLock<Regex> = LazyLock::new(|| {
    const PCT: &str = r"%[0-9a-fA-F]{2}";
    let segment = format!(r"(?:{PCT}|[!$&-.0-9:;=@A-Z_a-z~/])");
    let segment_start = format!(r"(?:{PCT}|[!$&-.0-9:;=@A-Z_a-z~])");
    let userinfo = format!(r"(?:{PCT}|[!$&-.0-9:;=A-Z_a-z~])*");
    // The bracketed form has already been through RFC 2396's IPv6 grammar by the time it gets
    // here, so re-deriving RFC 3986's near-identical one would only add a wall of alternations.
    let host = format!(r"(?:\[[0-9a-fA-F:.v]+\]|(?:{PCT}|[!$&-.0-9;=A-Z_a-z~])*)");
    let authority = format!(r"(?:{userinfo}@)?{host}(?::[0-9]*)?");
    let fragment = format!(r"(?:{PCT}|[!$&-.0-9:;=@A-Z_a-z~/?])*");
    Regex::new(&format!(
        r"\A(?:[A-Za-z][+\-.0-9A-Za-z]*):(?://{authority}(?:/{segment}*)?|/(?:{segment_start}{segment}*)?|{segment_start}{segment}*|)(?:\?[^\#]*)?(?:\#{fragment})?\z"
    ))
    .unwrap()
});

fn line_break_edits(context: &RuleContext<'_>, max: usize) -> HashMap<usize, Edit> {
    let comments: HashSet<usize> = context
        .nodes_of("comment")
        .map(|node| node.start_position().row + 1)
        .collect();
    let mut edits = HashMap::new();

    // RuboCop gives a single-line block precedence over the call that owns it.
    // Breaking immediately after `{` / `do` is syntax preserving even when the
    // line has a trailing comment.
    for node in context
        .nodes_of_any(&["block", "do_block"])
        .filter(|node| node.start_position().row == node.end_position().row)
    {
        let start = node
            .child_by_field_name("parameters")
            .map_or_else(
                || node.start_byte() + if node.kind() == "block" { 1 } else { 2 },
                |parameters| parameters.end_byte(),
            )
            .min(node.end_byte());
        edits.entry(node.start_position().row + 1).or_insert(Edit {
            start,
            end: start,
            replacement: "\n".to_owned(),
            safe: true,
        });
    }

    for node in context
        .nodes_of_any(&["call", "array", "hash", "method", "singleton_method"])
        .filter(|node| breakable_collection_on_one_line(*node))
    {
        let line_number = node.start_position().row + 1;
        if edits.contains_key(&line_number) || comments.contains(&line_number) {
            continue;
        }

        let Some(mut elements) = breakable_elements(node, context) else {
            continue;
        };
        if elements.len() < 2 {
            continue;
        }

        if node.kind() == "call" && !call_parenthesized(node, context) {
            elements.remove(0);
        }
        let Some(element) = elements
            .iter()
            .position(|element| element.start_position().column > max)
            .map_or_else(
                || elements.last().copied(),
                |index| elements.get(index.saturating_sub(1)).copied(),
            )
        else {
            continue;
        };
        let start = element.start_byte();
        edits.insert(
            line_number,
            Edit {
                start,
                end: start,
                replacement: "\n".to_owned(),
                safe: true,
            },
        );
    }

    edits
}

fn breakable_collection_on_one_line(node: Node<'_>) -> bool {
    if node.kind() == "call" {
        return node
            .child_by_field_name("arguments")
            .is_some_and(|arguments| {
                node.start_position().row == arguments.start_position().row
                    && arguments.start_position().row == arguments.end_position().row
            });
    }
    node.start_position().row == node.end_position().row
}

fn breakable_elements<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Vec<Node<'tree>>> {
    let container = match node.kind() {
        "call" => node.child_by_field_name("arguments")?,
        "method" | "singleton_method" => node.child_by_field_name("parameters")?,
        "array" => node,
        "hash" if context.source.node_text(node).starts_with('{') => node,
        _ => return None,
    };
    let mut cursor = container.walk();
    Some(container.named_children(&mut cursor).collect())
}

fn call_parenthesized(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.child_by_field_name("arguments")
        .is_some_and(|arguments| context.source.node_text(arguments).starts_with('('))
}

#[cfg(test)]
mod tests {
    use super::{URI, extended_end, indentation_difference, valid_uri};

    #[test]
    fn a_yard_link_swallows_the_closing_bracket_and_stops_being_a_uri() {
        let uri = "https://guides.rubyonrails.org/action_view_overview.html#strict-locals]";
        assert_eq!(
            URI.find(uri).map(|found| found.as_str()),
            Some(uri),
            "RFC 2396 では `]` もフラグメントの一部として食う"
        );
        assert!(
            !valid_uri(uri),
            "RFC 3986 のフラグメントは `]` を許さないので URI.parse は失敗する"
        );
        assert!(valid_uri(
            "https://guides.rubyonrails.org/action_view_overview.html#strict-locals"
        ));
    }

    #[test]
    fn a_bracketed_query_stays_a_valid_uri() {
        // RFC 3986 のクエリは `#` 以外を何でも許すので、`[` があっても弾かれない。
        assert!(valid_uri("http://example.com/?x=[1]"));
    }

    #[test]
    fn the_end_of_a_match_moves_to_the_end_of_its_word() {
        let line = r#"assert_equal "http://test.host/x", url"#;
        let found = URI.find(line).unwrap();
        // 引用符で閉じられた URI は、その閉じ引用符とカンマまで 1 語として伸びる。
        assert_eq!(
            &line[..extended_end(line, found.end())],
            r#"assert_equal "http://test.host/x","#
        );
    }

    #[test]
    fn leading_tabs_count_double() {
        assert_eq!(indentation_difference("\t\tx = 1"), 2);
        assert_eq!(indentation_difference("  x = 1"), 0);
    }
}
