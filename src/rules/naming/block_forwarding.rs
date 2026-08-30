use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::{push_named_children, push_named_children_in};
use crate::rules::send_node::send_range;
use crate::rules::send_node::named_children_of;

/// `minimum_target_ruby_version 3.1`: anonymous block forwarding is 3.1 syntax.
const MINIMUM: RubyVersion = RubyVersion::new(3, 1);

/// The Ruby versions that reject an anonymous `&` inside a block, which is what makes the whole
/// definition unsafe to rewrite.
const ANONYMOUS_IN_BLOCK_REJECTED_THROUGH: RubyVersion = RubyVersion::new(3, 3);

/// The parameter kinds a nested block or method introduces, whose names are bindings of their own
/// rather than reads of the one this cop is following.
const PARAMETER_LISTS: &[&str] = &[
    "block_parameters",
    "method_parameters",
    "lambda_parameters",
    "destructured_parameter",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let anonymous = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "anonymous".to_owned())
        != "explicit";
    let forwarding_name = context
        .setting::<String>("BlockForwardingName")
        .unwrap_or_else(|| "block".to_owned());
    let message = match anonymous {
        true => "Use anonymous block forwarding.",
        false => "Use explicit block forwarding.",
    };
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(parameters) = node.field("parameters") else {
            continue;
        };
        let Some(last) = named_children_of(parameters, context).last().copied() else {
            continue;
        };
        if expected_style(node, last, anonymous, context) {
            continue;
        }
        let Some(forwarded) = forwarded_arguments(node, last, context) else {
            continue;
        };
        for argument in forwarded {
            offenses.push(register(
                argument,
                node,
                anonymous,
                &forwarding_name,
                message,
                context,
            ));
        }
        offenses.push(register(
            last,
            node,
            anonymous,
            &forwarding_name,
            message,
            context,
        ));
    }
}

/// `expected_block_forwarding_style?`: whether the definition already forwards the way the style
/// asks, or is one the cop leaves alone.
fn expected_style(
    node: Node<'_>,
    last: Node<'_>,
    anonymous: bool,
    context: &RuleContext<'_>,
) -> bool {
    if !anonymous {
        return !is_anonymous_block_parameter(last);
    }
    if !is_explicit_block_parameter(last) {
        return true;
    }
    // An anonymous `&` cannot be forwarded past a keyword argument, and a name the body reads as a
    // variable cannot be taken away.
    uses_keyword_parameter(node) || reads_as_variable(node, block_parameter_name(last, context), context)
}

/// `node.each_descendant(:block_pass)` filtered by `block_argument_name_matched?`, or `None` when
/// one of them makes the whole definition unsafe to rewrite.
fn forwarded_arguments<'tree>(
    node: Node<'tree>,
    last: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Vec<Node<'tree>>> {
    let unsafe_in_block = context.target_ruby_version() <= ANONYMOUS_IN_BLOCK_REJECTED_THROUGH;
    let name = context.source.node_text(last);
    let mut found = Vec::new();
    let mut stack = Vec::new();
    push_named_children_in(node, context, &mut stack);
    while let Some(current) = stack.pop() {
        if current.kind_str() == "block_argument" {
            // `invalidates_syntax?` is asked of every block pass, matching or not, and aborts the
            // whole definition.
            if unsafe_in_block && inside_a_block(current, context) {
                return None;
            }
            // `&:sym` forwards a symbol rather than the block parameter.
            let symbolic = named_children_of(current, context).first().is_some_and(|child| {
                matches!(child.kind_str(), "simple_symbol" | "delimited_symbol")
            });
            if !symbolic && context.source.node_text(current) == name {
                found.push(current);
            }
        }
        push_named_children_in(current, context, &mut stack);
    }
    Some(found)
}

/// `block_pass_node.each_ancestor(:any_block).any?`.
///
/// A call written with a block is two nodes upstream -- a `block` wrapped around the `send` -- and
/// one node here, so everything the call holds sits inside that `block` there: its receiver and its
/// arguments as much as the block body. `TestTimer.new(x, &block).tap { }` is the shape that makes
/// the difference, where the block pass is an argument of a call the block hangs off.
fn inside_a_block(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent_of(context) {
        let is_block = match parent.kind_str() {
            "block" | "do_block" | "lambda" => true,
            "call" => parent.field("block").is_some(),
            _ => false,
        };
        if is_block {
            return true;
        }
        current = parent;
    }
    false
}

/// `register_offense`.
fn register(
    block_argument: Node<'_>,
    node: Node<'_>,
    anonymous: bool,
    forwarding_name: &str,
    message: &'static str,
    context: &RuleContext<'_>,
) -> Offense {
    let offense = context.offense(message, block_argument.byte_range());
    if !anonymous {
        // The explicit style cannot name the parameter after something the body already uses.
        if reads_as_variable(node, Some(forwarding_name), context) {
            return offense;
        }
        return offense.corrected_by(Edit {
            start: block_argument.start_byte(),
            end: block_argument.end_byte(),
            replacement: format!("&{forwarding_name}"),
            safe: true,
        });
    }
    let mut edits = vec![Edit {
        start: block_argument.start_byte(),
        end: block_argument.end_byte(),
        replacement: "&".to_owned(),
        safe: true,
    }];
    // `add_parentheses(block_argument.parent, corrector) unless parenthesized_call?`: an anonymous
    // `&` may only be passed on from a parenthesized argument list.
    if let Some(owner) = list_owner(block_argument, context)
        && !is_parenthesized(owner, context)
    {
        edits.extend(parenthesize(owner, context));
    }
    offense.corrected_by_all(edits)
}

