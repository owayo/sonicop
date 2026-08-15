//! `Style/ItBlockParameter`: when a block's one parameter should be the implicit `it`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

const MSG_USE_IT_PARAMETER: &str = "Use `it` block parameter.";
const MSG_AVOID_IT_PARAMETER: &str = "Avoid using `it` block parameter.";
const MSG_AVOID_IT_PARAMETER_MULTILINE: &str =
    "Avoid using `it` block parameter for multi-line blocks.";

/// `minimum_target_ruby_version 3.4`: `it` became the implicit parameter in 3.4.
const MINIMUM: RubyVersion = RubyVersion::new(3, 4);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "allow_single_line".to_owned());
    let locals = LocalVariables::new(context);
    for block in context.nodes_of_any(&["block", "do_block"]) {
        let Some(body) = block.field("body") else {
            continue;
        };
        // `-> x { }` is one `block` upstream, where the grammar writes the parameters on the
        // `lambda` and the braces on a block of their own.
        let written = block.field("parameters").or_else(|| {
            context
                .parent(block)
                .filter(|parent| parent.kind_str() == "lambda")
                .and_then(|parent| parent.field("parameters"))
        });
        match written {
            // `on_block`: only the `always` style asks a named parameter to become `it`.
            Some(parameters) => {
                if style != "always" {
                    continue;
                }
                let names = super::nodes::children(parameters);
                let [only] = names.as_slice() else {
                    continue;
                };
                if only.kind_str() != "identifier" {
                    continue;
                }
                let name = context.source.node_text(*only);
                for reference in references(body, name, context) {
                    offenses.push(
                        context
                            .offense(MSG_USE_IT_PARAMETER, reference.byte_range())
                            .corrected_by_all([
                                Edit {
                                    start: parameters.start_byte(),
                                    end: parameters.end_byte(),
                                    replacement: String::new(),
                                    safe: true,
                                },
                                Edit {
                                    start: reference.start_byte(),
                                    end: reference.end_byte(),
                                    replacement: "it".to_owned(),
                                    safe: true,
                                },
                            ]),
                    );
                }
            }
            None => match implicit_parameter(block, body, context, &locals) {
                // `on_numblock`: `_1` is what `it` replaced.
                Some(Implicit::Numbered) => {
                    if style == "disallow" {
                        continue;
                    }
                    for reference in references(body, "_1", context) {
                        offenses.push(
                            context
                                .offense(MSG_USE_IT_PARAMETER, reference.byte_range())
                                .corrected_by(Edit {
                                    start: reference.start_byte(),
                                    end: reference.end_byte(),
                                    replacement: "it".to_owned(),
                                    safe: true,
                                }),
                        );
                    }
                }
                // `on_itblock`: the two styles that have something to say about `it` itself.
                Some(Implicit::It) => match style.as_str() {
                    "allow_single_line" => {
                        // Upstream reports the `block` node, which begins at the call rather than
                        // at the brace.
                        let whole = context.parent(block).unwrap_or(block);
                        if whole.start_position().row != whole.end_position().row {
                            offenses.push(
                                context
                                    .offense(MSG_AVOID_IT_PARAMETER_MULTILINE, whole.byte_range()),
                            );
                        }
                    }
                    "disallow" => {
                        for reference in references(body, "it", context) {
                            offenses.push(
                                context.offense(MSG_AVOID_IT_PARAMETER, reference.byte_range()),
                            );
                        }
                    }
                    _ => {}
                },
                None => {}
            },
        }
    }
}

/// Which implicit parameter a block with no parameter list uses.
enum Implicit {
    Numbered,
    It,
}

