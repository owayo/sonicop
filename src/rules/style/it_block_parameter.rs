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
        match block.field("parameters") {
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
    let mut stack = super::nodes::children(body);
    while let Some(node) = stack.pop() {
        // A nested block owns the implicit parameters written inside it.
        if matches!(node.kind_str(), "block" | "do_block") && node.id() != block.id() {
            continue;
        }
        if node.kind_str() == "identifier" && !locals.is_lvar(node) {
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
        stack.extend(super::nodes::children(node));
    }
    match (highest, has_it) {
        // `node.children[1] == 1`: a block reaching `_2` is not one `it` could stand in for.
        (1, _) => Some(Implicit::Numbered),
        (0, true) => Some(Implicit::It),
        _ => None,
    }
}

/// `find_block_variables`: every read of the name in the block's body.
///
/// `each_descendant` does not visit the node it is called on, and upstream's `node.body` is the one
/// statement itself when the block holds only one -- so a block whose whole body *is* the parameter
/// (`foo { it }`) finds nothing. The `begin` a block of several statements gets does hold them as
/// children, and there the statements themselves are visited.
fn references<'tree>(body: Node<'tree>, name: &str, context: &RuleContext<'_>) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    let statements = super::nodes::children(body);
    let mut stack = match statements.as_slice() {
        [only] => super::nodes::children(*only),
        several => several.to_vec(),
    };
    while let Some(node) = stack.pop() {
        if node.kind_str() == "identifier" && context.source.node_text(node) == name {
            found.push(node);
        }
        stack.extend(super::nodes::children(node));
    }
    found.sort_by_key(|node| node.start_byte());
    found
}
