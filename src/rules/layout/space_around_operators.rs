//! `Layout/SpaceAroundOperators`.
//!
//! RuboCop reaches operators through a dozen handlers -- `on_send`, `on_binary`, `on_pair`,
//! `on_if`, `on_class`, `on_sclass`, `on_resbody`, the pattern-matching ones -- that all funnel
//! into one `check_operator`. Each of those handlers corresponds to a tree-sitter node kind, so
//! the walk below enumerates the same operators the cop's handlers do; anything else in the file
//! (a `..` range, a unary minus, `a[1]`) is deliberately not an operator here either.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::source::SourceFile;

/// The node kinds carrying an operator RuboCop checks. `binary` stands in for `on_send`,
/// `on_binary`, `on_and` and `on_or`, which all reduce to the same shape in tree-sitter.
const OPERATOR_NODES: &[&str] = &[
    "binary",
    "argument_list",
    "assignment",
    "operator_assignment",
    "pair",
    "conditional",
    "superclass",
    "singleton_class",
    "exception_variable",
    "match_pattern",
    "as_pattern",
    "alternative_pattern",
];

/// The comparisons of RuboCop's `ASSIGNMENT_OR_COMPARISON_TOKENS`, spelled as source rather than
/// as lexer token names. `<<` and the assignment operators are recognised structurally instead.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">="];

/// Two spaces: RuboCop's `EXCESSIVE_SPACE`, the padding that makes an otherwise correctly spaced
/// operator justify itself by aligning with a neighbouring line.
const EXCESSIVE_SPACE: &str = "  ";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let operators = collect(context);
    if operators.is_empty() {
        return;
    }
    let settings = Settings::read(context);
    let comments = comment_columns(context);
    // Only a file that actually pads an operator pays for the line and token bookkeeping.
    let alignment = OnceCell::new();
    for operator in &operators {
        if let Some(offense) = check_operator(context, &settings, &comments, &alignment, operator) {
            offenses.push(offense);
        }
    }
}

/// The cop's own configuration, plus the two neighbouring cops RuboCop consults.
struct Settings {
    allow_for_alignment: bool,
    space_around_exponent: bool,
    space_around_rational: bool,
    force_equal_sign_alignment: bool,
}

impl Settings {
    fn read(context: &RuleContext<'_>) -> Self {
        Self {
            allow_for_alignment: context.setting("AllowForAlignment").unwrap_or(true),
            space_around_exponent: context
                .setting::<String>("EnforcedStyleForExponentOperator")
                .as_deref()
                == Some("space"),
            space_around_rational: context
                .setting::<String>("EnforcedStyleForRationalLiterals")
                .as_deref()
                == Some("space"),
            force_equal_sign_alignment: context
                .setting_of("Layout/ExtraSpacing", "ForceEqualSignAlignment")
                .unwrap_or(false),
        }
    }
}

/// Which handler an operator came from. RuboCop singles out the plain assignment case, which
/// takes the equals-sign alignment path in `excess_leading_space?`; every other handler shares
/// the generic path.
#[derive(Clone, Copy, PartialEq)]
enum Site {
    Assignment,
    Other,
}

struct OperatorSite {
    range: Range<usize>,
    site: Site,
    /// The right operand, which the trailing-space check aligns against.
    right: Range<usize>,
    right_is_rational: bool,
}

fn collect(context: &RuleContext<'_>) -> Vec<OperatorSite> {
    let table_style = OnceCell::new();
    let mut sites = Vec::new();
    for node in context.nodes_of_any(OPERATOR_NODES) {
        match node.kind() {
            "binary" => collect_binary(context, node, &mut sites),
            "argument_list" => collect_block_pass(context, node, &mut sites),
            "assignment" | "operator_assignment" => collect_assignment(context, node, &mut sites),
            "pair" => collect_pair(context, node, &table_style, &mut sites),
            "conditional" => {
                push_operators(
                    node,
                    "?",
                    node.child_by_field_name("consequence"),
                    &mut sites,
                );
                push_operators(
                    node,
                    ":",
                    node.child_by_field_name("alternative"),
                    &mut sites,
                );
            }
            // `class Foo < Bar` puts `< Bar` in one node, so its named child is the right operand.
            "superclass" => push_operators(node, "<", first_named_child(node), &mut sites),
            "singleton_class" => push_operators(node, "<<", Some(node), &mut sites),
            "exception_variable" => push_operators(node, "=>", first_named_child(node), &mut sites),
            "match_pattern" => {
                // `on_match_pattern` runs only where the syntax exists.
                if context.target_ruby_version() >= RubyVersion::new(3, 0) {
                    push_operators(node, "=>", Some(node), &mut sites);
                }
            }
            "as_pattern" => push_operators(node, "=>", Some(node), &mut sites),
            "alternative_pattern" => push_operators(node, "|", Some(node), &mut sites),
            _ => {}
        }
    }
    sites.sort_by_key(|site| site.range.start);
    sites
}

