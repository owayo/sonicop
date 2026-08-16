use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `AllowedMethods`' default: the RSpec matchers whose argument reads better without parentheses.
const DEFAULT_ALLOWED: &[&str] = &[
    "be",
    "be_a",
    "be_an",
    "be_between",
    "be_falsey",
    "be_kind_of",
    "be_instance_of",
    "be_truthy",
    "be_within",
    "eq",
    "eql",
    "end_with",
    "include",
    "match",
    "raise_error",
    "respond_to",
    "start_with",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context
        .setting::<Vec<String>>("AllowedMethods")
        .unwrap_or_else(|| DEFAULT_ALLOWED.iter().map(|name| (*name).to_owned()).collect());

    for node in context.nodes_of("call") {
        // `node.parenthesized?`.
        let Some(arguments) = node.field("arguments") else {
            continue;
        };
        if !arguments.child(0).is_some_and(|open| open.kind_str() == "(") {
            continue;
        }
        let outer_arguments = super::nodes::children(arguments);
        // `each_child_node(:call)` walks the receiver too, since it is a child of the send.
        let children = node
            .field("receiver")
            .into_iter()
            .chain(outer_arguments.iter().copied());
        for nested in children {
            if nested.kind_str() != "call" {
                continue;
            }
            let Some(nested_arguments) = nested.field("arguments") else {
                continue;
            };
            let inner = super::nodes::children(nested_arguments);
            if inner.is_empty()
                || nested_arguments
                    .child(0)
                    .is_some_and(|open| open.kind_str() == "(")
                || reads_as_binary_and(context, nested_arguments, &inner)
            {
                continue;
            }
            let Some(method) = nested.field("method") else {
                continue;
            };
            let name = context.source.node_text(method);
            // `setter_method?` and `operator_method?`: neither can take parentheses at all.
            if name.ends_with('=') || super::nodes::is_operator_method(name) {
                continue;
            }
            if outer_arguments.len() == 1
                && inner.len() == 1
                && allowed.iter().any(|allowed| allowed == name)
            {
                continue;
            }
            let (Some(first), Some(last)) = (inner.first(), inner.last()) else {
                continue;
            };
            let message = format!(
                "Add parentheses to nested method call `{}`.",
                context.source.node_text(nested)
            );
            offenses.push(
                context
                    .offense(message, nested.byte_range())
                    .corrected_by_all([
                        // `range_with_surrounding_space(first_arg.begin, side: :left,
                        // whitespace: true, continuations: true)` becomes the opening parenthesis.
                        Edit {
                            start: leading_space(context, first.start_byte()),
                            end: first.start_byte(),
                            replacement: "(".to_owned(),
                            safe: true,
                        },
                        Edit {
                            start: last.end_byte(),
                            end: last.end_byte(),
                            replacement: ")".to_owned(),
                            safe: true,
                        },
                    ]),
            );
        }
    }
}

/// Whether the grammar read an `&` that upstream's lexer takes for the binary operator.
///
/// `x.attr&0x8` is `(send (send _ :attr) :& (int 8))` upstream, because an `&` with no space before
/// it cannot open a block argument; the grammar spells it as a block argument all the same, which
/// would make the call look like one taking an argument without parentheses.
fn reads_as_binary_and(
    context: &RuleContext<'_>,
    arguments: tree_sitter::Node<'_>,
    inner: &[tree_sitter::Node<'_>],
) -> bool {
    let [only] = inner else {
        return false;
    };
    only.kind_str() == "block_argument"
        && only.start_byte() == arguments.start_byte()
        && !context.source.text()[..only.start_byte()]
            .ends_with([' ', '\t', '\n'])
}

/// `range_with_surrounding_space(first_arg.begin, side: :left, whitespace: true,
/// continuations: true)`.
///
/// The line continuation is the part that matters: walking back over whitespace alone stops at the
/// `\` and leaves it in the source, so the `(` that replaces the run lands after it and writes
/// `foo \(bar)` -- which is not Ruby. The shared walk takes the `\` and its line break together,
/// since a backslash only ends a line when the break follows it.
fn leading_space(context: &RuleContext<'_>, start: usize) -> usize {
    crate::rules::support::final_pos(context.source.text(), start, false, true, true, true)
}
