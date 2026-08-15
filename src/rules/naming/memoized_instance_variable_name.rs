use tree_sitter::Node;

use super::support::quoted_content;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str =
    "Memoized variable `{var}` does not match method name `{method}`. Use `@{suggested}` instead.";
const UNDERSCORE_REQUIRED: &str =
    "Memoized variable `{var}` does not start with `_`. Use `@{suggested}` instead.";

/// `INITIALIZE_METHODS`, whose instance variables are not memoization at all.
const INITIALIZE_METHODS: &[&str] = &[
    "initialize",
    "initialize_clone",
    "initialize_copy",
    "initialize_dup",
];

const DYNAMIC_DEFINE_METHODS: &[&str] = &["define_method", "define_singleton_method"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyleForLeadingUnderscores")
        .unwrap_or_else(|| "disallowed".to_owned());
    let safe: bool = context.setting("Safe").unwrap_or(true);

    for node in context.nodes_of_any(&["operator_assignment", "unary"]) {
        if node.kind_str() == "operator_assignment" {
            on_or_asgn(context, offenses, node, &style, safe);
        } else {
            on_defined(context, offenses, node, &style, safe);
        }
    }
}

fn on_or_asgn<'tree>(
    context: &RuleContext<'tree>,
    offenses: &mut Vec<Offense>,
    node: Node<'tree>,
    style: &str,
    safe: bool,
) {
    if context.source.node_text(operator(node)) != "||=" {
        return;
    }
    let Some(lhs) = node
        .field("left")
        .filter(|left| left.kind_str() == "instance_variable")
    else {
        return;
    };
    let Some((definition, method_name)) = find_definition(context, node) else {
        return;
    };
    if !nameable_method(&method_name) || !is_body_tail(definition, node) {
        return;
    }
    let variable = context.source.node_text(lhs);
    if matches(&method_name, variable, style) {
        return;
    }
    report(context, offenses, lhs, variable, &method_name, style, safe);
}

/// `on_defined?`: the three-line form of memoization, whose `defined?`, `return` and assignment are
/// each reported and corrected on their own.
fn on_defined<'tree>(
    context: &RuleContext<'tree>,
    offenses: &mut Vec<Offense>,
    node: Node<'tree>,
    style: &str,
    safe: bool,
) {
    let Some(argument) = defined_argument(context, node) else {
        return;
    };
    let name = context.source.node_text(argument);
    let Some((definition, method_name)) = find_definition(context, node) else {
        return;
    };
    if !nameable_method(&method_name) {
        return;
    }
    let statements = body_statements(definition);
    if statements.len() < 2 {
        return;
    }
    let Some((defined_ivar, return_ivar)) = guard_clause(context, statements[0], name) else {
        return;
    };
    let Some(assigned) = memoizing_assignment(context, statements[statements.len() - 1], name)
    else {
        return;
    };
    if matches(&method_name, name, style) {
        return;
    }
    for target in [defined_ivar, return_ivar, assigned] {
        report(context, offenses, target, name, &method_name, style, safe);
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    target: Node<'_>,
    variable: &str,
    method_name: &str,
    style: &str,
    safe: bool,
) {
    let suggested = suggested_variable(method_name, style);
    let template = if style == "required" && !variable.replacen('@', "", 1).starts_with('_') {
        UNDERSCORE_REQUIRED
    } else {
        MSG
    };
    let message = template
        .replace("{var}", variable)
        .replace("{method}", method_name)
        .replace("{suggested}", &suggested);
    let range = target.byte_range();
    offenses.push(context.offense(message, range.clone()).corrected_by(Edit {
        start: range.start,
        end: range.end,
        replacement: format!("@{suggested}"),
        safe,
    }));
}

/// `find_definition`: the nearest enclosing `def`, `defs` or dynamic definition, with the name it
/// gives the method.
fn find_definition<'tree>(
    context: &RuleContext<'tree>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, String)> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        current = parent;
        match current.kind_str() {
            "method" | "singleton_method" => {
                let name = current.field("name")?;
                return Some((current, context.source.node_text(name).to_owned()));
            }
            "block" | "do_block" => {
                if let Some(name) = dynamic_definition_name(context, current) {
                    return Some((current, name));
                }
            }
            _ => {}
        }
    }
    None
}

/// The name a `define_method(:foo) { ... }` block defines. The receiver may be anything, but the
/// call takes exactly one argument and it has to be a literal string or symbol.
fn dynamic_definition_name(context: &RuleContext<'_>, block: Node<'_>) -> Option<String> {
    let call = block
        .parent_of(context)
        .filter(|parent| parent.kind_str() == "call")?;
    let method = call.field("method")?;
    if !DYNAMIC_DEFINE_METHODS.contains(&context.source.node_text(method)) {
        return None;
    }
    let arguments = call.field("arguments")?;
    let mut cursor = arguments.walk();
    let list: Vec<Node<'_>> = arguments.named_children(&mut cursor).collect();
    let [argument] = list.as_slice() else {
        return None;
    };
    match argument.kind_str() {
        "simple_symbol" => Some(
            context
                .source
                .node_text(*argument)
                .trim_start_matches(':')
                .to_owned(),
        ),
        "delimited_symbol" | "string" | "bare_string" => quoted_content(*argument, context.source),
        _ => None,
    }
}

/// `body == node || body.children.last == node`: the assignment has to be the last thing the
/// method evaluates.
fn is_body_tail(definition: Node<'_>, node: Node<'_>) -> bool {
    let statements = body_statements(definition);
    match statements.as_slice() {
        // Two or more statements are wrapped in a `begin` upstream, whose last child is the last
        // statement.
        [] => false,
        [only] => {
            only.id() == node.id()
                || parser_last_child(*only).is_some_and(|last| last.id() == node.id())
        }
        _ => statements[statements.len() - 1].id() == node.id(),
    }
}

