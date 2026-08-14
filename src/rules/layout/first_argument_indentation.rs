//! `Layout/FirstArgumentIndentation`.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{
    AlignmentPass, begins_its_line, comments, display_column, grouped_arguments, line_indentation,
};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::send_range;
use crate::rules::node_ext::NodeExt;

/// `MethodIdentifierPredicates::OPERATOR_METHODS`.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

const NESTED_MSG: &str = "Bad indentation of the first argument.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "special_for_inner_method_call_in_parentheses".to_owned());
    // A neighbouring cop indenting every argument by a fixed amount owns the first one too, unless
    // the cop that forces a line break before it is turned on.
    if argument_alignment_is_fixed(context) && !first_argument_line_break_enabled(context) {
        return;
    }
    let width: i64 = context
        .setting::<i64>("IndentationWidth")
        .or_else(|| context.setting_of::<i64>("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let comment_lines = comment_lines(context);

    let mut pass = AlignmentPass::new();
    for call in context.nodes_of("call") {
        // `should_check?`. A setter is an `assignment` here rather than a call, so only the bare
        // operators have to be turned away.
        if bare_operator(context, call) {
            continue;
        }
        let arguments = grouped_arguments(call);
        let Some(first) = arguments.first().map(|argument| argument.range.clone()) else {
            continue;
        };
        if context.source.line_column(call.start_byte()).0
            == context.source.line_column(first.start).0
        {
            continue;
        }
        let indent = base_indentation(context, call, &first, &style, &comment_lines) + width;
        for (item, delta) in
            AlignmentPass::misaligned(context, std::slice::from_ref(&first), indent)
        {
            let correct = correction_range(context, call, &item, delta, &style);
            let base = base_description(context, call, &item, &style);
            pass.register(
                context,
                item,
                correct,
                delta,
                move |nested| {
                    if nested {
                        NESTED_MSG.to_owned()
                    } else {
                        format!("Indent the first argument one step more than {base}.")
                    }
                },
                offenses,
            );
        }
    }
}

/// `enforce_first_argument_with_fixed_indentation?`, which reads the neighbour only while it is
/// enabled.
fn argument_alignment_is_fixed(context: &RuleContext<'_>) -> bool {
    if context.setting_of::<bool>("Layout/ArgumentAlignment", "Enabled") == Some(false) {
        return false;
    }
    context
        .setting_of::<String>("Layout/ArgumentAlignment", "EnforcedStyle")
        .as_deref()
        == Some("with_fixed_indentation")
}

fn first_argument_line_break_enabled(context: &RuleContext<'_>) -> bool {
    context
        .setting_of::<bool>("Layout/FirstMethodArgumentLineBreak", "Enabled")
        .unwrap_or(false)
}

/// `comment_lines`: the lines holding a comment and nothing else.
fn comment_lines(context: &RuleContext<'_>) -> Vec<usize> {
    comments(context)
        .into_iter()
        .filter(|comment| begins_its_line(context, comment.start))
        .map(|comment| context.source.line_column(comment.start).0)
        .collect()
}

/// `bare_operator?`: an operator written as an operator rather than dispatched through a dot.
fn bare_operator(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    let Some(method) = call.field("method") else {
        return false;
    };
    if !OPERATOR_METHODS.contains(&context.source.node_text(method)) {
        return false;
    }
    // `dot?` holds for a literal `.` only; `&.` and `::` are something else there.
    call.field("operator")
        .is_none_or(|operator| context.source.node_text(operator) != ".")
}

fn base_indentation(
    context: &RuleContext<'_>,
    call: Node<'_>,
    first: &Range<usize>,
    style: &str,
    comment_lines: &[usize],
) -> i64 {
    if special_inner_call_indentation(context, call, style) {
        column_of(context, &base_range(call, first.start), comment_lines)
    } else {
        previous_code_line_indent(
            context,
            context.source.line_column(first.start).0,
            comment_lines,
        )
    }
}

/// `base_range`: what stands between the call and its first argument. A splatted call is measured
/// from the splat, which is where upstream's parser puts the enclosing node.
fn base_range(call: Node<'_>, argument: usize) -> Range<usize> {
    let start = call
        .parent()
        .filter(|parent| matches!(parent.kind_str(), "splat_argument" | "hash_splat_argument"))
        .map_or_else(|| call.start_byte(), |parent| parent.start_byte());
    start..argument
}

/// `column_of`: the column a range starts at, or -- when it spans line breaks -- the indentation of
/// the last code line it covers.
fn column_of(context: &RuleContext<'_>, range: &Range<usize>, comment_lines: &[usize]) -> i64 {
    let source = context.source.text()[range.clone()].trim();
    if source.contains('\n') {
        let line = context.source.line_column(range.start).0;
        previous_code_line_indent(
            context,
            line + source.matches('\n').count() + 1,
            comment_lines,
        )
    } else {
        display_column(context, range.start)
    }
}

/// `previous_code_line(line) =~ /\S/`: how far the nearest line above `line` that is neither blank
/// nor a comment of its own is indented.
fn previous_code_line_indent(
    context: &RuleContext<'_>,
    line: usize,
    comment_lines: &[usize],
) -> i64 {
    let mut number = line.min(context.source.line_count() + 1);
    while number > 1 {
        number -= 1;
        let text = context.source.line(number);
        if text.trim_start().is_empty() || comment_lines.contains(&number) {
            continue;
        }
        return line_indentation(context, context.source.line_start(number));
    }
    0
}

/// `special_inner_call_indentation?`: the call is itself an argument, so its own arguments line up
/// under it rather than under the line it was written on.
fn special_inner_call_indentation(context: &RuleContext<'_>, call: Node<'_>, style: &str) -> bool {
    if style == "consistent" {
        return false;
    }
    if style == "consistent_relative_to_receiver" {
        return true;
    }
    let Some(parent) = send_parent(context, call) else {
        return false;
    };
    // `eligible_method_call?` is `(send _ !:[]= ...)`.
    if parent.method == "[]=" {
        return false;
    }
    if !parent.parenthesized && style == "special_for_inner_method_call_in_parentheses" {
        return false;
    }
    // The call must begin inside the parent, otherwise it is the first part of a chain.
    call.start_byte() > parent.start
}

/// What upstream's parser would hang the node off, when that is a `send`.
struct SendParent {
    /// The method it dispatches, which the cop tests against `[]=`.
    method: String,
    /// `parenthesized?`: the call closes with `)`.
    parenthesized: bool,
    start: usize,
}

fn send_parent(context: &RuleContext<'_>, node: Node<'_>) -> Option<SendParent> {
    let parent = node.parent_of(context)?;
    match parent.kind_str() {
        // An argument list is the grammar's own node; upstream hangs an argument off the call.
        "argument_list" => dispatch(context, parent.parent_of(context)?),
        // An index read is a call to `[]`, whose arguments sit directly under it.
        "element_reference" => Some(SendParent {
            method: "[]".to_owned(),
            parenthesized: false,
            start: parent.start_byte(),
        }),
        // The node is the receiver of the call it hangs off.
        "call" => dispatch(context, parent),
        // Every operator dispatches a method upstream, and none of them is parenthesized. The
        // logical ones are the exception: `and` and `or` are nodes of their own there.
        "binary" | "unary" => {
            let operator = parent
                .field("operator")
                .or_else(|| parent.child(0))?;
            let name = context.source.node_text(operator);
            if matches!(name, "&&" | "||" | "and" | "or") {
                return None;
            }
            Some(SendParent {
                method: name.to_owned(),
                parenthesized: false,
                start: parent.start_byte(),
            })
        }
        // Assigning through a reader is a `send` too; assigning to a variable is not.
        "assignment" => {
            let left = parent.field("left")?;
            let method = match left.kind_str() {
                "call" => format!(
                    "{}=",
                    context
                        .source
                        .node_text(left.field("method")?)
                ),
                "element_reference" => "[]=".to_owned(),
                _ => return None,
            };
            Some(SendParent {
                method,
                parenthesized: false,
                start: parent.start_byte(),
            })
        }
        _ => None,
    }
}

/// The call as a `send`. `super(...)` and `yield(...)` are nodes of their own upstream, so neither
/// is one.
fn dispatch(context: &RuleContext<'_>, call: Node<'_>) -> Option<SendParent> {
    if call.kind_str() != "call" {
        return None;
    }
    let method = call.field("method");
    if method.is_some_and(|method| method.kind_str() == "super") {
        return None;
    }
    Some(SendParent {
        // `l.(1)` dispatches `call` without naming it.
        method: method.map_or_else(
            || "call".to_owned(),
            |method| context.source.node_text(method).to_owned(),
        ),
        parenthesized: closing_parenthesis(call).is_some(),
        start: call.start_byte(),
    })
}

/// `node.loc.end`: the parenthesis a call's argument list closes with.
fn closing_parenthesis<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let list = call.field("arguments")?;
    let last = list.child(u32::try_from(list.child_count()).ok()?.checked_sub(1)?)?;
    (last.kind_str() == ")").then_some(last)
}

