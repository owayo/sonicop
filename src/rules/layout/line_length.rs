use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::Interpolations;
use crate::rules::support::is_ruby_space_char;
use crate::source::is_protected;

/// `Layout/IndentationStyle`'s `IndentationWidth` is unset by default, so RuboCop falls back to
/// `Layout/IndentationWidth`'s `Width`, which is 2. A cop only ever sees its own configuration
/// here, so that fallback is spelled out: one leading tab is worth two columns, i.e. one extra
/// column per tab.
const TAB_INDENTATION_WIDTH: usize = 2;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(120);
    // `AllowHeredoc` is either a boolean or a list of permitted delimiters. Deserializing the
    // list as `bool` used to fall back to `true` and exempt every heredoc in the file.
    let allow_heredoc: Option<bool> = context.setting("AllowHeredoc");
    let allowed_heredocs: Option<Vec<String>> = context.setting("AllowHeredoc");
    let allow_all_heredocs = allow_heredoc.unwrap_or(allowed_heredocs.is_none());
    let allowed_heredoc_lines = allowed_heredocs
        .as_deref()
        .map(|names| heredoc_lines(context, names))
        .unwrap_or_default();
    let rbs_lines = match context
        .setting::<bool>("AllowRBSInlineAnnotation")
        .unwrap_or(false)
    {
        true => rbs_annotation_lines(context),
        false => HashSet::new(),
    };
    // RuboCop drops the `__END__` line and the data section behind it from the lines it walks, so
    // a long line down there is not a long line of code.
    let data_line = context
        .nodes_of("uninterpreted")
        .next()
        .map_or(usize::MAX, |node| node.start_position().row + 1);
    let last_line = context.source.line_count().min(data_line.saturating_sub(1));

    // Building autocorrections walks a large part of the AST, and every break it finds is filed
    // under a line. Only the lines that are over the limit are ever looked up again, so the lines
    // are settled first and the walk below is held to them. Most files have none at all.
    let candidate_lines: HashSet<usize> = (1..=last_line)
        .filter(|line_number| {
            let raw = context.source.line(*line_number);
            let line = crate::rules::support::chomp(raw);
            let length = line.chars().count() + indentation_difference(line);
            length > max
                && !(*line_number == 1 && line.starts_with("#!"))
                && !rbs_lines.contains(line_number)
                && !(context.in_heredoc(
                    context.source.line_start(*line_number)
                        ..context.source.line_start(*line_number) + line.len(),
                ) && (allow_all_heredocs || allowed_heredoc_lines.contains(line_number)))
        })
        .collect();
    if candidate_lines.is_empty() {
        return;
    }

    let allow_uri: bool = context.setting("AllowURI").unwrap_or(true);
    let allow_qualified_name: bool = context.setting("AllowQualifiedName").unwrap_or(true);
    // `allowed_line?`: a line matching one of these is never long, whatever it holds.
    let allowed_patterns =
        crate::rules::naming::support::forbidden_patterns_named(context, "AllowedPatterns");
    let uri_pattern = match context.setting::<Vec<String>>("URISchemes") {
        Some(schemes) if !schemes.is_empty() => uri_regex(&schemes),
        _ => &*URI,
    };

    let allow_directives = allow_cop_directives(context);
    let break_edits = line_break_edits(context, max, &candidate_lines);
    let directive_lines = directive_lines(context);
    let endless_method_lines = endless_method_lines(context);

    for line_number in 1..=last_line {
        let raw = context.source.line(line_number);
        // **`String#chomp` takes off `\r\n` as one line ending, and a lone `\r` as well.** Counting
        // the `\r` made every 120-column line of a CRLF file one column too long, and the offense
        // that followed pulled `Style/BlockDelimiters` in behind it: `-A` rewrote `{ }` to
        // `do end` in files upstream left untouched. A CRLF file is not a file of longer lines.
        let line = crate::rules::support::chomp(raw);
        let indent = indentation_difference(line);
        let length = line.chars().count() + indent;
        let line_start = context.source.line_start(line_number);

        if length <= max
            || (line_number == 1 && line.starts_with("#!"))
            || allowed_patterns
                .iter()
                .any(|pattern| pattern.is_match(line))
            || rbs_lines.contains(&line_number)
            || (context.in_heredoc(line_start..line_start + line.len())
                && (allow_all_heredocs || allowed_heredoc_lines.contains(&line_number)))
        {
            continue;
        }

        // An endless method has a way out of being long -- it can be rewritten as a regular
        // method -- so RuboCop reports it before any exemption gets a say, and reports the whole
        // line even when a cop directive is what pushed it over.
        let (start_column, reported) = if let Some(node) = endless_method_lines.get(&line_number) {
            let range = line_start + byte_offset(line, max.saturating_sub(indent))
                ..line_start + byte_offset(line, length);
            offenses.push(
                context
                    .offense(format!("Line is too long. [{length}/{max}]"), range)
                    .corrected_by(endless_method_edit(context, *node)),
            );
            continue;
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
                .then(|| excessive_range(line, MatchKind::Uri, max, indent, uri_pattern))
                .flatten();
            let name = allow_qualified_name
                .then(|| excessive_range(line, MatchKind::QualifiedName, max, indent, uri_pattern))
                .flatten();
            if exempted(uri, name, length, max) {
                continue;
            }
            (excessive_position(uri.or(name), max, indent), length)
        } else {
            (max.saturating_sub(indent), length)
        };

        let offense = context
            .offense(
                format!("Line is too long. [{reported}/{max}]"),
                line_start + byte_offset(line, start_column)
                    ..line_start + byte_offset(line, reported),
            )
            .with_length(reported.saturating_sub(start_column));
        offenses.push(match break_edits.get(&line_number) {
            Some((edit, anchor)) => offense
                .corrected_by(edit.clone())
                .corrections_anchored_at(anchor.clone()),
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

/// Lines that belong to a heredoc whose delimiter appears in the list form of `AllowHeredoc`.
///
/// Heredocs opened on one statement are consumed in order, while one opened inside an
/// interpolation temporarily suspends the body around it. The grammar keeps the opening nodes
/// even for that nested shape but cannot attach all of their bodies correctly, so deriving this
/// small state machine from the opening lines is more faithful than zipping AST body nodes.
fn heredoc_lines(context: &RuleContext<'_>, allowed: &[String]) -> HashSet<usize> {
    let mut openings: HashMap<usize, Vec<String>> = HashMap::new();
    for node in context.nodes_of("heredoc_beginning") {
        let Some(delimiter) = heredoc_delimiter(context.source.node_text(node)) else {
            continue;
        };
        openings
            .entry(node.start_position().row + 1)
            .or_default()
            .push(delimiter);
    }

    let mut result = HashSet::new();
    let mut pending = VecDeque::new();
    let mut suspended = Vec::new();
    let mut current: Option<String> = None;
    for line_number in 1..=context.source.line_count() {
        let line = crate::rules::support::chomp(context.source.line(line_number));
        let terminates = current
            .as_deref()
            .is_some_and(|delimiter| line.trim() == delimiter);
        if !terminates
            && current
                .iter()
                .chain(suspended.iter())
                .any(|delimiter| allowed.iter().any(|name| name == delimiter))
        {
            result.insert(line_number);
        }

        if terminates {
            current = suspended.pop().or_else(|| pending.pop_front());
        }
        if let Some(mut on_line) = openings.remove(&line_number) {
            match current.take() {
                Some(parent) => {
                    suspended.push(parent);
                    current = Some(on_line.remove(0));
                    for delimiter in on_line.into_iter().rev() {
                        pending.push_front(delimiter);
                    }
                }
                None => {
                    pending.extend(on_line);
                    current = pending.pop_front();
                }
            }
        }
    }
    result
}

fn heredoc_delimiter(opening: &str) -> Option<String> {
    let rest = opening.strip_prefix("<<")?;
    let rest = rest
        .strip_prefix('-')
        .or_else(|| rest.strip_prefix('~'))
        .unwrap_or(rest)
        .trim();
    let unquoted = match rest.as_bytes().first() {
        Some(quote @ (b'\'' | b'"' | b'`')) if rest.as_bytes().last() == Some(quote) => {
            &rest[1..rest.len().saturating_sub(1)]
        }
        _ => rest,
    };
    (!unquoted.is_empty()).then(|| unquoted.to_owned())
}

fn rbs_annotation_lines(context: &RuleContext<'_>) -> HashSet<usize> {
    context
        .comment_ranges()
        .iter()
        .filter(|range| context.source.slice((*range).clone()).starts_with("#:"))
        .map(|range| context.source.line_column(range.start).0)
        .collect()
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
fn endless_method_lines<'tree>(context: &'tree RuleContext<'_>) -> HashMap<usize, Node<'tree>> {
    context
        .nodes_of_any(&["method", "singleton_method"])
        .filter(|node| is_endless_method(*node))
        .map(|node| (node.start_position().row + 1, node))
        .collect()
}

/// RuboCop's `correct_to_multiline`: an endless method is rewritten as a regular one, which is the
/// correction that makes every over-long endless method line correctable.
fn endless_method_edit(context: &RuleContext<'_>, node: Node<'_>) -> Edit {
    let indent = " ".repeat(context.source.line_column(node.start_byte()).1 - 1);
    let mut signature = String::from("def ");
    if let Some(object) = node.field("object") {
        signature.push_str(context.source.node_text(object));
        // The separator is whatever sits between the receiver and the name: `.` or `::`.
        signature.push_str(
            context
                .source
                .slice(object.end_byte()..name_start(node, object)),
        );
    }
    if let Some(name) = node.field("name") {
        signature.push_str(context.source.node_text(name));
    }
    if let Some(parameters) = node.field("parameters") {
        signature.push_str(context.source.node_text(parameters));
    }
    let body = node
        .field("body")
        .map_or("", |body| context.source.node_text(body));
    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: format!("{signature}\n{indent}  {body}\n{indent}end"),
        safe: true,
    }
}

fn name_start(node: Node<'_>, object: Node<'_>) -> usize {
    node.field("name")
        .map_or(object.end_byte(), |name| name.start_byte())
}

fn is_endless_method(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    !node
        .children(&mut cursor)
        .any(|child| child.kind_str() == "end")
}

fn marker_only(prefix: &str) -> bool {
    prefix
        .strip_prefix('#')
        .is_some_and(|rest| rest.chars().all(is_ruby_space_char))
}

fn length_without_directive(line: &str) -> usize {
    DIRECTIVE
        .find(line)
        .map_or(line, |found| &line[..found.start()])
        .trim_end_matches(is_ruby_space_char)
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
    uri: &Regex,
) -> Option<(usize, usize)> {
    let found = match kind {
        // RuboCop drops matches that `URI.parse` rejects, but the scan still consumed them, so
        // filtering after the scan -- not before -- keeps the remaining matches where they were.
        MatchKind::Uri => uri
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
    if rest.starts_with(|character| !is_ruby_space_char(character)) {
        end += rest.find(is_ruby_space_char).unwrap_or(rest.len());
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
    Regex::new(r"#(?-u:\s)*rubocop(?-u:\s)*:(?-u:\s)*(?:disable-next|todo-next|disable|enable|todo|push|pop)(?-u:\b)")
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
/// `URISchemes`: the schemes `URI::DEFAULT_PARSER.make_regexp` is built from. Hard-coding
/// `https?` made a line holding any other scheme -- the config exists to name one -- too long.
/// The pattern is built from the configuration, so it cannot live in a `LazyLock` -- but the
/// configuration does not change between files, and `URISchemes` is set in the default
/// configuration, so this was compiling the RFC 2396 grammar once per file. It was the single
/// largest cop cost in a run over RuboCop's own tree.
fn uri_regex(schemes: &[String]) -> &'static Regex {
    let escaped: Vec<String> = schemes.iter().map(|scheme| regex::escape(scheme)).collect();
    crate::rules::regex_cache::compiled(&uri_regex_pattern(&escaped.join("|")))
        .unwrap_or_else(|| &URI)
}

static URI: LazyLock<Regex> = LazyLock::new(|| build_uri_regex("https?"));

fn build_uri_regex(schemes: &str) -> Regex {
    Regex::new(&uri_regex_pattern(schemes)).unwrap()
}

fn uri_regex_pattern(schemes: &str) -> String {
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
    format!(
        r"(?:{schemes}):(?:{uric_no_slash}{uric}*|(?:(?://(?:(?:(?:{userinfo}@)?(?:{host}(?::(?-u:\d)*)?))?|{reg_name}))?(?:{abs_path})?)(?:\?{uric}*)?)(?:\#{uric}*)?"
    )
}

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

/// Node kinds whose RuboCop counterpart `Layout/LineLength` offers a line break inside.
///
/// `element_reference` and an `assignment` onto one are `send` nodes upstream (`[]` and `[]=`),
/// and the three array spellings plus a multiple assignment's right-hand side are all `array`.
const BREAKABLE_KINDS: &[&str] = &[
    "block",
    "do_block",
    "call",
    "element_reference",
    "assignment",
    "array",
    "string_array",
    "symbol_array",
    "right_assignment_list",
    "exceptions",
    "hash",
    "method",
    "singleton_method",
];

/// Nodes the grammar writes body-first but upstream's parser stores condition-first, so its walk
/// reaches the condition before the body however the source reads.
const MODIFIER_KINDS: &[&str] = &[
    "if_modifier",
    "unless_modifier",
    "while_modifier",
    "until_modifier",
];

/// The order upstream's walk reaches each node that can hold a break, numbered from the top.
///
/// Which of two candidates on a line wins depends on the order they are reached in, and the two
/// trees do not agree on it: a modifier keeps its condition after its body here, and a block hangs
/// off the end of the call it belongs to rather than wrapping it. Both are re-sorted here.
fn upstream_order(root: Node<'_>) -> HashMap<usize, u32> {
    let mut order = HashMap::new();
    let mut stack = vec![root];
    let mut index = 0;
    let mut children = Vec::new();
    while let Some(node) = stack.pop() {
        if matches!(node.kind_str(), "lambda") || BREAKABLE_KINDS.contains(&node.kind_str()) {
            order.insert(node.id(), index);
        }
        index += 1;
        children.clear();
        let mut cursor = node.walk();
        children.extend(node.named_children(&mut cursor));
        if MODIFIER_KINDS.contains(&node.kind_str()) {
            children.reverse();
        }
        stack.extend(children.iter().rev().copied());
    }
    order
}

/// Where a candidate sorts, with a block placed just ahead of the call it belongs to: upstream's
/// block node stands where the grammar puts the call, and the call is its first child.
fn visit_order(node: Node<'_>, order: &HashMap<usize, u32>) -> (u32, u8) {
    if matches!(node.kind_str(), "block" | "do_block") {
        if let Some(parent) = node.parent() {
            if let Some(index) = order.get(&parent.id()) {
                return (*index, 0);
            }
        }
    }
    (order.get(&node.id()).copied().unwrap_or(u32::MAX), 1)
}

/// Where `Layout/LineLength` would insert a line break on each line, keyed by line number. A line
/// with no entry has no correction, which is what makes its offense uncorrectable.
///
/// RuboCop builds one table for the whole file, and the order it is filled in decides the ties:
/// semicolons first, then a single walk of the AST in which a single-line block overwrites whatever
/// its line already held while every other node only fills a line that is still empty.
fn line_break_edits(
    context: &RuleContext<'_>,
    max: usize,
    lines: &HashSet<usize>,
) -> HashMap<usize, (Edit, std::ops::Range<usize>)> {
    let breaker = Breaker {
        context,
        max,
        comment_lines: comment_lines(context),
        heredocs: context
            .nodes_of("heredoc_beginning")
            .map(|node| node.byte_range())
            .collect(),
    };
    // The range each break hangs off, which is not the range the offense is reported on: upstream
    // calls `insert_before(breakable_range, ...)` with the element it would break in front of, and
    // that range is what orders this insertion against another cop's at the same offset.
    // The insertion is `"\n"` for everything but a split string, where upstream writes
    // `delimiter + " \\\n" + delimiter` so the two halves stay one literal.
    let mut positions: HashMap<usize, (std::ops::Range<usize>, String)> = HashMap::new();

    // Reversed, so that the first semicolon on a line is the one whose position survives.
    if context.source.text().contains(';') {
        for offset in semicolon_break_positions(context).into_iter().rev() {
            let line = context.source.line_column(offset).0;
            if lines.contains(&line) {
                positions.insert(line, (offset..(offset + 1), "\n".to_owned()));
            }
        }
    }

    // `check_for_breakable_str` / `check_for_breakable_dstr` run on `on_str` / `on_dstr`, which
    // come before the walk below fills the same lines -- a string that can be split wins over the
    // element a break would otherwise go in front of.
    if context.setting::<bool>("SplitStrings").unwrap_or(false) {
        for node in context.nodes_of_any(&["string", "bare_string"]) {
            if !lines.contains(&(node.start_position().row + 1)) {
                continue;
            }
            let Some((offset, delimiter)) = breakable_string(context, node, max) else {
                continue;
            };
            let anchor_end = offset
                + context.source.text()[offset..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(0);
            positions
                .entry(node.start_position().row + 1)
                .or_insert_with(|| (offset..anchor_end, format!("{delimiter} \\\n{delimiter}")));
        }
    }

    // A break is filed under the line of a node the break goes in front of, which lies inside
    // `node` -- except for a block, whose break is filed under the line the call it hangs off
    // begins on, and that call can start earlier. A node spanning no over-long line has nothing to
    // contribute, and finding that out costs two integers rather than a search for the element.
    let mut candidates: Vec<Node<'_>> = context
        .nodes_of_any(BREAKABLE_KINDS)
        .filter(|node| {
            let first = match matches!(node.kind_str(), "block" | "do_block") {
                true => node
                    .parent_of(context)
                    .unwrap_or(*node)
                    .start_position()
                    .row,
                false => node.start_position().row,
            };
            (first + 1..=node.end_position().row + 1).any(|line| lines.contains(&line))
        })
        .collect();
    if candidates.is_empty() {
        return finish(positions);
    }
    let order = upstream_order(context.root_node());
    candidates.sort_by_key(|node| visit_order(*node, &order));

    for node in candidates {
        if matches!(node.kind_str(), "block" | "do_block") {
            // Upstream's block node starts at the receiver, not at the brace, so a call split
            // over two lines files its break under the line the receiver is on.
            let owner = node.parent_of(context).unwrap_or(node);
            let line = owner.start_position().row + 1;
            if lines.contains(&line)
                && let Some(offset) = breaker.block_break_position(node)
            {
                positions.insert(line, (offset..(offset + 1), "\n".to_owned()));
            }
        } else if let Some(element) = breaker.breakable_element(node) {
            let line = element.start_position().row + 1;
            if lines.contains(&line) {
                positions
                    .entry(line)
                    .or_insert_with(|| (element.byte_range(), "\n".to_owned()));
            }
        }
    }

    finish(positions)
}

/// Turns each recorded break into the insertion it stands for.
fn finish(
    positions: HashMap<usize, (std::ops::Range<usize>, String)>,
) -> HashMap<usize, (Edit, std::ops::Range<usize>)> {
    positions
        .into_iter()
        .map(|(line, (anchor, replacement))| {
            (
                line,
                (
                    Edit {
                        start: anchor.start,
                        end: anchor.start,
                        replacement,
                        safe: true,
                    },
                    anchor,
                ),
            )
        })
        .collect()
}

/// Every line a comment sits on. A node whose first line carries one is never broken, because the
/// break would push the rest of the line behind the comment marker.
fn comment_lines(context: &RuleContext<'_>) -> HashSet<usize> {
    let mut lines = HashSet::new();
    for range in context.comment_ranges() {
        let first = context.source.line_column(range.start).0;
        let last = context.source.line_column(range.end.saturating_sub(1)).0;
        lines.extend(first..=last);
    }
    lines
}

/// The offset just after each semicolon that has something else behind it on its line.
fn semicolon_break_positions(context: &RuleContext<'_>) -> Vec<usize> {
    let ranges = context.protected_ranges();
    // `$;`, `?;` and `:";"` spell a semicolon inside a single token, which is not a `tSEMI`.
    let tokens: Vec<std::ops::Range<usize>> = context
        .nodes_of_any(&["global_variable", "character", "delimited_symbol"])
        .map(|node| node.byte_range())
        .collect();
    let interpolations = Interpolations::new(context);
    let text = context.source.text();
    text.bytes()
        .enumerate()
        .filter(|(offset, byte)| {
            *byte == b';'
                && (!is_protected(*offset, ranges) || interpolations.holds_code(*offset))
                && !tokens.iter().any(|token| token.contains(offset))
        })
        .filter_map(|(offset, _)| match text.as_bytes().get(offset + 1) {
            Some(b'\n' | b'\r' | b';') | None => None,
            Some(_) => Some(offset + 1),
        })
        .collect()
}

/// RuboCop's `CheckLineBreakable`, which decides where a too-long line could be split.
struct Breaker<'a, 'b> {
    context: &'a RuleContext<'b>,
    max: usize,
    comment_lines: HashSet<usize>,
    /// The `<<~FOO` openers of the file, which several of the rules below have to steer around.
    heredocs: Vec<std::ops::Range<usize>>,
}

impl Breaker<'_, '_> {
    /// `check_for_breakable_block`: a single-line block breaks right after what opens its body.
    fn block_break_position(&self, node: Node<'_>) -> Option<usize> {
        if node.start_position().row != node.end_position().row
            || self.receiver_contains_heredoc(node)
        {
            return None;
        }
        // With block arguments the break goes after the closing `|`. A lambda is exempt -- both
        // `->` and a call to `lambda` count as one upstream -- and falls through to the brace.
        let opener = if node.kind_str() == "block" { 1 } else { 2 };
        let parameters = if is_lambda_block(node, self.context) {
            None
        } else {
            node.field("parameters")
        };
        let position = match parameters {
            Some(parameters) => parameters.end_byte(),
            None => node.start_byte() + opener,
        };
        Some(position.min(node.end_byte()))
    }

    /// `extract_breakable_node`: the element a break would be inserted before, if any.
    fn breakable_element<'t>(&self, node: Node<'t>) -> Option<Node<'t>> {
        if node.kind_str() == "call" && self.chained_to_heredoc(node) {
            return None;
        }
        if matches!(node.kind_str(), "method" | "singleton_method") && is_endless_method(node) {
            return None;
        }
        let elements = self.elements(node)?;
        if elements.len() < 2 {
            return None;
        }
        let line = node.start_position().row + 1;
        if line_char_count(self.context, line) <= self.max
            || self.comment_lines.contains(&line)
            || self.safe_to_ignore(node, &elements)
        {
            return None;
        }
        self.first_element_over_column_limit(node, elements)
            .map(|element| element.node)
    }

    /// The children RuboCop counts when it asks whether a node is a collection worth breaking.
    ///
    /// A brace-less hash argument is already spelled as loose `pair`s by the grammar, which is what
    /// upstream's `process_args` reaches by unfolding the `hash` node its parser builds.
    fn elements<'t>(&self, node: Node<'t>) -> Option<Vec<Element<'t>>> {
        let container = match node.kind_str() {
            // `super(...)` is its own node type upstream rather than a `send`, so no cop callback
            // reaches it and it is never broken.
            "call" if is_super_call(node) => return None,
            "call" => node.field("arguments")?,
            "method" | "singleton_method" => node.field("parameters")?,
            // A `rescue` clause's exception list reaches RuboCop as an `array` too.
            "array" | "string_array" | "symbol_array" | "right_assignment_list" | "exceptions" => {
                node
            }
            // A kwargs hash has no braces and is never broken, only a literal one is.
            "hash" if self.context.source.node_text(node).starts_with('{') => node,
            "element_reference" => return Some(group_pairs(index_arguments(node), true)),
            // `a[b] = c` is the `[]=` call whose arguments are the subscripts and the value.
            "assignment" => {
                let left = node.field("left")?;
                if left.kind_str() != "element_reference" {
                    return None;
                }
                let mut children = index_arguments(left);
                children.push(node.field("right")?);
                return Some(group_pairs(children, true));
            }
            _ => return None,
        };
        let mut cursor = container.walk();
        // A comment is a node of the tree but not of RuboCop's AST, and a heredoc's body hangs off
        // the argument list it was opened in rather than off the opener, so neither is an element.
        let children: Vec<Node<'t>> = container
            .named_children(&mut cursor)
            .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
            .collect();
        // A literal hash's own pairs are its elements; only an argument list and an array literal
        // carry a brace-less hash that upstream's parser folds into one node. `process_args` then
        // unfolds it again -- but for a call's last argument only, so a hash followed by a block
        // argument, or one inside an array, stays a single element.
        let inside_array = matches!(
            container.kind_str(),
            "array" | "string_array" | "symbol_array" | "right_assignment_list"
        );
        Some(group_pairs(children, !inside_array))
    }

    fn safe_to_ignore(&self, node: Node<'_>, elements: &[Element<'_>]) -> bool {
        self.already_on_multiple_lines(node, elements)
            || self.contained_by_breakable_collection_on_same_line(node)
            || self.contained_by_multiline_collection_that_could_be_broken_up(node)
    }

    /// A node already spread over several lines has been broken enough.
    ///
    /// A method definition is measured by its parameter list rather than by the whole definition,
    /// and a call by its arguments: the block it may carry is a separate node upstream.
    fn already_on_multiple_lines(&self, node: Node<'_>, elements: &[Element<'_>]) -> bool {
        let last_row = match node.kind_str() {
            "method" | "singleton_method" => match elements.last() {
                Some(last) => last.last_row,
                None => return false,
            },
            "call" => node
                .field("arguments")
                .map_or(node.end_position().row, |arguments| {
                    arguments.end_position().row
                }),
            _ => node.end_position().row,
        };
        node.start_position().row != last_row
    }

    /// A collection nested in another one that starts on the same line waits its turn: upstream
    /// only ever marks one break per line, and the outer one is broken first.
    fn contained_by_breakable_collection_on_same_line(&self, node: Node<'_>) -> bool {
        let row = node.start_position().row;
        for ancestor in Ancestors::of(node) {
            if ancestor.start_position().row != row {
                return false;
            }
            if self
                .ancestor_elements(ancestor)
                .is_some_and(|elements| elements.len() >= 2)
            {
                return true;
            }
        }
        false
    }

    /// The nearest enclosing collection, wherever it starts, gets asked whether its own children
    /// still have room to be spread out; if they do, breaking this one would be redundant.
    fn contained_by_multiline_collection_that_could_be_broken_up(&self, node: Node<'_>) -> bool {
        for ancestor in Ancestors::of(node) {
            if let Some(elements) = self.ancestor_elements(ancestor) {
                if elements.len() >= 2 {
                    return children_could_be_broken_up(&elements);
                }
            }
        }
        false
    }

    /// The elements of an enclosing node, which upstream only asks of a `hash`, an `array` or a
    /// `send` -- never of a method definition, whose parameters are not the caller's to break.
    fn ancestor_elements<'t>(&self, node: Node<'t>) -> Option<Vec<Element<'t>>> {
        if matches!(node.kind_str(), "method" | "singleton_method") {
            return None;
        }
        self.elements(node)
    }

    /// `extract_first_element_over_column_limit`.
    fn first_element_over_column_limit<'t>(
        &self,
        node: Node<'t>,
        mut elements: Vec<Element<'t>>,
    ) -> Option<Element<'t>> {
        // Moving the first argument of a call written without parentheses would change what the
        // line means, so it is left where it is.
        if is_call_like(node)
            && !call_parenthesized(node, self.context)
            && !elements
                .first()
                .is_some_and(|first| self.is_heredoc(first.node))
        {
            elements.remove(0);
        }

        let row = node.start_position().row;
        let mut index = 0;
        while elements.get(index).is_some_and(|element| {
            self.char_column(element.node) <= self.max && element.node.start_position().row == row
        }) {
            index += 1;
        }
        index = self.shift_elements_for_heredoc_arg(node, &elements, index)?;
        if index == 0 {
            return elements.first().copied();
        }
        elements.get(index - 1).copied()
    }

    /// Breaking after a heredoc argument would leave the body stranded, so the break moves to just
    /// after it -- and a heredoc in first position rules the line out entirely.
    fn shift_elements_for_heredoc_arg(
        &self,
        node: Node<'_>,
        elements: &[Element<'_>],
        index: usize,
    ) -> Option<usize> {
        if !matches!(
            node.kind_str(),
            "call"
                | "array"
                | "string_array"
                | "symbol_array"
                | "right_assignment_list"
                | "exceptions"
        ) {
            return Some(index);
        }
        let Some(heredoc) = elements
            .iter()
            .position(|element| self.is_heredoc(element.node))
        else {
            return Some(index);
        };
        if heredoc == 0 {
            return None;
        }
        Some(if heredoc >= index { index } else { heredoc + 1 })
    }

    fn char_column(&self, node: Node<'_>) -> usize {
        self.context.source.line_column(node.start_byte()).1 - 1
    }

    fn is_heredoc(&self, node: Node<'_>) -> bool {
        node.kind_str() == "heredoc_beginning"
    }

    /// Whether a heredoc opens anywhere inside `node`.
    fn contains_heredoc(&self, node: Node<'_>) -> bool {
        let range = node.byte_range();
        self.heredocs
            .iter()
            .any(|heredoc| heredoc.start >= range.start && heredoc.end <= range.end)
    }

    fn receiver_contains_heredoc(&self, node: Node<'_>) -> bool {
        node.parent_of(self.context)
            .and_then(|parent| parent.field("receiver"))
            .is_some_and(|receiver| self.contains_heredoc(receiver))
    }

    /// A call whose receiver chain starts from a heredoc cannot take a break in its arguments.
    fn chained_to_heredoc(&self, node: Node<'_>) -> bool {
        let mut receiver = node.field("receiver");
        while let Some(current) = receiver {
            if self.is_heredoc(current) {
                return true;
            }
            receiver = current.field("receiver");
        }
        false
    }
}