fn collect_binary(context: &RuleContext<'_>, node: Node<'_>, sites: &mut Vec<OperatorSite>) {
    let (Some(operator), Some(left), Some(right)) = (
        node.child_by_field_name("operator"),
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return;
    };
    let text = context.source.node_text(operator);
    // `return +1` and `next -1` are jumps carrying a signed literal; tree-sitter reads the
    // keyword as the left operand of a binary expression, where RuboCop sees no operator at all.
    if matches!(text, "+" | "-") && matches!(left.kind(), "return" | "break" | "next") {
        return;
    }
    // `/re/ =~ str` is a `match_with_lvasgn`, not a send, because the match may bind the
    // pattern's named captures as local variables. The cop has no handler for it, so the
    // operator goes unchecked -- unlike `str =~ /re/` and `/re/ !~ str`, which stay sends.
    if text == "=~" && left.kind() == "regex" {
        return;
    }
    // `rational_literal?`: `1/48r` is a single literal to RuboCop, which skips the send rather
    // than judging the spacing around its slash.
    let right_is_rational = right.kind() == "rational";
    if text == "/" && right_is_rational && is_integer_literal(left) {
        return;
    }
    sites.push(OperatorSite {
        range: operator.byte_range(),
        site: Site::Other,
        right: right.byte_range(),
        right_is_rational,
    });
}

fn collect_assignment(context: &RuleContext<'_>, node: Node<'_>, sites: &mut Vec<OperatorSite>) {
    if is_parameter_default(node) {
        return;
    }
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return;
    };
    let Some(operator) = operator_between(node, left, right) else {
        return;
    };
    // tree-sitter reads `a[0] =~ /x/` as assigning `~ /x/` to `a[0]`, but the source spells one
    // operator: `=` butted against `~` can only be Ruby's `=~`, which lexes as a single token
    // and reaches RuboCop as a match rather than as an assignment. `a[0] = ~b` keeps its space,
    // and so keeps its two operators.
    if context.source.node_text(operator) == "="
        && context.source.text().as_bytes().get(operator.end_byte()) == Some(&b'~')
    {
        let operand = leading_tilde_operand(right, operator.end_byte());
        sites.push(OperatorSite {
            range: operator.start_byte()..operator.end_byte() + 1,
            site: Site::Other,
            right: operand.unwrap_or(right).byte_range(),
            right_is_rational: false,
        });
        return;
    }
    // `foo.bar = 1` and `x += 1` reach RuboCop as sends, reported as `:special_asgn`; only the
    // assignment node types take the equals-sign alignment path.
    let assigns_a_variable = if node.kind() == "operator_assignment" {
        matches!(context.source.node_text(operator), "||=" | "&&=")
    } else {
        !matches!(left.kind(), "call" | "element_reference")
    };
    sites.push(OperatorSite {
        range: operator.byte_range(),
        site: if assigns_a_variable {
            Site::Assignment
        } else {
            Site::Other
        },
        right: right.byte_range(),
        right_is_rational: right.kind() == "rational",
    });
}

/// What a mis-read `=~` matches against. The `~` tree-sitter split off always opens the
/// assignment's right-hand side, however deeply that side nests it -- `x =~ /re/ && y` puts it
/// under a binary node, `x =~ f ? a : b` under a conditional one.
fn leading_tilde_operand<'tree>(right: Node<'tree>, at: usize) -> Option<Node<'tree>> {
    let mut current = right;
    while current.start_byte() == at {
        if current.kind() == "unary" && current.child(0).is_some_and(|op| op.kind() == "~") {
            return current.child_by_field_name("operand");
        }
        current = current.child(0)?;
    }
    None
}