/// The node upstream reaches through `block_argument.parent`: the parameter list of the definition,
/// or the call the block was passed on from.
fn list_owner<'tree>(
    block_argument: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let parent = block_argument.parent_of(context)?;
    match parent.kind_str() {
        // `def foo(&block)`: upstream's parent is the `args` node, which is the parameter list here.
        "method_parameters" => Some(parent),
        // `bar(&block)`: upstream's parent is the `send`, which the argument list sits under here.
        "argument_list" => parent.parent_of(context),
        _ => None,
    }
}

/// `parenthesized_call?`: whether the list was written with parentheses.
fn is_parenthesized(owner: Node<'_>, context: &RuleContext<'_>) -> bool {
    let list = match owner.kind_str() {
        "method_parameters" => owner,
        _ => match owner.field("arguments") {
            Some(list) => list,
            None => return true,
        },
    };
    context.source.text()[list.start_byte()..].starts_with('(')
}

/// `add_parentheses`.
fn parenthesize(owner: Node<'_>, context: &RuleContext<'_>) -> Vec<Edit> {
    let text = context.source.text();
    match owner.kind_str() {
        // The `args` branch: the blank that separated the name from the parameters becomes the
        // opening parenthesis, and the closing one is written after them.
        "method_parameters" => {
            let range = owner.byte_range();
            let mut start = range.start;
            while start > 0 && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
                start -= 1;
            }
            vec![
                Edit {
                    start,
                    end: range.start,
                    replacement: "(".to_owned(),
                    safe: true,
                },
                Edit {
                    start: range.end,
                    end: range.end,
                    replacement: ")".to_owned(),
                    safe: true,
                },
            ]
        }
        // The call branch: the one character after the selector is removed and replaced by `(`, and
        // the `)` goes after the last argument.
        _ => {
            let Some(selector) = owner.field("method") else {
                return Vec::new();
            };
            let opening = selector.end_byte();
            let closing = send_range(owner, context).end;
            vec![
                Edit {
                    start: opening,
                    end: opening + 1,
                    replacement: "(".to_owned(),
                    safe: true,
                },
                Edit {
                    start: closing,
                    end: closing,
                    replacement: ")".to_owned(),
                    safe: true,
                },
            ]
        }
    }
}

/// `anonymous_block_argument?`: `&` with no name.
fn is_anonymous_block_parameter(node: Node<'_>) -> bool {
    node.kind_str() == "block_parameter" && node.field("name").is_none()
}

/// `explicit_block_argument?`: `&name`.
fn is_explicit_block_parameter(node: Node<'_>) -> bool {
    node.kind_str() == "block_parameter" && node.field("name").is_some()
}

/// The name `&name` binds.
fn block_parameter_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    node.field("name")
        .map(|name| context.source.node_text(name))
}

/// `use_kwarg_in_method_definition?`: a `kwarg` or `kwoptarg` among the parameters. A `**rest` is
/// neither, and does not stop the rewrite.
fn uses_keyword_parameter(node: Node<'_>) -> bool {
    let Some(parameters) = node.field("parameters") else {
        return false;
    };
    let mut stack = vec![parameters];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "keyword_parameter" {
            return true;
        }
        push_named_children(current, &mut stack);
    }
    false
}

/// `use_block_argument_as_local_variable?`: whether the body reads or writes the name as a variable
/// rather than only passing it on as `&name`.
fn reads_as_variable(node: Node<'_>, name: Option<&str>, context: &RuleContext<'_>) -> bool {
    let Some(name) = name else {
        return false;
    };
    let Some(body) = node.field("body") else {
        return false;
    };
    let mut stack = vec![body];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "identifier"
            && context.source.node_text(current) == name
            && is_variable_use(current, context)
        {
            return true;
        }
        push_named_children_in(current, context, &mut stack);
    }
    false
}

/// Whether the identifier reaches upstream as an `lvar` or the target of an `lvasgn`, which is what
/// tells a read of the block parameter from a call of the same name or a pass of the block itself.
fn is_variable_use(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return true;
    };
    match parent.kind_str() {
        // `&block`, which is the `block_pass` upstream skips.
        "block_argument" => false,
        // `foo.block` and `block(1)` are calls, not reads of the variable.
        "call" => parent.field("method") != Some(node),
        // A name bound again by a nested block or definition is not this variable.
        kind if PARAMETER_LISTS.contains(&kind) => false,
        "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter" => parent.field("name") != Some(node),
        _ => true,
    }
}
