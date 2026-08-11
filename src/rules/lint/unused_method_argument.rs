use tree_sitter::Node;

use super::variable_force::{Analysis, Argument, Declaration, Scope, Variable};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_unused_keywords: bool = context
        .setting("AllowUnusedKeywordArguments")
        .unwrap_or(false);
    let ignore_empty: bool = context.setting("IgnoreEmptyMethods").unwrap_or(true);
    let ignore_not_implemented: bool = context
        .setting("IgnoreNotImplementedMethods")
        .unwrap_or(true);
    let not_implemented_exceptions: Vec<String> = context
        .setting("NotImplementedExceptions")
        .unwrap_or_default();
    let analysis = Analysis::run(context.root_node(), context.source);
    for scope in &analysis.scopes {
        if !matches!(scope.node.kind(), "method" | "singleton_method") {
            continue;
        }
        let body = method_body(scope.node);
        // `ignored_method?`: a method with nothing in it, or one that exists only to announce that
        // a subclass has to supply the implementation, names its parameters for the signature
        // rather than to use them.
        if ignore_empty && body.is_none() {
            continue;
        }
        if ignore_not_implemented
            && body.and_then(|body| body.single).is_some_and(|statement| {
                not_implemented(statement, context, &analysis, &not_implemented_exceptions)
            })
        {
            continue;
        }
        for &index in &scope.variables {
            let variable = &analysis.variables[index];
            if !variable.is_argument()
                || (allow_unused_keywords && keyword_argument(variable))
                || block_argument_with_yield(variable, body)
                || variable.should_be_unused()
                || variable.referenced
            {
                continue;
            }
            let message = message(context, &analysis, scope, variable);
            let offense = context.offense(message, variable.name_node.byte_range());
            offenses.push(match correction(context, variable) {
                Some(edit) => offense.corrected_by(edit),
                None => offense,
            });
        }
    }
}

/// What the third child of a `def` holds upstream: nothing when the method is empty, the sole
/// statement when there is one, and the `begin` wrapping them when there are several.
///
/// The distinction matters twice over: `IgnoreEmptyMethods` asks whether the body is there at all,
/// and the `raise NotImplementedError` pattern only matches a body that is that single call.
#[derive(Clone, Copy)]
struct MethodBody<'tree> {
    /// The whole body, which the `yield` search scans.
    region: Node<'tree>,
    /// The one statement the body holds, when it holds exactly one.
    single: Option<Node<'tree>>,
}

/// Named children of a statement list that are not statements: a `;`, and the two kinds the
/// grammar hangs off the statement that mentioned them rather than off the expression itself. A
/// method whose one statement carries a trailing comment still has one statement.
const NOT_A_STATEMENT: &[&str] = &["empty_statement", "comment", "heredoc_body"];