/// The enclosing nodes of `node` as RuboCop sees them.
///
/// The grammar hangs a block off the call it belongs to, while upstream's parser hangs the call off
/// the block. So walking out of a block body must not pass through the call that owns it: the block
/// is not an argument of that call, and treating it as one would let an unrelated argument list
/// decide whether the body can be broken.
struct Ancestors<'t> {
    current: Node<'t>,
}

impl<'t> Ancestors<'t> {
    fn of(node: Node<'t>) -> Self {
        Self { current: node }
    }
}

impl<'t> Iterator for Ancestors<'t> {
    type Item = Node<'t>;

    fn next(&mut self) -> Option<Node<'t>> {
        loop {
            let parent = self.current.parent()?;
            let through_block = parent.kind_str() == "call"
                && parent
                    .field("block")
                    .is_some_and(|block| block.id() == self.current.id());
            self.current = parent;
            if !through_block {
                return Some(parent);
            }
        }
    }
}

/// The subscripts of `a[b, c]`, which are every child but the object being indexed.
fn index_arguments<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let object = node.field("object");
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| {
            !matches!(child.kind_str(), "comment" | "heredoc_body")
                && Some(child.id()) != object.map(|object| object.id())
        })
        .collect()
}

/// Whether the elements of a collection are already spread over lines with room left to spread
/// further: two of them sharing a line is what makes another pass worthwhile.
fn children_could_be_broken_up(elements: &[Element<'_>]) -> bool {
    let (Some(first), Some(last)) = (elements.first(), elements.last()) else {
        return false;
    };
    if first.node.start_position().row == last.last_row {
        return false;
    }
    let mut last_seen: isize = -1;
    for element in elements {
        if last_seen >= element.node.start_position().row as isize {
            return true;
        }
        last_seen = element.last_row as isize;
    }
    false
}

/// One element of a collection as RuboCop's parser groups them.
///
/// `node` is where a break would go, which for a run of loose `pair`s standing in for a brace-less
/// hash is the first of them; `last_row` covers the whole run.
#[derive(Clone, Copy)]
struct Element<'t> {
    node: Node<'t>,
    last_row: usize,
}

