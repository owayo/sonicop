use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::support::Interpolations;
use crate::source::is_protected;

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

    // RuboCop drops the `__END__` line and the data section behind it from the lines it walks, so
    // a long line down there is not a long line of code.
    let data_line = context
        .nodes_of("uninterpreted")
        .next()
        .map_or(usize::MAX, |node| node.start_position().row + 1);

    for line_number in 1..=context.source.line_count().min(data_line.saturating_sub(1)) {
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
    if let Some(object) = node.child_by_field_name("object") {
        signature.push_str(context.source.node_text(object));
        // The separator is whatever sits between the receiver and the name: `.` or `::`.
        signature.push_str(
            context
                .source
                .slice(object.end_byte()..name_start(node, object)),
        );
    }
    if let Some(name) = node.child_by_field_name("name") {
        signature.push_str(context.source.node_text(name));
    }
    if let Some(parameters) = node.child_by_field_name("parameters") {
        signature.push_str(context.source.node_text(parameters));
    }
    let body = node
        .child_by_field_name("body")
        .map_or("", |body| context.source.node_text(body));
    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: format!("{signature}\n{indent}  {body}\n{indent}end"),
        safe: true,
    }
}

fn name_start(node: Node<'_>, object: Node<'_>) -> usize {
    node.child_by_field_name("name")
        .map_or(object.end_byte(), |name| name.start_byte())
}

fn is_endless_method(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    !node
        .children(&mut cursor)
        .any(|child| child.kind() == "end")
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
        if matches!(node.kind(), "lambda") || BREAKABLE_KINDS.contains(&node.kind()) {
            order.insert(node.id(), index);
        }
        index += 1;
        children.clear();
        let mut cursor = node.walk();
        children.extend(node.named_children(&mut cursor));
        if MODIFIER_KINDS.contains(&node.kind()) {
            children.reverse();
        }
        stack.extend(children.iter().rev().copied());
    }
    order
}

