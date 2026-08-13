use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::node_equality::numeric_value;
use crate::rules::send_node;

const MSG: &str = "Pass `__FILE__` and `__LINE__` to `%s`.";
const MSG_EVAL: &str = "Pass a binding, `__FILE__`, and `__LINE__` to `eval`.";
const MSG_INCORRECT_FILE: &str = "Incorrect file for `%s`; use `%s` instead of `%s`.";
const MSG_INCORRECT_LINE: &str = "Incorrect line number for `%s`; use `%s` instead of `%s`.";

/// `RESTRICT_ON_SEND`.
const EVAL_METHODS: &[&str] = &["eval", "class_eval", "module_eval", "instance_eval"];

/// The keywords the parser resolves while it parses, which is why a cop sees a `str` holding the
/// path and an `int` holding the line rather than the keyword itself -- and why upstream tells
/// them from any other literal by the *source* the node was written as.
const FILE_KEYWORD: &str = "__FILE__";
const LINE_KEYWORD: &str = "__LINE__";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(name) = method_name(context, node) else {
            continue;
        };
        if !EVAL_METHODS.contains(&name) || !send_node::is_plain_send(node, context) {
            continue;
        }
        // Classes should not redefine `eval`, but in case one does, only `eval` without a receiver
        // and `Kernel.eval` are considered.
        if name == "eval" && !valid_eval_receiver(context, node) {
            continue;
        }
        let arguments = arguments(node);
        let Some(code) = arguments.first() else {
            continue;
        };
        // The cop works only when a string literal is given as the code string.
        if !is_string_literal(context, code.node) {
            continue;
        }
        let code = code.node;
        check_location(context, offenses, node, name, &arguments, code);
    }
}

fn check_location(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    name: &str,
    arguments: &[Arg<'_>],
    code: Node<'_>,
) {
    // `eval` takes the binding first, so its location arguments start one place further along.
    let base = usize::from(name == "eval") + 1;
    let (file, line) = (arguments.get(base), arguments.get(base + 1));

    if line.is_some() {
        if let Some(file) = file {
            check_file(context, offenses, name, file);
        }
        check_line(context, offenses, name, arguments, code);
    } else if let Some(file) = file {
        check_file(context, offenses, name, file);
        // `add_offense_for_missing_line`.
        let expected = missing_line(context, arguments, code);
        register_offense(
            context,
            offenses,
            node,
            name,
            Some(format!(", {expected}")),
            arguments,
        );
    } else {
        add_offense_for_missing_location(context, offenses, node, name, arguments, code);
    }
}

/// `add_offense_for_missing_location`: without a binding there is nowhere to put the file and the
/// line, so `eval` is reported and left alone.
fn add_offense_for_missing_location(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    name: &str,
    arguments: &[Arg<'_>],
    code: Node<'_>,
) {
    if name == "eval" && arguments.len() < 2 {
        register_offense(context, offenses, node, name, None, arguments);
        return;
    }
    let expected = missing_line(context, arguments, code);
    register_offense(
        context,
        offenses,
        node,
        name,
        Some(format!(", {FILE_KEYWORD}, {expected}")),
        arguments,
    );
}

/// `register_offense`: the call itself, with the arguments it is missing appended after the last
/// one it has.
fn register_offense(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    name: &str,
    appended: Option<String>,
    arguments: &[Arg<'_>],
) {
    let message = match name {
        "eval" => MSG_EVAL.to_owned(),
        _ => MSG.replacen("%s", name, 1),
    };
    let offense = context.offense(message, send_node::send_range(node, context));
    let Some(appended) = appended else {
        offenses.push(offense);
        return;
    };
    let Some(last) = arguments.last() else {
        offenses.push(offense);
        return;
    };
    let at = last.range.end;
    offenses.push(
        offense
            // `insert_after(node.last_argument.source_range.end, ...)` hands the corrector the
            // empty range after the last argument rather than the call it reported.
            .corrections_anchored_at(at..at)
            .corrected_by(Edit {
                start: at,
                end: at,
                replacement: appended,
                safe: true,
            }),
    );
}

fn check_file(context: &RuleContext<'_>, offenses: &mut Vec<Offense>, name: &str, file: &Arg<'_>) {
    let range = file.range.clone();
    let actual = context.source.slice(range.clone());
    if actual == FILE_KEYWORD {
        return;
    }
    let message = MSG_INCORRECT_FILE
        .replacen("%s", name, 1)
        .replacen("%s", FILE_KEYWORD, 1)
        .replacen("%s", actual, 1);
    offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: FILE_KEYWORD.to_owned(),
        safe: true,
    }));
}