/// Folds the trailing key/value arguments into the single `hash` node upstream's parser builds.
///
/// `expand` keeps them apart instead, which is what `process_args` does to the last argument of a
/// call -- but only there: an array literal, or a call whose hash is followed by a block argument,
/// keeps the hash whole.
fn group_pairs<'t>(children: Vec<Node<'t>>, expand: bool) -> Vec<Element<'t>> {
    let is_entry = |node: &Node<'t>| matches!(node.kind_str(), "pair" | "hash_splat_argument");
    let plain = |node: Node<'t>| Element {
        node,
        last_row: node.end_position().row,
    };
    let Some(end) = children.iter().rposition(is_entry) else {
        return children.into_iter().map(plain).collect();
    };
    if expand && end + 1 == children.len() {
        return children.into_iter().map(plain).collect();
    }
    let mut start = end;
    while start > 0 && is_entry(&children[start - 1]) {
        start -= 1;
    }
    let mut elements: Vec<Element<'t>> = children[..start].iter().copied().map(plain).collect();
    elements.push(Element {
        node: children[start],
        last_row: children[end].end_position().row,
    });
    elements.extend(children[end + 1..].iter().copied().map(plain));
    elements
}

/// Whether the block belongs to a lambda: `-> {}` and `lambda {}` are the same node upstream.
fn is_lambda_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.parent_of(context) {
        Some(parent) if parent.kind_str() == "lambda" => true,
        Some(parent) => parent
            .field("method")
            .is_some_and(|method| context.source.node_text(method) == "lambda"),
        None => false,
    }
}

