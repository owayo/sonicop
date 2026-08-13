use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;

/// `BASIC_LITERALS`: the values `simple_method_arg?` reads as "this fold builds a value rather than
/// an object to fill in".
const BASIC_LITERAL_KINDS: &[&str] = &[
    "string",
    "integer",
    "float",
    "simple_symbol",
    "delimited_symbol",
    "complex",
    "rational",
    "true",
    "false",
    "nil",
    "character",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        let Some(block) = node.child_by_field_name("block") else {
            continue;
        };
        let Some(selector) = node.child_by_field_name("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        if !matches!(method, "inject" | "reduce") {
            continue;
        }
        // `(call _ {:inject :reduce} _)`: exactly one argument, and not one that is already the
        // value being folded into.
        let arguments = node
            .child_by_field_name("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let [seed] = arguments.as_slice() else {
            continue;
        };
        if BASIC_LITERAL_KINDS.contains(&strip_sign(*seed).kind()) {
            continue;
        }
        let Some(body) = block.child_by_field_name("body") else {
            continue;
        };
        let edits = match block.child_by_field_name("parameters") {
            Some(parameters) => block_edits(context, &locals, selector, parameters, body),
            // Before 2.7 a `_1` was a receiverless call, so the block took no parameters at all.
            None if context.target_ruby_version() >= RubyVersion::new(2, 7) => {
                numbered_edits(context, selector, body)
            }
            None => None,
        };
        let Some(edits) = edits else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    format!("Use `each_with_object` instead of `{method}`."),
                    selector.byte_range(),
                )
                .corrected_by_all(edits),
        );
    }
}

/// `(block $(call ...) $(args _ _) $_)`: two parameters, the first of which the block hands back.
fn block_edits(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_>,
    selector: Node<'_>,
    parameters: Node<'_>,
    body: Node<'_>,
) -> Option<Vec<Edit>> {
    let written = super::nodes::children(parameters);
    let [accumulator, element] = written.as_slice() else {
        return None;
    };
    if accumulator.kind() != "identifier" {
        return None;
    }
    let name = context.source.node_text(*accumulator);
    let returned = return_value(body)?;
    if !locals.is_lvar(returned) || context.source.node_text(returned) != name {
        return None;
    }
    if assigns(context, body, name) {
        return None;
    }
    let mut edits = vec![
        rename(selector),
        Edit {
            start: accumulator.start_byte(),
            end: accumulator.end_byte(),
            replacement: context.source.node_text(*element).to_owned(),
            safe: true,
        },
        Edit {
            start: element.start_byte(),
            end: element.end_byte(),
            replacement: name.to_owned(),
            safe: true,
        },
    ];
    edits.push(drop_returned(context, returned));
    Some(edits)
}

/// `(numblock $(call ...) 2 $_)`: the same fold written with `_1` and `_2`.
fn numbered_edits(
    context: &RuleContext<'_>,
    selector: Node<'_>,
    body: Node<'_>,
) -> Option<Vec<Edit>> {
    if numbered_parameters(context, body) != 2 {
        return None;
    }
    let returned = return_value(body)?;
    if context.source.node_text(returned) != "_1" {
        return None;
    }
    let mut edits = vec![rename(selector)];
    // `each_descendant`: the body itself is not among the nodes upstream renames.
    for child in super::nodes::children(body) {
        collect_swaps(context, child, &mut edits);
    }
    Some(edits)
}

fn rename(selector: Node<'_>) -> Edit {
    Edit {
        start: selector.start_byte(),
        end: selector.end_byte(),
        replacement: "each_with_object".to_owned(),
        safe: true,
    }
}

fn collect_swaps(context: &RuleContext<'_>, node: Node<'_>, edits: &mut Vec<Edit>) {
    if node.kind() == "identifier" {
        let replacement = match context.source.node_text(node) {
            "_1" => "_2",
            "_2" => "_1",
            _ => return,
        };
        edits.push(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: replacement.to_owned(),
            safe: true,
        });
        return;
    }
    for child in super::nodes::children(node) {
        collect_swaps(context, child, edits);
    }
}

/// The accumulator handed back at the end of the block, which `each_with_object` returns on its own.
fn return_value<'tree>(body: Node<'tree>) -> Option<Node<'tree>> {
    super::nodes::children(body).last().copied()
}

/// `accumulator_param_assigned_to?`: a fold that reassigns its accumulator is not the same as
/// filling one in.
fn assigns(context: &RuleContext<'_>, body: Node<'_>, name: &str) -> bool {
    super::super::send_node::any_descendant(body, &mut |node| {
        node.kind() == "assignment"
            && node.child_by_field_name("left").is_some_and(|left| {
                left.kind() == "identifier" && context.source.node_text(left) == name
            })
    })
}

/// How many numbered parameters the block reads, which is what upstream records on a `numblock`.
fn numbered_parameters(context: &RuleContext<'_>, body: Node<'_>) -> usize {
    let mut highest = 0;
    scan(context, body, &mut highest);
    highest
}

fn scan(context: &RuleContext<'_>, node: Node<'_>, highest: &mut usize) {
    for child in super::nodes::children(node) {
        // A nested block's numbered parameters are its own.
        if matches!(child.kind(), "block" | "do_block" | "lambda") {
            continue;
        }
        if child.kind() == "identifier" {
            let name = context.source.node_text(child).as_bytes();
            if name.len() == 2 && name[0] == b'_' && name[1].is_ascii_digit() && name[1] != b'0' {
                *highest = (*highest).max(usize::from(name[1] - b'0'));
            }
            continue;
        }
        scan(context, child, highest);
    }
}

/// The statement that hands the accumulator back goes, along with its line when it had one to
/// itself.
fn drop_returned(context: &RuleContext<'_>, returned: Node<'_>) -> Edit {
    let range = returned.byte_range();
    let first = context.source.line_column(range.start).0;
    let last = context.source.line_column(range.end).0;
    let whole = context.source.line_start(first)..context.source.line_range(last).end;
    match context.source.slice(whole.clone()).trim() == context.source.node_text(returned) {
        true => Edit {
            start: whole.start,
            end: whole.end,
            replacement: String::new(),
            safe: true,
        },
        false => Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        },
    }
}

/// `-1` is one `int` upstream, so a signed literal is a basic literal too.
fn strip_sign<'tree>(node: Node<'tree>) -> Node<'tree> {
    match node.kind() {
        "unary" => node.child_by_field_name("operand").unwrap_or(node),
        _ => node,
    }
}