/// tree-sitter reads `a&b` as `a(&b)`, a block pass. Ruby's lexer only reads `&` that way when a
/// space separates it from the receiver, so an ampersand butted against the name is the binary
/// operator, which RuboCop reports as one.
fn collect_block_pass(context: &RuleContext<'_>, node: Node<'_>, sites: &mut Vec<OperatorSite>) {
    let Some(first) = node.child(0) else {
        return;
    };
    if first.kind() != "block_argument" || first.start_byte() != node.start_byte() {
        return;
    }
    let bytes = context.source.text().as_bytes();
    let preceded_by_space = node
        .start_byte()
        .checked_sub(1)
        .is_none_or(|index| bytes[index].is_ascii_whitespace());
    if preceded_by_space {
        return;
    }
    let (Some(operator), Some(right)) = (first.child(0), first.named_child(0)) else {
        return;
    };
    if operator.kind() != "&" {
        return;
    }
    sites.push(OperatorSite {
        range: operator.byte_range(),
        site: Site::Other,
        right: right.byte_range(),
        right_is_rational: false,
    });
}

fn collect_pair(
    context: &RuleContext<'_>,
    node: Node<'_>,
    table_style: &OnceCell<bool>,
    sites: &mut Vec<OperatorSite>,
) {
    let Some(operator) = child_of_kind(node, "=>") else {
        return;
    };
    // Table-style hash alignment pads the rockets of a multiline hash on purpose, and RuboCop
    // leaves the whole hash alone rather than reporting every one of them.
    if *table_style.get_or_init(|| hash_table_style(context)) && !pairs_on_same_line(node) {
        return;
    }
    sites.push(OperatorSite {
        range: operator.byte_range(),
        site: Site::Other,
        right: node.byte_range(),
        right_is_rational: false,
    });
}

/// Reports every `operator` child of `node`. `1 | 2 | 3` is one alternative pattern here where
/// RuboCop nests one node per bar, so a node can carry more than one operator.
fn push_operators(
    node: Node<'_>,
    operator: &str,
    right: Option<Node<'_>>,
    sites: &mut Vec<OperatorSite>,
) {
    let Some(right) = right else {
        return;
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == operator {
            sites.push(OperatorSite {
                range: child.byte_range(),
                site: Site::Other,
                right: right.byte_range(),
                right_is_rational: false,
            });
        }
    }
}

fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn first_named_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next()
}

fn operator_between<'tree>(
    node: Node<'tree>,
    left: Node<'_>,
    right: Node<'_>,
) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).find(|child| {
        child.start_byte() >= left.end_byte() && child.end_byte() <= right.start_byte()
    })
}

/// RuboCop folds a sign into a numeric literal, so `-1/3r` is as much a rational literal as
/// `1/3r`.
fn is_integer_literal(node: Node<'_>) -> bool {
    match node.kind() {
        "integer" => true,
        "unary" => node
            .child_by_field_name("operand")
            .is_some_and(|operand| operand.kind() == "integer"),
        _ => false,
    }
}

/// tree-sitter reads `def f(a = nil, b = nil)` as a single optional parameter whose default is
/// the multiple assignment `nil, b = nil`. RuboCop sees two `optarg`s, whose `=` belongs to
/// `Layout/SpaceAroundEqualsInParameterDefault` rather than to this cop, so an assignment
/// standing where a parameter default belongs carries no operator to check.
fn is_parameter_default(node: Node<'_>) -> bool {
    if node
        .child_by_field_name("left")
        .is_none_or(|left| left.kind() != "left_assignment_list")
    {
        return false;
    }
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "optional_parameter" => return true,
            "assignment" => current = parent,
            _ => return false,
        }
    }
    false
}

fn hash_table_style(context: &RuleContext<'_>) -> bool {
    const COP: &str = "Layout/HashAlignment";
    const KEY: &str = "EnforcedHashRocketStyle";
    // The parameter takes either one style or a list of them.
    if let Some(styles) = context.setting_of::<Vec<String>>(COP, KEY) {
        return styles.iter().any(|style| style == "table");
    }
    context.setting_of::<String>(COP, KEY).as_deref() == Some("table")
}

