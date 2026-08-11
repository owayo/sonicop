use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::{RuleContext, walk_named};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ignore_empty: bool = context.setting("IgnoreEmptyBlocks").unwrap_or(true);
    let allow_unused_keywords: bool = context
        .setting("AllowUnusedKeywordArguments")
        .unwrap_or(false);
    for node in context.nodes_of_any(&["block", "do_block", "lambda"]) {
        // A lambda literal owns a `block` node holding its braces; the parameters belong to the
        // `lambda` above it, so the inner node is not a scope of its own.
        if node
            .parent()
            .is_some_and(|parent| parent.kind() == "lambda")
        {
            continue;
        }
        inspect_scope(context, offenses, node, ignore_empty, allow_unused_keywords);
    }
}

/// One parameter as the cop reports it: the declaration it can rewrite, the name it points at, and
/// the two traits that change how it is treated.
struct Parameter<'tree> {
    declaration: Node<'tree>,
    name: Node<'tree>,
    /// Declared after the `;` in `|a; b|`, which RuboCop calls a block local variable.
    local: bool,
    keyword: bool,
}

fn inspect_scope(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    ignore_empty: bool,
    allow_unused_keywords: bool,
) {
    let Some(list) = node.child_by_field_name("parameters") else {
        return;
    };
    let body = scope_body(node);
    if ignore_empty && body.is_none() {
        return;
    }
    let parameters = collect_parameters(list);
    if parameters.is_empty() {
        return;
    }
    // A `binding` call hands the whole scope to the caller, so RuboCop's variable table marks
    // every variable in reach as referenced and the cop has nothing left to report.
    if body.is_some_and(|body| calls_binding(context, body)) {
        return;
    }

    let referenced: Vec<bool> = parameters
        .iter()
        .map(|parameter| {
            body.is_some_and(|body| {
                is_referenced(context, body, context.source.node_text(parameter.name))
            })
        })
        .collect();
    let none_referenced = !referenced.iter().any(|used| *used);
    let lambda = is_lambda(context, node);
    let define_method = is_define_method(context, node);

    for (parameter, used) in parameters.iter().zip(&referenced) {
        let name = context.source.node_text(parameter.name);
        if *used || name.starts_with('_') || (parameter.keyword && allow_unused_keywords) {
            continue;
        }
        // A block local that is written to is doing its job -- it exists precisely so the name
        // stays out of the enclosing scope -- so only an untouched one is reported.
        if parameter.local && body.is_some_and(|body| is_assigned(context, body, name)) {
            continue;
        }
        let message = if parameter.local {
            format!("Unused block local variable - `{name}`.")
        } else {
            let augmentation = if lambda {
                lambda_message(name, none_referenced)
            } else if none_referenced && !define_method {
                omit_message(parameters.len())
            } else {
                underscore_message(name)
            };
            format!("Unused block argument - `{name}`. {augmentation}")
        };
        let offense = context.offense(message, parameter.name.byte_range());
        offenses.push(match correction(context, parameter) {
            Some(edit) => offense.corrected_by(edit),
            None => offense,
        });
    }
}

/// The statements the parameters are in scope for. A lambda literal keeps them one level down,
/// inside the `block` that holds its braces.
fn scope_body<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let body = node.child_by_field_name("body")?;
    if node.kind() == "lambda" {
        body.child_by_field_name("body")
    } else {
        Some(body)
    }
}

fn collect_parameters(list: Node<'_>) -> Vec<Parameter<'_>> {
    let mut parameters = Vec::new();
    let mut cursor = list.walk();
    if !cursor.goto_first_child() {
        return parameters;
    }
    loop {
        let node = cursor.node();
        if node.is_named() {
            let local = cursor.field_name() == Some("locals");
            push_parameter(node, local, &mut parameters);
        }
        if !cursor.goto_next_sibling() {
            return parameters;
        }
    }
}

fn push_parameter<'tree>(node: Node<'tree>, local: bool, parameters: &mut Vec<Parameter<'tree>>) {
    match node.kind() {
        "identifier" => parameters.push(Parameter {
            declaration: node,
            name: node,
            local,
            keyword: false,
        }),
        // `|(a, b)|` declares each element separately, and each is reported on its own.
        "destructured_parameter" => {
            let mut cursor = node.walk();
            let children: Vec<Node<'tree>> = node.named_children(&mut cursor).collect();
            for child in children {
                push_parameter(child, local, parameters);
            }
        }
        _ => {
            if let Some(name) = node.child_by_field_name("name") {
                parameters.push(Parameter {
                    declaration: node,
                    name,
                    local,
                    keyword: node.kind() == "keyword_parameter",
                });
            }
        }
    }
}