fn method_body(node: Node<'_>) -> Option<MethodBody<'_>> {
    let body = node.child_by_field_name("body")?;
    // An endless method keeps its expression directly under `body`, with no statement list.
    if body.kind() != "body_statement" {
        return Some(MethodBody {
            region: body,
            single: Some(body),
        });
    }
    let mut cursor = body.walk();
    let statements: Vec<Node<'_>> = body
        .named_children(&mut cursor)
        .filter(|child| !NOT_A_STATEMENT.contains(&child.kind()))
        .collect();
    match statements.len() {
        0 => None,
        1 => Some(MethodBody {
            region: statements[0],
            single: Some(statements[0]),
        }),
        _ => Some(MethodBody {
            region: body,
            single: None,
        }),
    }
}

fn keyword_argument(variable: &Variable<'_>) -> bool {
    variable.kind == Declaration::Argument(Argument::Keyword)
}

/// An explicit `&block` that the body reaches through `yield` instead of by name. The parameter is
/// then documentation of the method's interface rather than a variable nothing uses.
fn block_argument_with_yield(variable: &Variable<'_>, body: Option<MethodBody<'_>>) -> bool {
    if variable.kind != Declaration::Argument(Argument::Block) {
        return false;
    }
    body.is_some_and(|body| contains_yield(body.region))
}

fn contains_yield(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let mut depth = 0usize;
    loop {
        if cursor.node().kind() == "yield" {
            return true;
        }
        if cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if depth == 0 {
                return false;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            cursor.goto_parent();
            depth -= 1;
        }
    }
}

/// `not_implemented?`: `raise` of one of the configured exception classes, or a bare `fail`.
fn not_implemented(
    node: Node<'_>,
    context: &RuleContext<'_>,
    analysis: &Analysis<'_>,
    exceptions: &[String],
) -> bool {
    // A receiverless call with no arguments reaches tree-sitter as a bare identifier, so `fail`
    // written on its own has no call node to inspect.
    if node.kind() == "identifier" {
        return context.source.node_text(node) == "fail" && !analysis.is_variable_reference(node);
    }
    if node.kind() != "call" || node.child_by_field_name("receiver").is_some() {
        return false;
    }
    let Some(method) = node.child_by_field_name("method") else {
        return false;
    };
    match context.source.node_text(method) {
        "fail" => true,
        "raise" => node
            .child_by_field_name("arguments")
            .and_then(|arguments| arguments.named_child(0))
            .and_then(|argument| const_name(argument, context))
            .is_some_and(|name| exceptions.iter().any(|exception| *exception == name)),
        _ => false,
    }
}

/// `Node#const_name`. A leading `::` names the same constant, so `::NotImplementedError` reads as
/// `NotImplementedError`, while a namespace that is not itself a constant contributes nothing.
fn const_name(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let name = match node.kind() {
        "constant" => return Some(context.source.node_text(node).to_owned()),
        "scope_resolution" => context.source.node_text(node.child_by_field_name("name")?),
        _ => return None,
    };
    match node.child_by_field_name("scope") {
        Some(scope) => Some(format!(
            "{}::{name}",
            const_name(scope, context).unwrap_or_default()
        )),
        None => Some(name.to_owned()),
    }
}

fn message(
    context: &RuleContext<'_>,
    analysis: &Analysis<'_>,
    scope: &Scope<'_>,
    variable: &Variable<'_>,
) -> String {
    let name = &variable.name;
    let mut message = format!("Unused method argument - `{name}`.");
    if !keyword_argument(variable) {
        message.push_str(&format!(
            " If it's necessary, use `_` or `_{name}` as an argument name to indicate that it \
             won't be used. If it's unnecessary, remove it."
        ));
    }
    let none_referenced = scope
        .variables
        .iter()
        .map(|&index| &analysis.variables[index])
        .filter(|variable| variable.is_argument())
        .all(|argument| !argument.referenced);
    if none_referenced {
        let method = scope
            .node
            .child_by_field_name("name")
            .map_or("", |node| context.source.node_text(node));
        message.push_str(&format!(
            " You can also write as `{method}(*)` if you want the method to accept any arguments \
             but don't care about them."
        ));
    }
    message
}

/// `UnusedArgCorrector`. A keyword argument cannot be renamed without renaming the keyword, and an
/// unused block argument is surplus rather than misnamed, so it is deleted instead.
fn correction(context: &RuleContext<'_>, variable: &Variable<'_>) -> Option<Edit> {
    match variable.kind {
        Declaration::Argument(Argument::Keyword) => None,
        Declaration::Argument(Argument::Block) => {
            let start = removal_start(context, variable.declaration.start_byte());
            Some(Edit {
                start,
                end: variable.declaration.end_byte(),
                replacement: String::new(),
                safe: true,
            })
        }
        _ => Some(Edit {
            start: variable.name_node.start_byte(),
            end: variable.name_node.start_byte(),
            replacement: "_".to_owned(),
            safe: true,
        }),
    }
}

/// Walks back over the whitespace and then the comma that separated the argument from the one
/// before it, so deleting the argument does not leave `(a, )` behind.
fn removal_start(context: &RuleContext<'_>, start: usize) -> usize {
    let text = context.source.text().as_bytes();
    let mut cursor = start;
    while cursor > 0 && (text[cursor - 1] == b' ' || text[cursor - 1] == b'\t') {
        cursor -= 1;
    }
    if cursor > 0 && text[cursor - 1] == b',' {
        cursor -= 1;
    }
    cursor
}