/// Whether any two neighbouring pairs of the enclosing hash share a line, RuboCop's test for a
/// hash that is not written one entry per line.
fn pairs_on_same_line(pair: Node<'_>) -> bool {
    let Some(parent) = pair.parent() else {
        return false;
    };
    let mut cursor = parent.walk();
    let lines: Vec<usize> = parent
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "pair")
        .map(|child| child.start_position().row)
        .collect();
    lines.windows(2).any(|window| window[0] == window[1])
}

fn check_operator<'src>(
    context: &RuleContext<'src>,
    settings: &Settings,
    comments: &HashMap<usize, usize>,
    alignment: &OnceCell<Alignment<'src>>,
    operator: &OperatorSite,
) -> Option<Offense> {
    let source = context.source.text();
    let padded_range = surrounding_space(source, &operator.range)?;
    let (line, _) = context.source.line_column(operator.range.start);
    // An operator that trails its line, with the rest of the line taken up by a comment, is
    // spaced as well as it can be.
    if let Some(column) = comments.get(&line) {
        let (_, last_column) = context.source.line_column(padded_range.end);
        if last_column - 1 == *column {
            return None;
        }
    }

    let text = &source[operator.range.clone()];
    let padded = &source[padded_range.clone()];
    let message = if should_not_have_surrounding_space(settings, text, operator.right_is_rational) {
        if padded == text {
            return None;
        }
        format!("Space around operator `{text}` detected.")
    } else if wrapped_in_space(padded) {
        // Nothing but a single space on either side is beyond reproach, and answering that
        // without indexing the file's lines is what keeps the common case cheap.
        if !padded.starts_with(EXCESSIVE_SPACE) && !padded.ends_with(EXCESSIVE_SPACE) {
            return None;
        }
        let alignment = alignment.get_or_init(|| Alignment::new(context));
        if !excess_leading_space(settings, alignment, operator, padded)
            && !excess_trailing_space(settings, alignment, operator, padded)
        {
            return None;
        }
        format!("Operator `{text}` should be surrounded by a single space.")
    } else {
        format!("Surrounding space missing for operator `{text}`.")
    };

    Some(
        context
            .offense(message, operator.range.clone())
            .corrected_by(Edit {
                start: padded_range.start,
                end: padded_range.end,
                replacement: correction(settings, text, padded, operator.right_is_rational),
                safe: true,
            }),
    )
}

/// RuboCop's `range_with_surrounding_space`: the operator plus the horizontal whitespace on
/// either side, plus the line breaks that follow it. `None` stands for the range that begins
/// with a line break, which `check_operator` refuses to judge.
fn surrounding_space(source: &str, operator: &Range<usize>) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    let mut start = operator.start;
    while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    if start > 0 && bytes[start - 1] == b'\n' {
        return None;
    }
    let mut end = operator.end;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    while end < bytes.len() && bytes[end] == b'\n' {
        end += 1;
    }
    Some(start..end)
}

fn should_not_have_surrounding_space(
    settings: &Settings,
    operator: &str,
    right_is_rational: bool,
) -> bool {
    match operator {
        "**" => !settings.space_around_exponent,
        "/" => right_is_rational && !settings.space_around_rational,
        _ => false,
    }
}

fn correction(
    settings: &Settings,
    operator: &str,
    padded: &str,
    right_is_rational: bool,
) -> String {
    if should_not_have_surrounding_space(settings, operator, right_is_rational) {
        operator.to_owned()
    } else if padded.ends_with('\n') {
        format!(" {operator}\n")
    } else if settings.force_equal_sign_alignment && !padded.ends_with(' ') {
        // `Layout/ExtraSpacing` pads the left side itself; padding both would leave the two
        // cops correcting each other forever.
        format!("{padded} ")
    } else {
        format!(" {operator} ")
    }
}

/// RuboCop's `/^\s.*\s$/` over the padded operator, with Ruby's line anchors: some line of the
/// range has to both start and end with whitespace.
fn wrapped_in_space(padded: &str) -> bool {
    let bytes = padded.as_bytes();
    let line_starts = std::iter::once(0).chain(
        bytes
            .iter()
            .enumerate()
            .filter(|(index, byte)| **byte == b'\n' && index + 1 < bytes.len())
            .map(|(index, _)| index + 1),
    );
    for start in line_starts {
        if !is_ruby_space(bytes[start]) {
            continue;
        }
        for end in start + 1..bytes.len() {
            // `$` matches at the end of the range and before any line break within it.
            if is_ruby_space(bytes[end]) && (end + 1 == bytes.len() || bytes[end + 1] == b'\n') {
                return true;
            }
            // `.` never crosses a line break, so the match cannot reach past one.
            if bytes[end] == b'\n' {
                break;
            }
        }
    }
    false
}