fn is_referenced(context: &RuleContext<'_>, body: Node<'_>, name: &str) -> bool {
    let mut referenced = false;
    walk_named(body, &mut |candidate| {
        if candidate.kind() == "identifier"
            && context.source.node_text(candidate) == name
            && !is_assignment_target(candidate)
        {
            referenced = true;
        }
    });
    referenced
}

fn is_assigned(context: &RuleContext<'_>, body: Node<'_>, name: &str) -> bool {
    let mut assigned = false;
    walk_named(body, &mut |candidate| {
        if candidate.kind() == "identifier"
            && context.source.node_text(candidate) == name
            && is_assignment_target(candidate)
        {
            assigned = true;
        }
    });
    assigned
}

/// Whether the identifier is being written rather than read. RuboCop counts only reads as
/// references, so `do |x| x = 1 end` still reports `x`. An operator assignment such as `x += 1`
/// reads the variable first, so it is deliberately not treated as a bare write.
fn is_assignment_target(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let target = match parent.kind() {
        "assignment" => parent,
        "left_assignment_list" => match parent.parent() {
            Some(grandparent) if grandparent.kind() == "assignment" => grandparent,
            _ => return false,
        },
        _ => return false,
    };
    target
        .child_by_field_name("left")
        .is_some_and(|left| left == node || left == parent)
}

fn calls_binding(context: &RuleContext<'_>, body: Node<'_>) -> bool {
    let mut found = false;
    walk_named(body, &mut |candidate| {
        if candidate.kind() == "identifier" && context.source.node_text(candidate) == "binding" {
            found = true;
        }
    });
    found
}

/// `->() {}` and `lambda {}` are lambdas; `proc {}` and `Proc.new {}` are not, and get the
/// ordinary block wording.
fn is_lambda(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if node.kind() == "lambda" {
        return true;
    }
    block_method(context, node) == Some("lambda")
}

fn is_define_method(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    block_method(context, node) == Some("define_method")
}

fn block_method<'a>(context: &'a RuleContext<'_>, node: Node<'_>) -> Option<&'a str> {
    let call = node.parent().filter(|parent| parent.kind() == "call")?;
    Some(
        context
            .source
            .node_text(call.child_by_field_name("method")?),
    )
}

fn underscore_message(name: &str) -> String {
    format!(
        "If it's necessary, use `_` or `_{name}` as an argument name to indicate that it won't be used."
    )
}

fn omit_message(count: usize) -> String {
    if count > 1 {
        "You can omit all the arguments if you don't care about them.".to_owned()
    } else {
        "You can omit the argument if you don't care about it.".to_owned()
    }
}

fn lambda_message(name: &str, none_referenced: bool) -> String {
    let mut message = underscore_message(name);
    if none_referenced {
        message.push_str(
            " Also consider using a proc without arguments instead of a lambda if you want it \
             to accept any arguments but don't care about them.",
        );
    }
    message
}

/// RuboCop's `UnusedArgCorrector` leaves keyword arguments alone -- prefixing one would rename the
/// keyword itself -- and deletes an explicit block argument instead of renaming it, since an
/// unused `&block` is simply surplus.
fn correction(context: &RuleContext<'_>, parameter: &Parameter<'_>) -> Option<Edit> {
    if parameter.keyword {
        return None;
    }
    if parameter.declaration.kind() == "block_parameter" {
        let start = removal_start(context, parameter.declaration.start_byte());
        return Some(Edit {
            start,
            end: parameter.declaration.end_byte(),
            replacement: String::new(),
            safe: true,
        });
    }
    Some(Edit {
        start: parameter.name.start_byte(),
        end: parameter.name.start_byte(),
        replacement: "_".to_owned(),
        safe: true,
    })
}

/// Walks back over the whitespace and then the comma that separated the argument from the one
/// before it, so deleting the argument does not leave `|a, |` behind.
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