/// The parser decides between a numbered block and an `it` block from what the body reads, and a
/// numbered one only counts here when `_1` is the highest it goes.
fn implicit_parameter(
    block: Node<'_>,
    body: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Implicit> {
    let mut highest = 0_u32;
    let mut has_it = false;
    let mut stack = readable_children(body);
    while let Some(node) = stack.pop() {
        // A nested block owns the implicit parameters written inside it.
        if matches!(node.kind_str(), "block" | "do_block" | "lambda") && node.id() != block.id() {
            continue;
        }
        if node.kind_str() == "identifier"
            && !locals.is_lvar(node)
            && is_variable_read(node, context)
        {
            match context.source.node_text(node) {
                "it" => has_it = true,
                name => {
                    if let Some(digit) = name
                        .strip_prefix('_')
                        .and_then(|rest| rest.parse::<u32>().ok())
                        .filter(|digit| (1..=9).contains(digit))
                    {
                        highest = highest.max(digit);
                    }
                }
            }
        }
        stack.extend(readable_children(node));
    }
    match (highest, has_it) {
        // `node.children[1] == 1`: a block reaching `_2` is not one `it` could stand in for.
        (1, _) => Some(Implicit::Numbered),
        (0, true) => Some(Implicit::It),
        _ => None,
    }
}

/// The children a name can be *read* from: all of them but the one that names a call and the one
/// an assignment writes to.
///
/// The parser only makes `it` the block's parameter where a bare `it` would otherwise be a
/// receiverless call taking nothing, so the `it` of `it "example" do ... end` is a method name and
/// no parameter at all. A name being assigned is an `lvasgn` rather than an `lvar` for the same
/// reason, which is what tells `it = 1` (a variable from there on) from `it += 1` (a read of the
/// parameter, and then a write).
fn readable_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let skipped = match node.kind_str() {
        "call" => node.field("method").map(|method| method.id()),
        "assignment" | "operator_assignment" => node
            .field("left")
            .filter(|left| left.kind_str() == "identifier")
            .map(|left| left.id()),
        // What a nested block or definition declares is an `arg` there, not an `lvar`, however
        // much the name looks like the one being read.
        "block" | "do_block" | "lambda" | "method" | "singleton_method" => {
            node.field("parameters").map(|parameters| parameters.id())
        }
        _ => None,
    };
    let names_a_target = node.kind_str() == "left_assignment_list";
    // A heredoc's body is spelled after the statement that opened it and the grammar leaves it
    // there, but upstream's parser holds it inside the literal -- so what it interpolates is part
    // of the block and the names read there are the block's.
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind_str() != "comment")
        .filter(|child| {
            Some(child.id()) != skipped && !(names_a_target && child.kind_str() == "identifier")
        })
        .collect()
}

/// `find_block_variables`: every read of the name in the block's body.
///
/// `each_descendant` does not visit the node it is called on, and upstream's `node.body` is the one
/// statement itself when the block holds only one -- so a block whose whole body *is* the parameter
/// (`foo { it }`) finds nothing. The `begin` a block of several statements gets does hold them as
/// children, and there the statements themselves are visited.
fn references<'tree>(body: Node<'tree>, name: &str, context: &RuleContext<'_>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    // A heredoc's body is parked beside the statement that opened it rather than inside it, and
    // upstream has no node for it at all, so the two together are the one statement there.
    let children = readable_children(body);
    let (heredocs, statements): (Vec<Node<'tree>>, Vec<Node<'tree>>) = children
        .into_iter()
        .partition(|child| child.kind_str() == "heredoc_body");
    let mut stack = match statements.as_slice() {
        [only] => readable_children(*only),
        several => several.to_vec(),
    };
    stack.extend(heredocs);
    while let Some(node) = stack.pop() {
        if node.kind_str() == "identifier"
            && context.source.node_text(node) == name
            && is_variable_read(node, context)
        {
            found.push(node);
        }
        // `{ name: }` is `(pair (sym :name) (lvar :name))` there, and the value the parser filled
        // in stands exactly where the key is written.
        if node.kind_str() == "pair"
            && node.field("value").is_none()
            && let Some(key) = node.field("key")
            && key.kind_str() == "hash_key_symbol"
            && context.source.node_text(key) == name
        {
            found.push(key);
        }
        stack.extend(readable_children(node));
    }
    found.sort_by_key(|node| node.start_byte());
    found
}

/// Whether the name is read as a variable, which is what `each_descendant(:lvar)` asks.
///
/// The grammar writes a method's name with the same node it writes a variable with, so the `it` a
/// specification names its examples with (`it 'works' do ... end`) reads as the implicit parameter
/// unless the call is told apart. Only the name is the call -- an implicit parameter standing there
/// as the receiver, as `it.round` does, is still a read.
fn is_variable_read(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.parent_of(context).is_none_or(|parent| {
        parent.kind_str() != "call"
            || parent
                .field("method")
                .is_none_or(|method| method.id() != node.id())
    })
}
