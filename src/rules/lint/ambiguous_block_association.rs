use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The enumerable methods a trailing `do` block was probably meant for.
const BLOCK_METHODS: &[&str] = &[
    "map",
    "collect",
    "flat_map",
    "collect_concat",
    "select",
    "filter",
    "find_all",
    "reject",
    "find",
    "detect",
    "each",
    "each_with_object",
    "each_with_index",
    "reduce",
    "inject",
    "sort_by",
    "min_by",
    "max_by",
    "group_by",
    "filter_map",
];

/// `OPERATOR_METHODS`. A call written as an operator needs no disambiguation, because a block
/// cannot follow an operand.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = Allowed::new(context);
    for node in context.nodes_of("call") {
        check_brace_block_argument(node, context, &allowed, offenses);
    }
    for node in context.nodes_of("do_block") {
        check_do_block(node, context, &allowed, offenses);
    }
}

/// `AllowedMethods` and `AllowedPatterns`, both empty by default.
struct Allowed {
    methods: Vec<String>,
    patterns: Vec<Regex>,
}

impl Allowed {
    fn new(context: &RuleContext<'_>) -> Self {
        let patterns: Vec<String> = context.setting("AllowedPatterns").unwrap_or_default();
        Self {
            methods: context.setting("AllowedMethods").unwrap_or_default(),
            patterns: patterns
                .iter()
                .filter_map(|pattern| Regex::new(pattern).ok())
                .collect(),
        }
    }

    fn method(&self, name: &str) -> bool {
        self.methods.iter().any(|allowed| allowed == name)
    }

    fn pattern(&self, source: &str) -> bool {
        self.patterns.iter().any(|pattern| pattern.is_match(source))
    }
}

/// `on_send`: `some_method a { |val| ... }`, where the braces bind to `a` rather than to the call
/// that was written around it.
fn check_brace_block_argument(
    node: Node<'_>,
    context: &RuleContext<'_>,
    allowed: &Allowed,
    offenses: &mut Vec<Offense>,
) {
    let Some(arguments) = node.field("arguments") else {
        return;
    };
    let list = named_children(arguments);
    let (Some(&first), Some(&last)) = (list.first(), list.last()) else {
        return;
    };
    // The last argument has to be a block whose own call takes no arguments: with arguments there
    // is nothing to be ambiguous about, since the block can only be the inner call's.
    let Some(block) = block_of(last) else {
        return;
    };
    if has_arguments(last)
        || parenthesized(node, arguments, context)
        || lambda_or_proc(last, context)
    {
        return;
    }
    let method = node.field("method");
    let name = method.map_or("", |method| context.source.node_text(method));
    let inner = call_source(last, block, context);
    if OPERATOR_METHODS.contains(&name)
        || allowed.method(call_method_name(last, context))
        || allowed.pattern(inner)
    {
        return;
    }
    let message = format!(
        "Parenthesize the param `{}` to make sure that the block will be associated with the \
         `{inner}` method call.",
        context.source.node_text(last)
    );
    let offense = context.offense(message, node.byte_range());
    // Upstream replaces only the space before the argument list and hangs the closing paren off
    // the last argument, rather than rewriting the whole list:
    //
    //   range = node.loc.selector.end.join(node.first_argument.source_range.begin)
    //   corrector.remove(range); corrector.insert_before(range, '(')
    //   corrector.insert_after(node.last_argument, ')')
    //
    // Re-emitting the list as one replacement would swallow whatever another cop wants to correct
    // inside it -- on Rails that lost `Layout/IndentationWidth`'s dedent of a block body.
    let correction = method.map(|method| {
        [
            Edit {
                start: method.end_byte(),
                end: first.start_byte(),
                replacement: "(".to_owned(),
                safe: true,
            },
            Edit {
                start: arguments.end_byte(),
                end: arguments.end_byte(),
                replacement: ")".to_owned(),
                safe: true,
            },
        ]
    });
    offenses.push(match correction {
        // The closing paren is an `insert_after` on the last argument, so that is the range the
        // insertion hangs off. See `Offense::correction_anchor`.
        Some(edits) => offense
            .corrected_by_all(edits)
            .corrections_anchored_at(last.start_byte()..arguments.end_byte()),
        None => offense,
    });
}