/// `check_line`: the line argument has to be `__LINE__` offset by however far the code string
/// starts below it.
///
/// The argument checked is the *last* one rather than the one the file was found next to, which is
/// what upstream reads, and anything whose value cannot be seen -- a variable, or a call that is
/// not an addition -- is left alone.
fn check_line(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    name: &str,
    arguments: &[Arg<'_>],
    code: Node<'_>,
) {
    let Some(line) = arguments.last() else {
        return;
    };
    if is_opaque(context, line.node) {
        return;
    }
    let difference = line_difference(context, line, code);
    let (sign, magnitude) = match difference {
        0 => {
            // `add_offense_for_same_line`.
            if is_line_keyword(context, line) {
                return;
            }
            ("+", 0)
        }
        difference => {
            // `add_offense_for_different_line`.
            let sign = if difference > 0 { "+" } else { "-" };
            if line_with_offset(context, line.node, sign, difference.abs()) {
                return;
            }
            (sign, difference.abs())
        }
    };
    let range = line.range.clone();
    let expected = expected_line(sign, magnitude);
    let message = MSG_INCORRECT_LINE
        .replacen("%s", name, 1)
        .replacen("%s", &expected, 1)
        .replacen("%s", context.source.slice(range.clone()), 1);
    offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: expected,
        safe: true,
    }));
}

/// `missing_line`: what the line argument would have to say if it were written.
fn missing_line(context: &RuleContext<'_>, arguments: &[Arg<'_>], code: Node<'_>) -> String {
    let Some(last) = arguments.last() else {
        return LINE_KEYWORD.to_owned();
    };
    let difference = line_difference(context, last, code);
    let sign = if difference > 0 { "+" } else { "-" };
    expected_line(sign, difference.abs())
}

fn expected_line(sign: &str, magnitude: i64) -> String {
    match magnitude {
        0 => LINE_KEYWORD.to_owned(),
        magnitude => format!("{LINE_KEYWORD} {sign} {magnitude}"),
    }
}

/// `line_difference`: how far below the line argument the code string starts.
fn line_difference(context: &RuleContext<'_>, line: &Arg<'_>, code: Node<'_>) -> i64 {
    let start = first_line(context, code);
    let at = context.source.line_column(line.range.start).0 as i64;
    start - at
}

/// `string_first_line`: where the code the string holds begins, which for a heredoc is its body
/// rather than the marker that opened it.
fn first_line(context: &RuleContext<'_>, code: Node<'_>) -> i64 {
    if code.kind() != "heredoc_beginning" {
        return code.start_position().row as i64 + 1;
    }
    // The grammar starts a heredoc's body where its opening line ends, so the line upstream's
    // `heredoc_body` begins on is always the next one -- which is not the marker's line when an
    // earlier heredoc on the same line pushed this one's body further down.
    match send_node::heredoc_body(code, context) {
        Some(body) => body.start_position().row as i64 + 2,
        None => code.start_position().row as i64 + 2,
    }
}

/// `line_with_offset?`: `__LINE__ + n` or `n + __LINE__`, for the `n` the code string is below.
fn line_with_offset(context: &RuleContext<'_>, node: Node<'_>, sign: &str, magnitude: i64) -> bool {
    if node.kind() != "binary" {
        return false;
    }
    let operator = node
        .child_by_field_name("operator")
        .map(|operator| context.source.node_text(operator));
    if operator != Some(sign) {
        return false;
    }
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return false;
    };
    (is_line_node(context, left) && is_integer(context, right, magnitude))
        || (is_integer(context, left, magnitude) && is_line_node(context, right))
}