/// The node kinds that reach RuboCop as a `send`, where an unparenthesized first argument is
/// pinned in place.
fn is_call_like(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "call" | "element_reference" | "assignment")
}

fn is_super_call(node: Node<'_>) -> bool {
    node.field("method")
        .is_some_and(|method| method.kind_str() == "super")
}

fn line_char_count(context: &RuleContext<'_>, line: usize) -> usize {
    let raw = context.source.line(line);
    crate::rules::support::chomp(raw).chars().count()
}

fn call_parenthesized(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("arguments")
        .is_some_and(|arguments| context.source.node_text(arguments).starts_with('('))
}

/// `check_for_breakable_str` and its helpers: where a too-long string literal may be split, and
/// which quote the two halves close and reopen with.
///
/// The break is put where the text can be cut without bisecting an escape: at the last blank that
/// still fits, else in front of the last escape, else at the width `Max` leaves once the closing
/// quote and the ` \` continuation are taken off.
fn breakable_string(
    context: &RuleContext<'_>,
    node: Node<'_>,
    max: usize,
) -> Option<(usize, &'static str)> {
    // `breakable_string?`: a heredoc has no quotes to reopen, and a string standing as a hash value
    // or an array element is left alone for now upstream too.
    if node.start_position().row != node.end_position().row {
        return None;
    }
    let parent = node.parent_of(context)?;
    if matches!(parent.kind_str(), "pair" | "keyword_parameter" | "array") {
        return None;
    }
    let source = context.source.node_text(node);
    let delimiter = match source.as_bytes().first() {
        Some(b'\'') => "'",
        Some(b'"') => "\"",
        _ => return None,
    };
    // `check_for_breakable_dstr` handles an interpolated literal separately, breaking only in front
    // of a `#{`. Cutting one by width instead splits the marker itself -- `"…#" \ "{bbbb}"`.
    if crate::rules::send_node::has_interpolation(node) {
        return breakable_dstr(context, node, max, delimiter)
            .or_else(|| breakable_string_part(context, node, max, delimiter));
    }
    // `return if source_range.last_column < max`.
    let last_column = context
        .source
        .line_column(node.end_byte().saturating_sub(1))
        .1;
    if last_column < max {
        return None;
    }
    // `largest_possible_string`: `Max` less the closing quote and the ` \`, then less wherever the
    // literal starts -- on its parent's line that is the offset between them, otherwise its indent.
    let column = context.source.line_column(node.start_byte()).1 - 1;
    let offset = match parent.start_position().row == node.start_position().row {
        true => column.saturating_sub(context.source.line_column(parent.start_byte()).1 - 1),
        false => column,
    };
    let limit = max.saturating_sub(3).saturating_sub(offset);
    let candidate: String = source.chars().take(limit).collect();
    let cut = if let Some(blank) = candidate.rfind(char::is_whitespace) {
        blank + 1
    } else if let Some(escape) = trailing_escape(&candidate) {
        escape
    } else {
        // `source_range.adjust(end_pos: max - last_column - 3)` measured from the literal's end.
        let adjustment = (max as isize) - (last_column as isize) - 3;
        let size = source.chars().count() as isize;
        if adjustment.abs() > size {
            return None;
        }
        byte_offset(source, (size + adjustment) as usize)
    };
    let position = node.start_byte() + cut;
    // `pos.end_pos unless pos.end_pos == source_range.begin_pos`.
    (position != node.start_byte() && position < node.end_byte()).then_some((position, delimiter))
}