fn is_ruby_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

fn excess_leading_space(
    settings: &Settings,
    alignment: &Alignment<'_>,
    operator: &OperatorSite,
    padded: &str,
) -> bool {
    if !settings.allow_for_alignment || !padded.starts_with(EXCESSIVE_SPACE) {
        return false;
    }
    if operator.site != Site::Assignment {
        return !alignment.aligned_with_operator(&operator.range);
    }
    if alignment.aligned_with_preceding_equals(&operator.range) == Aligned::Yes {
        return false;
    }
    alignment.aligned_with_subsequent_equals(&operator.range) == Aligned::No
}

fn excess_trailing_space(
    settings: &Settings,
    alignment: &Alignment<'_>,
    operator: &OperatorSite,
    padded: &str,
) -> bool {
    padded.ends_with(EXCESSIVE_SPACE)
        && (!settings.allow_for_alignment || !alignment.aligned_with_something(&operator.right))
}

/// 1-based line to the 0-based column of the comment on it, RuboCop's `comment_at_line`.
fn comment_columns(context: &RuleContext<'_>) -> HashMap<usize, usize> {
    let mut columns = HashMap::new();
    for comment in context.comment_ranges() {
        let (line, column) = context.source.line_column(comment.start);
        columns.insert(line, column - 1);
    }
    columns
}

#[derive(PartialEq)]
enum Aligned {
    Yes,
    No,
    None,
}

/// What kind of lexer token an operator would have been. `aligned_equals_operator?` treats an
/// append and an assignment as interchangeable, so the two have to stay apart from the
/// comparisons that only ever match themselves.
#[derive(Clone, Copy, PartialEq)]
enum TokenKind {
    EqualSign,
    Lshift,
    Comparison,
}

#[derive(Clone, Copy)]
struct EqualsToken {
    start: usize,
    /// The 0-based character column just past the operator.
    last_column: usize,
    kind: TokenKind,
}

/// Everything `PrecedingFollowingAlignment` reads out of a file: the lines, the comment lines it
/// will not align against, and the `=`-ish operators it aligns with.
pub(super) struct Alignment<'src> {
    source: &'src SourceFile,
    /// The file's lines without their line breaks, as `processed_source.lines` holds them.
    lines: Vec<&'src str>,
    indents: Vec<usize>,
    blank: Vec<bool>,
    /// 1-based lines carrying a comment that starts the line.
    comment_lines: HashSet<usize>,
    /// 1-based line to its first assignment or comparison operator.
    equals_tokens: HashMap<usize, EqualsToken>,
    /// 1-based lines carrying an assignment `=`, ignoring parameter defaults and endless `def`s.
    assignment_lines: HashSet<usize>,
}

impl<'src> Alignment<'src> {
    pub(super) fn new(context: &RuleContext<'src>) -> Self {
        let source: &'src SourceFile = context.source;
        // `Parser::Source::Buffer#source_lines` keeps a trailing empty line for a file ending in
        // a newline, which is exactly what `SourceFile::line_count` counts.
        let lines: Vec<&str> = (1..=source.line_count())
            .map(|line| {
                let text = source.line(line);
                text.strip_suffix('\n').unwrap_or(text)
            })
            .collect();
        let indents = lines
            .iter()
            .map(|line| line.chars().take_while(|c| c.is_whitespace()).count())
            .collect();
        let blank = lines
            .iter()
            .map(|line| line.chars().all(char::is_whitespace))
            .collect();

        let mut comment_lines = HashSet::new();
        for comment in context.comment_ranges() {
            let (line, column) = source.line_column(comment.start);
            if lines[line - 1]
                .chars()
                .position(|character| !character.is_whitespace())
                .is_some_and(|index| index + 1 == column)
            {
                comment_lines.insert(line);
            }
        }

        let mut alignment = Self {
            source,
            lines,
            indents,
            blank,
            comment_lines,
            equals_tokens: HashMap::new(),
            assignment_lines: HashSet::new(),
        };
        alignment.collect_equals_tokens(context);
        alignment
    }