fn is_integer(context: &RuleContext<'_>, node: Node<'_>, value: i64) -> bool {
    matches!(node.kind(), "integer" | "unary")
        && numeric_value(node, context).is_some_and(|found| found == value as f64)
}

fn is_line_node(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind() == "identifier" && context.source.node_text(node) == LINE_KEYWORD
}

fn is_line_keyword(context: &RuleContext<'_>, argument: &Arg<'_>) -> bool {
    context.source.slice(argument.range.clone()) == LINE_KEYWORD
}

/// Whether the line argument is one whose value the cop declines to read: a variable, or a call
/// that is not an addition.
fn is_opaque(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        "instance_variable" | "class_variable" | "global_variable" => true,
        // A bare name is either a local variable or a receiverless call, and both are skipped.
        // The two keywords the parser resolves into literals are neither.
        "identifier" => !matches!(context.source.node_text(node), FILE_KEYWORD | LINE_KEYWORD),
        // `a[0]` is `(send a :[] ...)`, whose method is never `+`.
        "element_reference" => true,
        "call" => method_name(context, node) != Some("+"),
        "binary" => {
            node.child_by_field_name("operator")
                .map(|operator| context.source.node_text(operator))
                != Some("+")
        }
        // The parser folds the sign of a numeric literal into the literal, so `-1` is an `int`
        // rather than a call; a sign written against anything else is a call.
        "unary" => numeric_value(node, context).is_none(),
        _ => false,
    }
}

/// `valid_eval_receiver?`: `{ nil? (const {nil? cbase} :Kernel) }`.
fn valid_eval_receiver(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.child_by_field_name("receiver") {
        None => true,
        Some(receiver) => send_node::top_level_constant(receiver, "Kernel", context),
    }
}

/// Whether the node is what upstream's parser calls a `str` or a `dstr`.
fn is_string_literal(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind() {
        // `?a`, an adjacent pair of literals, and a heredoc are all one of the two.
        "string" | "character" | "chained_string" | "heredoc_beginning" => true,
        "identifier" => context.source.node_text(node) == FILE_KEYWORD,
        _ => false,
    }
}

fn method_name<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    node.child_by_field_name("method")
        .map(|method| context.source.node_text(method))
}

/// One argument of the call.
struct Arg<'tree> {
    /// What the argument reads as, for the tests that ask its type. For the tail of an argument
    /// list the grammar mis-read as a multiple assignment this is the `assignment` node, which is
    /// the `lvasgn` upstream's parser puts there.
    node: Node<'tree>,
    range: std::ops::Range<usize>,
}

/// The call's arguments, with the grammar's multiple-assignment misreading undone.
///
/// tree-sitter folds a trailing run of assignable arguments into a single `assignment`:
/// `m.module_eval "x", __FILE__, line = __LINE__` comes out as `"x"` and `(__FILE__, line) =
/// __LINE__`. A multiple assignment cannot appear in an argument list in Ruby -- upstream's parser
/// reads the same text as three arguments whose last is an `lvasgn` -- so a `left_assignment_list`
/// found there is always this misreading, and its leading targets are arguments of their own.
///
/// This is the argument-list twin of the folded optional parameters that `style::parameters` and
/// `lint::parameters` restore; when a third caller needs it, the three belong in `send_node`.
fn arguments<'tree>(call: Node<'tree>) -> Vec<Arg<'tree>> {
    let mut out = Vec::new();
    for argument in send_node::arguments(call) {
        let (node, range) = (argument.first(), argument.range());
        let folded = (argument.parts().len() == 1 && node.kind() == "assignment")
            .then(|| node.child_by_field_name("left"))
            .flatten()
            .filter(|left| left.kind() == "left_assignment_list");
        let Some(left) = folded else {
            out.push(Arg { node, range });
            continue;
        };
        let targets = send_node::named_children(left);
        let Some((last, leading)) = targets.split_last() else {
            out.push(Arg { node, range });
            continue;
        };
        for target in leading {
            out.push(Arg {
                node: *target,
                range: target.byte_range(),
            });
        }
        out.push(Arg {
            node,
            range: last.start_byte()..range.end,
        });
    }
    out
}
