//! `Layout/SpaceAroundOperators`.
//!
//! RuboCop reaches operators through a dozen handlers -- `on_send`, `on_binary`, `on_pair`,
//! `on_if`, `on_class`, `on_sclass`, `on_resbody`, the pattern-matching ones -- that all funnel
//! into one `check_operator`. Each of those handlers corresponds to a tree-sitter node kind, so
//! the walk below enumerates the same operators the cop's handlers do; anything else in the file
//! (a `..` range, a unary minus, `a[1]`) is deliberately not an operator here either.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use super::alignment::{Aligned, Alignment};
use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support;
use crate::rules::support::is_ruby_space;

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
        match node.kind_str() {
            "binary" => collect_binary(context, node, &mut sites),
            "argument_list" => collect_block_pass(context, node, &mut sites),
            "assignment" | "operator_assignment" => collect_assignment(context, node, &mut sites),
            "pair" => collect_pair(context, node, &table_style, &mut sites),
            "conditional" => {
                push_operators(
                    node,
                    "?",
                    node.field("consequence"),
                    &mut sites,
                );
                push_operators(
                    node,
                    ":",
                    node.field("alternative"),
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
        node.field("operator"),
        node.field("left"),
        node.field("right"),
    ) else {
        return;
    };
    let text = context.source.node_text(operator);
    // `return +1` and `next -1` are jumps carrying a signed literal; tree-sitter reads the
    // keyword as the left operand of a binary expression, where RuboCop sees no operator at all.
    if matches!(text, "+" | "-") && matches!(left.kind_str(), "return" | "break" | "next") {
        return;
    }
    // `/re/ =~ str` is a `match_with_lvasgn`, not a send, because the match may bind the
    // pattern's named captures as local variables. The cop has no handler for it, so the
    // operator goes unchecked -- unlike `str =~ /re/` and `/re/ !~ str`, which stay sends.
    if text == "=~" && left.kind_str() == "regex" {
        return;
    }
    // **`a +42` is a call carrying a signed argument, not an addition.** Ruby reads a sign as
    // belonging to the operand when a space stands before it and none after, and the receiver is
    // a bare name that could take arguments -- so upstream builds `(send nil :a (int 42))` with
    // no operator in it at all, while the grammar builds a `binary`.
    if matches!(text, "+" | "-") && is_argument_sign(context, operator, left) {
        return;
    }
    // `rational_literal?`: `1/48r` is a single literal to RuboCop, which skips the send rather
    // than judging the spacing around its slash.
    let right_is_rational = right.kind_str() == "rational";
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
        node.field("left"),
        node.field("right"),
    ) else {
        return;
    };
    let Some(operator) = operator_between(node, left, right) else {
        return;
    };
    // **An attribute written on a safe navigation is a `csend`, and this cop has no handler for
    // one.** `obj&.foo = y` reaches upstream as `(csend … :foo= …)`, so its `=` goes unchecked --
    // while `obj.foo = y` is a `send` and is checked.
    if left.kind_str() == "call"
        && left
            .field("operator")
            .is_some_and(|dot| context.source.node_text(dot) == "&.")
    {
        return;
    }
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
    let assigns_a_variable = if node.kind_str() == "operator_assignment" {
        matches!(context.source.node_text(operator), "||=" | "&&=")
    } else {
        !matches!(left.kind_str(), "call" | "element_reference")
    };
    sites.push(OperatorSite {
        range: operator.byte_range(),
        site: if assigns_a_variable {
            Site::Assignment
        } else {
            Site::Other
        },
        right: right.byte_range(),
        right_is_rational: right.kind_str() == "rational",
    });
}

/// What a mis-read `=~` matches against. The `~` tree-sitter split off always opens the
/// assignment's right-hand side, however deeply that side nests it -- `x =~ /re/ && y` puts it
/// under a binary node, `x =~ f ? a : b` under a conditional one.
fn leading_tilde_operand<'tree>(right: Node<'tree>, at: usize) -> Option<Node<'tree>> {
    let mut current = right;
    while current.start_byte() == at {
        if current.kind_str() == "unary" && current.child(0).is_some_and(|op| op.kind_str() == "~") {
            return current.field("operand");
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
    if first.kind_str() != "block_argument" || first.start_byte() != node.start_byte() {
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
    if operator.kind_str() != "&" {
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
        if child.kind_str() == operator {
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
        .find(|child| child.kind_str() == kind)
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
    match node.kind_str() {
        "integer" => true,
        "unary" => node
            .field("operand")
            .is_some_and(|operand| operand.kind_str() == "integer"),
        _ => false,
    }
}

/// tree-sitter reads `def f(a = nil, b = nil)` as a single optional parameter whose default is
/// the multiple assignment `nil, b = nil`. RuboCop sees two `optarg`s, whose `=` belongs to
/// `Layout/SpaceAroundEqualsInParameterDefault` rather than to this cop, so an assignment
/// standing where a parameter default belongs carries no operator to check.
fn is_parameter_default(node: Node<'_>) -> bool {
    if node
        .field("left")
        .is_none_or(|left| left.kind_str() != "left_assignment_list")
    {
        return false;
    }
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind_str() {
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
    let lines: Vec<usize> = parent.named_children(&mut cursor)
        .filter(|child| child.kind_str() == "pair")
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
    // The left side is `range_with_surrounding_space(side: :left, newlines: false)` and the right is
    // the same with `newlines: true`, **but this cop gives up entirely when a line break sits on the
    // left**: an operator that opens a line is aligned rather than spaced, so the range is no longer
    // the thing to measure. That decision belongs to the cop and stays here.
    let start = support::final_pos(source, operator.start, false, false, false, false);
    if start > 0 && source.as_bytes()[start - 1] == b'\n' {
        return None;
    }
    Some(start..support::final_pos(source, operator.end, true, false, true, false))
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

/// Whether the sign is the start of an argument rather than an operator: a space before it, none
/// after it, and a receiver Ruby would read as a method able to take one.
fn is_argument_sign(context: &RuleContext<'_>, operator: Node<'_>, left: Node<'_>) -> bool {
    if left.kind_str() != "identifier" {
        return false;
    }
    // A local variable is a value, and a value takes no arguments.
    if context.variable_analysis().is_reference(left) {
        return false;
    }
    let text = context.source.text();
    let before = text[..operator.start_byte()].ends_with([' ', '\t']);
    let after = text[operator.end_byte()..].starts_with([' ', '\t']);
    before && !after
}