    fn collect_equals_tokens(&mut self, context: &RuleContext<'_>) {
        for node in context.nodes() {
            let (operator, kind, assigns) = match node.kind() {
                "assignment" => (
                    node.child_by_field_name("left").and_then(|left| {
                        node.child_by_field_name("right")
                            .and_then(|right| operator_between(node, left, right))
                    }),
                    TokenKind::EqualSign,
                    true,
                ),
                "operator_assignment" => (
                    node.child_by_field_name("operator"),
                    TokenKind::EqualSign,
                    true,
                ),
                // An optional parameter's `=` and an endless `def`'s are still tokens to align
                // against, even though `assignment_lines` leaves them out.
                "optional_parameter" | "method" | "singleton_method" => {
                    (child_of_kind(node, "="), TokenKind::EqualSign, false)
                }
                "singleton_class" => (child_of_kind(node, "<<"), TokenKind::Lshift, false),
                "binary" => {
                    let operator = node.child_by_field_name("operator");
                    let text = operator.map(|operator| context.source.node_text(operator));
                    match text {
                        Some("<<") => (operator, TokenKind::Lshift, false),
                        Some(text) if COMPARISON_OPERATORS.contains(&text) => {
                            (operator, TokenKind::Comparison, false)
                        }
                        _ => (None, TokenKind::Comparison, false),
                    }
                }
                _ => (None, TokenKind::Comparison, false),
            };
            let Some(operator) = operator else {
                continue;
            };
            let (line, _) = self.source.line_column(operator.start_byte());
            let (_, end_column) = self.source.line_column(operator.end_byte());
            let token = EqualsToken {
                start: operator.start_byte(),
                last_column: end_column - 1,
                kind,
            };
            self.equals_tokens
                .entry(line)
                .and_modify(|current| {
                    if token.start < current.start {
                        *current = token;
                    }
                })
                .or_insert(token);
            if assigns {
                self.assignment_lines.insert(line);
            }
        }
    }

    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn slice(&self, range: &Range<usize>) -> &str {
        &self.source.text()[range.clone()]
    }

    pub(super) fn aligned_with_something(&self, range: &Range<usize>) -> bool {
        self.aligned_with_adjacent_line(range, Predicate::Token)
    }

    fn aligned_with_operator(&self, range: &Range<usize>) -> bool {
        self.aligned_with_adjacent_line(range, Predicate::Operator)
    }

    fn aligned_with_adjacent_line(&self, range: &Range<usize>, predicate: Predicate) -> bool {
        let (line, _) = self.source.line_column(range.start);
        // RuboCop searches the preceding lines first, then the following ones; both lists hold
        // 0-based indices into `lines`.
        let preceding: Vec<usize> = (0..line.saturating_sub(1)).rev().collect();
        let following: Vec<usize> = (line..self.line_count()).collect();
        let candidates = [preceding, following];
        if self.aligned_with_any_line(&candidates, range, None, predicate) {
            return true;
        }
        // Failing that, the nearest line indented like this one gets to answer instead.
        let base = self.lines[line - 1]
            .chars()
            .position(|character| !character.is_whitespace());
        base.is_some_and(|indent| {
            self.aligned_with_any_line(&candidates, range, Some(indent), predicate)
        })
    }

    fn aligned_with_any_line(
        &self,
        candidates: &[Vec<usize>; 2],
        range: &Range<usize>,
        indent: Option<usize>,
        predicate: Predicate,
    ) -> bool {
        candidates
            .iter()
            .any(|lines| self.aligned_with_line(lines, range, indent, predicate))
    }

    /// The first line of `lines` that is neither blank nor a comment (and, when `indent` is
    /// given, is indented the same) settles the question on its own.
    fn aligned_with_line(
        &self,
        lines: &[usize],
        range: &Range<usize>,
        indent: Option<usize>,
        predicate: Predicate,
    ) -> bool {
        for &index in lines {
            if self.comment_lines.contains(&(index + 1)) {
                continue;
            }
            let line = self.lines[index];
            let Some(first) = line
                .chars()
                .position(|character| !character.is_whitespace())
            else {
                continue;
            };
            if indent.is_some_and(|indent| indent != first) {
                continue;
            }
            let matched = match predicate {
                Predicate::Token => self.aligned_words(range, line),
                Predicate::Operator => self.aligned_identical(range, line),
            };
            return matched || self.aligned_equals_operator(range, index + 1);
        }
        false
    }