/// `autocorrect`: the range `AlignmentCorrector` moves, which is the whole receiver chain when the
/// argument alone cannot be pulled far enough left.
fn correction_range(
    context: &RuleContext<'_>,
    call: Node<'_>,
    item: &Range<usize>,
    delta: i64,
    style: &str,
) -> Range<usize> {
    let top = top_level_send(call);
    if should_correct_entire_chain(context, call, top, delta, style) {
        send_range(top, context)
    } else {
        item.clone()
    }
}

/// `find_top_level_send`: the outermost call the given one is the receiver of.
fn top_level_send<'tree>(call: Node<'tree>) -> Node<'tree> {
    let mut top = call;
    while let Some(parent) = top.parent() {
        if parent.kind_str() != "call"
            || parent.field("receiver") != Some(top)
            || parent.field("operator").is_none()
        {
            break;
        }
        top = parent;
    }
    top
}

fn should_correct_entire_chain(
    context: &RuleContext<'_>,
    call: Node<'_>,
    top: Node<'_>,
    delta: i64,
    style: &str,
) -> bool {
    if style != "special_for_inner_method_call_in_parentheses" {
        return false;
    }
    // `inner_call?`: the chain is itself an argument of a call that parenthesizes its arguments.
    if !send_parent(context, top).is_some_and(|parent| parent.parenthesized) {
        return false;
    }
    if display_column(context, call.start_byte()) >= delta.abs() {
        return false;
    }
    top.id() != call.id()
        || closing_parenthesis(top)
            .is_some_and(|paren| begins_its_line(context, paren.start_byte()))
}

/// The `%<base>s` of the message: what the first argument is expected to line up under.
fn base_description(
    context: &RuleContext<'_>,
    call: Node<'_>,
    first: &Range<usize>,
    style: &str,
) -> String {
    let text = context.source.text()[base_range(call, first.start)].trim();
    if !text.contains('\n') && special_inner_call_indentation(context, call, style) {
        return format!("`{text}`");
    }
    let last = text.rsplit('\n').next().unwrap_or(text);
    if last.trim_start().starts_with('#') {
        "the start of the previous line (not counting the comment)".to_owned()
    } else {
        "the start of the previous line".to_owned()
    }
}