/// The statements of a method or block body, with the `rescue`, `else` and `ensure` clauses that
/// wrap them upstream left out.
fn body_statements<'tree>(definition: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(body) = definition.field("body") else {
        return Vec::new();
    };
    parser_children(body)
        .into_iter()
        .filter(|child| !matches!(child.kind_str(), "rescue" | "else" | "ensure"))
        .collect()
}

/// The children upstream's parser builds a node for.
///
/// A comment is no node at all there, and a heredoc's body belongs to the literal that opened it
/// rather than to the statement list the grammar parks it in -- so counting either makes a body of
/// one statement look like a body of two, and the assignment stops being the last thing the method
/// evaluates.
fn parser_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect()
}

/// `children.last` for the nodes a one-statement body can be. The conditionals are the ones that
/// have to be spelled out: their last child upstream is the `else` branch, which is `nil` when it
/// was not written.
fn parser_last_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "if" | "unless" | "case" | "case_match" => else_statement(node),
        "if_modifier" => None,
        "unless_modifier" | "while_modifier" | "until_modifier" => node.field("body"),
        "while" | "until" => sole_statement(node.field("body")?),
        "call" => match node.field("block") {
            Some(block) => sole_statement(block.field("body")?),
            None => parser_children(node.field("arguments")?).pop(),
        },
        "begin" | "body_statement" | "block_body" | "then" | "else" => parser_children(node)
            .into_iter()
            .rfind(|child| !matches!(child.kind_str(), "rescue" | "else" | "ensure")),
        _ => parser_children(node).pop(),
    }
}

/// The `else` branch of a conditional, which is what upstream reads as its last child. A branch
/// that was never written is `nil` there, and an `elsif` nests another conditional rather than
/// ending the chain, so neither can be the assignment being looked for.
fn else_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let branch = parser_children(node)
        .into_iter()
        .find(|child| child.kind_str() == "else")?;
    sole_statement(branch)
}

/// The single statement a container holds, or nothing when it holds none or several -- several
/// become a `begin` node upstream, which is not the assignment being looked for.
fn sole_statement(container: Node<'_>) -> Option<Node<'_>> {
    let statements = parser_children(container);
    match statements.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The instance variable a `defined?` asks about, which is all this cop looks at.
fn defined_argument<'tree>(context: &RuleContext<'tree>, node: Node<'tree>) -> Option<Node<'tree>> {
    let operator = node.field("operator")?;
    if context.source.node_text(operator) != "defined?" {
        return None;
    }
    let operand = node.field("operand")?;
    let argument = if operand.kind_str() == "parenthesized_statements" {
        sole_statement(operand)?
    } else {
        operand
    };
    (argument.kind_str() == "instance_variable").then_some(argument)
}

/// `(if (defined (ivar %1)) (return (ivar %1)) nil?)`: the guard clause the pattern opens with,
/// and the two instance variables it names.
fn guard_clause<'tree>(
    context: &RuleContext<'tree>,
    node: Node<'tree>,
    name: &str,
) -> Option<(Node<'tree>, Node<'tree>)> {
    let (condition, consequence) = match node.kind_str() {
        "if" => {
            if node.field("alternative").is_some() {
                return None;
            }
            (
                node.field("condition")?,
                sole_statement(node.field("consequence")?)?,
            )
        }
        "if_modifier" => (node.field("condition")?, node.field("body")?),
        _ => return None,
    };
    let defined_ivar = defined_argument(context, condition)
        .filter(|ivar| context.source.node_text(*ivar) == name)?;
    if consequence.kind_str() != "return" {
        return None;
    }
    let arguments = consequence.field("arguments").or_else(|| {
        let mut cursor = consequence.walk();
        consequence.named_children(&mut cursor).next()
    })?;
    let return_ivar = sole_statement(arguments)?;
    (return_ivar.kind_str() == "instance_variable" && context.source.node_text(return_ivar) == name)
        .then_some((defined_ivar, return_ivar))
}

/// `(ivasgn %1 _)`: the assignment the pattern closes with, whose name is what gets corrected.
fn memoizing_assignment<'tree>(
    context: &RuleContext<'tree>,
    node: Node<'tree>,
    name: &str,
) -> Option<Node<'tree>> {
    if node.kind_str() != "assignment" || node.field("right").is_none() {
        return None;
    }
    node.field("left").filter(|left| {
        left.kind_str() == "instance_variable" && context.source.node_text(*left) == name
    })
}

fn operator(node: Node<'_>) -> Node<'_> {
    node.field("operator").unwrap_or(node)
}

fn nameable_method(name: &str) -> bool {
    let bare: String = name.chars().filter(|c| !"!?=".contains(*c)).collect();
    let mut characters = bare.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn matches(method_name: &str, variable: &str, style: &str) -> bool {
    if INITIALIZE_METHODS.contains(&method_name) {
        return true;
    }
    let method_name: String = method_name
        .chars()
        .filter(|c| !"!?=".contains(*c))
        .collect();
    let variable = variable.replacen('@', "", 1);
    let variable = variable.as_str();
    let without = method_name.strip_prefix('_').unwrap_or(&method_name);
    let with = format!("_{method_name}");
    match style {
        "required" => variable == with || (method_name.starts_with('_') && variable == method_name),
        "optional" => variable == method_name || variable == with || variable == without,
        _ => variable == method_name || variable == without,
    }
}

fn suggested_variable(method_name: &str, style: &str) -> String {
    let bare: String = method_name
        .chars()
        .filter(|c| !"!?=".contains(*c))
        .collect();
    if style == "required" {
        format!("_{bare}")
    } else {
        bare
    }
}