/// Where a candidate sorts, with a block placed just ahead of the call it belongs to: upstream's
/// block node stands where the grammar puts the call, and the call is its first child.
fn visit_order(node: Node<'_>, order: &HashMap<usize, u32>) -> (u32, u8) {
    if matches!(node.kind(), "block" | "do_block") {
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
fn line_break_edits(context: &RuleContext<'_>, max: usize) -> HashMap<usize, Edit> {
    let breaker = Breaker {
        context,
        max,
        comment_lines: comment_lines(context),
        heredocs: context
            .nodes_of("heredoc_beginning")
            .map(|node| node.byte_range())
            .collect(),
    };
    let mut positions: HashMap<usize, usize> = HashMap::new();

    // Reversed, so that the first semicolon on a line is the one whose position survives.
    if context.source.text().contains(';') {
        for offset in semicolon_break_positions(context).into_iter().rev() {
            positions.insert(context.source.line_column(offset).0, offset);
        }
    }

    let order = upstream_order(context.root_node());
    let mut candidates: Vec<Node<'_>> = context.nodes_of_any(BREAKABLE_KINDS).collect();
    candidates.sort_by_key(|node| visit_order(*node, &order));

    for node in candidates {
        if matches!(node.kind(), "block" | "do_block") {
            if let Some(offset) = breaker.block_break_position(node) {
                // Upstream's block node starts at the receiver, not at the brace, so a call split
                // over two lines files its break under the line the receiver is on.
                let owner = node.parent().unwrap_or(node);
                positions.insert(owner.start_position().row + 1, offset);
            }
        } else if let Some(element) = breaker.breakable_element(node) {
            positions
                .entry(element.start_position().row + 1)
                .or_insert_with(|| element.start_byte());
        }
    }

    positions
        .into_iter()
        .map(|(line, start)| {
            (
                line,
                Edit {
                    start,
                    end: start,
                    replacement: "\n".to_owned(),
                    safe: true,
                },
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
        let opener = if node.kind() == "block" { 1 } else { 2 };
        let parameters = if is_lambda_block(node, self.context) {
            None
        } else {
            node.child_by_field_name("parameters")
        };
        let position = match parameters {
            Some(parameters) => parameters.end_byte(),
            None => node.start_byte() + opener,
        };
        Some(position.min(node.end_byte()))
    }

    /// `extract_breakable_node`: the element a break would be inserted before, if any.
    fn breakable_element<'t>(&self, node: Node<'t>) -> Option<Node<'t>> {
        if node.kind() == "call" && self.chained_to_heredoc(node) {
            return None;
        }
        if matches!(node.kind(), "method" | "singleton_method") && is_endless_method(node) {
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
        let container = match node.kind() {
            // `super(...)` is its own node type upstream rather than a `send`, so no cop callback
            // reaches it and it is never broken.
            "call" if is_super_call(node) => return None,
            "call" => node.child_by_field_name("arguments")?,
            "method" | "singleton_method" => node.child_by_field_name("parameters")?,
            // A `rescue` clause's exception list reaches RuboCop as an `array` too.
            "array" | "string_array" | "symbol_array" | "right_assignment_list" | "exceptions" => {
                node
            }
            // A kwargs hash has no braces and is never broken, only a literal one is.
            "hash" if self.context.source.node_text(node).starts_with('{') => node,
            "element_reference" => return Some(group_pairs(index_arguments(node), true)),
            // `a[b] = c` is the `[]=` call whose arguments are the subscripts and the value.
            "assignment" => {
                let left = node.child_by_field_name("left")?;
                if left.kind() != "element_reference" {
                    return None;
                }
                let mut children = index_arguments(left);
                children.push(node.child_by_field_name("right")?);
                return Some(group_pairs(children, true));
            }
            _ => return None,
        };
        let mut cursor = container.walk();
        // A comment is a node of the tree but not of RuboCop's AST, and a heredoc's body hangs off
        // the argument list it was opened in rather than off the opener, so neither is an element.
        let children: Vec<Node<'t>> = container
            .named_children(&mut cursor)
            .filter(|child| !matches!(child.kind(), "comment" | "heredoc_body"))
            .collect();
        // A literal hash's own pairs are its elements; only an argument list and an array literal
        // carry a brace-less hash that upstream's parser folds into one node. `process_args` then
        // unfolds it again -- but for a call's last argument only, so a hash followed by a block
        // argument, or one inside an array, stays a single element.
        let inside_array = matches!(
            container.kind(),
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
        let last_row = match node.kind() {
            "method" | "singleton_method" => match elements.last() {
                Some(last) => last.last_row,
                None => return false,
            },
            "call" => node
                .child_by_field_name("arguments")
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
        if matches!(node.kind(), "method" | "singleton_method") {
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
            node.kind(),
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
        node.kind() == "heredoc_beginning"
    }

    /// Whether a heredoc opens anywhere inside `node`.
    fn contains_heredoc(&self, node: Node<'_>) -> bool {
        let range = node.byte_range();
        self.heredocs
            .iter()
            .any(|heredoc| heredoc.start >= range.start && heredoc.end <= range.end)
    }

    fn receiver_contains_heredoc(&self, node: Node<'_>) -> bool {
        node.parent()
            .and_then(|parent| parent.child_by_field_name("receiver"))
            .is_some_and(|receiver| self.contains_heredoc(receiver))
    }

    /// A call whose receiver chain starts from a heredoc cannot take a break in its arguments.
    fn chained_to_heredoc(&self, node: Node<'_>) -> bool {
        let mut receiver = node.child_by_field_name("receiver");
        while let Some(current) = receiver {
            if self.is_heredoc(current) {
                return true;
            }
            receiver = current.child_by_field_name("receiver");
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
            let through_block = parent.kind() == "call"
                && parent
                    .child_by_field_name("block")
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
    let object = node.child_by_field_name("object");
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| {
            !matches!(child.kind(), "comment" | "heredoc_body")
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
    let is_entry = |node: &Node<'t>| matches!(node.kind(), "pair" | "hash_splat_argument");
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
    match node.parent() {
        Some(parent) if parent.kind() == "lambda" => true,
        Some(parent) => parent
            .child_by_field_name("method")
            .is_some_and(|method| context.source.node_text(method) == "lambda"),
        None => false,
    }
}

/// The node kinds that reach RuboCop as a `send`, where an unparenthesized first argument is
/// pinned in place.
fn is_call_like(node: Node<'_>) -> bool {
    matches!(node.kind(), "call" | "element_reference" | "assignment")
}

fn is_super_call(node: Node<'_>) -> bool {
    node.child_by_field_name("method")
        .is_some_and(|method| method.kind() == "super")
}

fn line_char_count(context: &RuleContext<'_>, line: usize) -> usize {
    let raw = context.source.line(line);
    raw.strip_suffix('\n').unwrap_or(raw).chars().count()
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