/// `/\\(u[\da-f]{0,4}|x[\da-f]{0,2})?\z/`: an escape the cut would otherwise land inside.
fn trailing_escape(candidate: &str) -> Option<usize> {
    let bytes = candidate.as_bytes();
    let mut index = bytes.len();
    while index > 0 {
        let start = index - 1;
        if bytes[start] != b'\\' {
            // Only a short run of hex digits can follow the backslash.
            if candidate.len() - start > 5 || !bytes[start].is_ascii_alphanumeric() {
                return None;
            }
            index = start;
            continue;
        }
        return Some(start);
    }
    None
}

/// `check_for_breakable_dstr` with `breakable_dstr_begin_position`: an interpolated literal breaks
/// in front of the first `#{` that **starts before `Max` and ends past it**, and nowhere else.
fn breakable_dstr(
    context: &RuleContext<'_>,
    node: Node<'_>,
    max: usize,
    delimiter: &'static str,
) -> Option<(usize, &'static str)> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind_str() != "interpolation" {
            continue;
        }
        let start = context.source.line_column(child.start_byte()).1 - 1;
        let end = context
            .source
            .line_column(child.end_byte().saturating_sub(1))
            .1;
        if start < max && end >= max {
            return Some((child.start_byte(), delimiter));
        }
    }
    None
}