    fn aligned_words(&self, range: &Range<usize>, line: &str) -> bool {
        let (_, column) = self.source.line_column(range.start);
        let left_edge = column - 1;
        let characters: Vec<char> = line.chars().collect();
        // `line[left_edge - 1, 2]` in Ruby, where a zero edge reads the line's last character
        // and so can never hold the two-character match.
        if left_edge > 0
            && characters
                .get(left_edge - 1..left_edge + 1)
                .is_some_and(|pair| pair[0].is_whitespace() && !pair[1].is_whitespace())
        {
            return true;
        }
        self.same_text_at(range, &characters, left_edge)
    }

    fn aligned_identical(&self, range: &Range<usize>, line: &str) -> bool {
        let (_, column) = self.source.line_column(range.start);
        let characters: Vec<char> = line.chars().collect();
        self.same_text_at(range, &characters, column - 1)
    }

    fn same_text_at(&self, range: &Range<usize>, characters: &[char], column: usize) -> bool {
        let token = self.slice(range);
        let width = token.chars().count();
        characters
            .get(column..column + width)
            .is_some_and(|slice| slice.iter().copied().eq(token.chars()))
    }

    /// Whether the operator ends in the same column as the first assignment or comparison
    /// operator of `line`, which is how RuboCop lets an `=` line up with the one above it.
    fn aligned_equals_operator(&self, range: &Range<usize>, line: usize) -> bool {
        let Some(token) = self.equals_tokens.get(&line) else {
            return false;
        };
        let source = self.slice(range);
        let (_, end_column) = self.source.line_column(range.end);
        if end_column - 1 != token.last_column {
            return false;
        }
        source.ends_with('=')
            || (source == "<<" && token.kind == TokenKind::EqualSign)
            || (source.ends_with('=') && token.kind == TokenKind::Lshift)
    }

    fn aligned_with_preceding_equals(&self, range: &Range<usize>) -> Aligned {
        let (line, _) = self.source.line_column(range.start);
        let lines: Vec<usize> = (1..=line).rev().collect();
        self.aligned_with_equals_sign(range, &lines)
    }

    fn aligned_with_subsequent_equals(&self, range: &Range<usize>) -> Aligned {
        let (line, _) = self.source.line_column(range.start);
        let lines: Vec<usize> = (line..=self.line_count()).collect();
        self.aligned_with_equals_sign(range, &lines)
    }

    fn aligned_with_equals_sign(&self, range: &Range<usize>, lines: &[usize]) -> Aligned {
        let (line, _) = self.source.line_column(range.start);
        let token_indent = self.indentation(line);
        let assignments = self.relevant_assignment_lines(lines);
        // The operator's own line comes first; the next assignment of the same block decides.
        let Some(&relevant) = assignments.get(1) else {
            return Aligned::None;
        };
        if self.indentation(relevant) < token_indent {
            return Aligned::None;
        }
        if self.aligned_equals_operator(range, relevant) {
            Aligned::Yes
        } else {
            Aligned::No
        }
    }

    /// The lines of the same block, at the same indentation, that hold an assignment. The walk
    /// stops at the first line leaving the block, or at the blank line ending it.
    fn relevant_assignment_lines(&self, lines: &[usize]) -> Vec<usize> {
        let mut result = Vec::new();
        let Some(&first) = lines.first() else {
            return result;
        };
        let original_indent = self.indentation(first);
        let mut indent_at_level = true;
        for &line in lines {
            let current_indent = self.indentation(line);
            let blank = self.blank.get(line - 1).copied().unwrap_or(true);
            if (current_indent < original_indent && !blank) || (indent_at_level && blank) {
                break;
            }
            if self.assignment_lines.contains(&line) && current_indent == original_indent {
                result.push(line);
            }
            if !blank {
                indent_at_level = current_indent == original_indent;
            }
        }
        result
    }

    fn indentation(&self, line: usize) -> usize {
        self.indents.get(line - 1).copied().unwrap_or(0)
    }
}

#[derive(Clone, Copy)]
enum Predicate {
    Token,
    Operator,
}