/// `on_block`: `render json: data.map do |item| ... end`, where the `do` block binds to `render`
/// and `map` is left without one.
fn check_do_block(
    node: Node<'_>,
    context: &RuleContext<'_>,
    allowed: &Allowed,
    offenses: &mut Vec<Offense>,
) {
    let Some(call) = node.parent_of(context).filter(|parent| parent.kind_str() == "call") else {
        return;
    };
    // `super do ... end` is a `zsuper` upstream, not a send.
    if !call
        .field("method")
        .is_some_and(|method| method.kind_str() != "super")
    {
        return;
    }
    let Some(arguments) = call.field("arguments") else {
        return;
    };
    let Some(&last) = named_children(arguments).last() else {
        return;
    };
    if parenthesized(call, arguments, context) {
        return;
    }
    let Some(inner) = trailing_block_method(last, arguments.end_byte(), context) else {
        return;
    };
    let inner_method = call_method_name(inner, context);
    if allowed.method(inner_method) || allowed.pattern(context.source.node_text(inner)) {
        return;
    }
    let outer_method = call
        .field("method")
        .map_or("", |method| context.source.node_text(method));
    let message = format!(
        "`{inner_method}` is called without a block because the `do` block binds to \
         `{outer_method}`. Use braces or extract to a variable."
    );
    offenses.push(context.offense(message, inner.byte_range()));
}

/// The call the `do` block could actually have attached to: the one whose source ends where the
/// arguments end. A call chained into another, or one buried in a parenthesized subcall, was never
/// a candidate.
fn trailing_block_method<'tree>(
    node: Node<'tree>,
    end: usize,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "call"
            && current.end_byte() == end
            && BLOCK_METHODS.contains(&call_method_name(current, context))
            && !has_arguments(current)
        {
            return Some(current);
        }
        let start = stack.len();
        let mut cursor = current.walk();
        stack.extend(current.named_children(&mut cursor));
        stack[start..].reverse();
    }
    None
}

fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// The block written on a call, which upstream holds one level above the call instead.
fn block_of(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind_str() == "call")
        .then(|| node.field("block"))
        .flatten()
}

/// `arguments?`: an empty argument list is no arguments at all.
fn has_arguments(node: Node<'_>) -> bool {
    node.field("arguments")
        .is_some_and(|arguments| arguments.named_child_count() > 0)
}

/// `parenthesized?`: whether the argument list is the one written in parentheses. Ruby decides by
/// adjacency -- `foo (a)` passes a parenthesized expression rather than taking a parenthesized
/// argument list -- but the grammar here starts the argument list at the `(` either way.
fn parenthesized(call: Node<'_>, arguments: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !context.source.node_text(arguments).starts_with('(') {
        return false;
    }
    call.field("method")
        .is_none_or(|method| method.end_byte() == arguments.start_byte())
}

/// `lambda_or_proc?`. A lambda or a proc is written to be passed on, so the braces are not a
/// mistake. The stabby form is a node of its own here rather than a call, and so never reaches
/// this at all.
fn lambda_or_proc(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let method = call_method_name(node, context);
    if matches!(method, "lambda" | "proc") {
        return node.field("receiver").is_none();
    }
    method == "new"
        && node
            .field("receiver")
            .is_some_and(|receiver| top_level_proc(receiver, context))
}

fn top_level_proc(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == "Proc",
        "scope_resolution" => {
            node.field("scope").is_none()
                && node
                    .field("name")
                    .is_some_and(|name| context.source.node_text(name) == "Proc")
        }
        _ => false,
    }
}

fn call_method_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    node.field("method")
        .map_or("", |method| context.source.node_text(method))
}

/// The source of the call the block was written on, which upstream holds as a node of its own.
fn call_source<'a>(node: Node<'_>, block: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    context.source.text()[node.start_byte()..block.start_byte()].trim_end()
}
