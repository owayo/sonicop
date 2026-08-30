use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

/// Ruby's `\s`, which stays ASCII where this engine's would take in every Unicode blank.
const BLANK: &str = r"[ \t\r\n\x0B\x0C]";

/// `LINE_1_ENDING`: the closing quote of the first line, the blanks after it and the backslash.
static LINE_1_ENDING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r#"['"]{BLANK}*\\\n"#)).expect("the pattern compiles"));

/// `LINE_2_BEGINNING`: the opening quote of the second line.
static LINE_2_BEGINNING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r#"\A{BLANK}*['"]"#)).expect("the pattern compiles"));

/// `LEADING_STYLE_OFFENSE`.
static LEADING_STYLE_OFFENSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"({BLANK}+)(['"]{BLANK}*\\\n)"#)).expect("the pattern compiles")
});

/// `TRAILING_STYLE_OFFENSE`.
static TRAILING_STYLE_OFFENSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"(\A{BLANK}*['"])({BLANK}+)"#)).expect("the pattern compiles")
});

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let leading = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "trailing".to_owned())
        == "leading";
    let message = match leading {
        true => "Move trailing spaces to the start of the next line.",
        false => "Move leading spaces to the end of the previous line.",
    };
    for node in context.nodes_of_any(&["chained_string", "string"]) {
        if !is_dynamic_string(node, context) || !context.source.node_text(node).contains('\\') {
            continue;
        }
        let first_line = context.source.line_column(node.start_byte()).0;
        let last_line = context.source.line_column(node.end_byte()).0;
        for line in first_line..last_line {
            let one = context.source.slice(context.source.line_range(line));
            let two = context.source.slice(context.source.line_range(line + 1));
            // The offset the second line starts at, which is what upstream's running
            // `end_of_first_line` holds once the first line's length has been added to it.
            let border = context.source.line_start(line + 1);
            if !is_continuation(one, line, node, context) {
                continue;
            }
            let offense = match leading {
                true => leading_offense(one, two, border, message, context),
                false => trailing_offense(one, two, border, message, context),
            };
            offenses.extend(offense);
        }
    }
}

/// `investigate_leading_style`: the blanks at the end of the first line move to the start of the
/// second.
fn leading_offense(
    one: &str,
    two: &str,
    border: usize,
    message: &'static str,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    let matched = LEADING_STYLE_OFFENSE.captures(one)?;
    let spaces = matched.get(1)?.as_str();
    let ending = matched.get(2)?.as_str();
    let end = border - ending.len();
    let range = end - spaces.len()..end;
    let insert = border + LINE_2_BEGINNING.find(two)?.as_str().len();
    Some(
        context
            .offense(message, range.clone())
            .corrected_by_all(moved(range, insert, spaces)),
    )
}

/// `investigate_trailing_style`: the blanks at the start of the second line move to the end of the
/// first.
fn trailing_offense(
    one: &str,
    two: &str,
    border: usize,
    message: &'static str,
    context: &RuleContext<'_>,
) -> Option<Offense> {
    let matched = TRAILING_STYLE_OFFENSE.captures(two)?;
    let beginning = matched.get(1)?.as_str();
    let spaces = matched.get(2)?.as_str();
    let start = border + beginning.len();
    let range = start..start + spaces.len();
    let insert = border - LINE_1_ENDING.find(one)?.as_str().len();
    Some(
        context
            .offense(message, range.clone())
            .corrected_by_all(moved(range, insert, spaces)),
    )
}

/// `autocorrect`: the blanks are removed where they were and written where they belong.
fn moved(range: std::ops::Range<usize>, insert: usize, spaces: &str) -> Vec<Edit> {
    vec![
        Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        },
        Edit {
            start: insert,
            end: insert,
            replacement: spaces.to_owned(),
            safe: true,
        },
    ]
}

/// `continuation?`: the first line ends in a backslash, and no part of the literal spans the break
/// itself.
fn is_continuation(one: &str, line: usize, node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !one.ends_with("\\\n") {
        return false;
    }
    named_children_of(node, context).into_iter().all(|child| {
        let first = context.source.line_column(child.start_byte()).0;
        let last = context.source.line_column(child.end_byte()).0;
        !((first..last).contains(&line) && first != last)
    })
}

/// Whether the literal reaches upstream as a `dstr` rather than a `str`, which is what `on_dstr`
/// sees: adjacent literals written next to one another, a literal that interpolates, and one that
/// does not fit on a line.
fn is_dynamic_string(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "chained_string" {
        return true;
    }
    context.source.line_column(node.start_byte()).0 != context.source.line_column(node.end_byte()).0
        || named_children_of(node, context)
            .iter()
            .any(|child| child.kind_str() == "interpolation")
}