/// `check_for_breakable_str` reaching the `str` parts **inside** a `dstr`: when no `#{` straddles
/// `Max`, upstream still cuts the run of plain text by width, measuring the offset from the `dstr`
/// that holds it rather than from the start of the line.
fn breakable_string_part(
    context: &RuleContext<'_>,
    node: Node<'_>,
    max: usize,
    delimiter: &'static str,
) -> Option<(usize, &'static str)> {
    let parent_column = context.source.line_column(node.start_byte()).1 - 1;
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        if part.kind_str() != "string_content" {
            continue;
        }
        let last_column = context
            .source
            .line_column(part.end_byte().saturating_sub(1))
            .1;
        if last_column < max {
            continue;
        }
        let column = context.source.line_column(part.start_byte()).1 - 1;
        let limit = max
            .saturating_sub(3)
            .saturating_sub(column.saturating_sub(parent_column));
        let source = context.source.node_text(part);
        let candidate: String = source.chars().take(limit).collect();
        let cut = if let Some(blank) = candidate.rfind(char::is_whitespace) {
            blank + 1
        } else if let Some(escape) = trailing_escape(&candidate) {
            escape
        } else {
            let adjustment = (max as isize) - (last_column as isize) - 3;
            let size = source.chars().count() as isize;
            if adjustment.abs() > size {
                continue;
            }
            byte_offset(source, (size + adjustment) as usize)
        };
        let position = part.start_byte() + cut;
        if position != part.start_byte() && position < part.end_byte() {
            return Some((position, delimiter));
        }
    }
    None
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
