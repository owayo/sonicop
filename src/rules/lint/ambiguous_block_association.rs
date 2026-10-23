use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, InsertionAnchor, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_iter;
use crate::rules::send_node::named_children_of;

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

/// `AllowedPatterns` は文字列と Ruby の Regexp の両方を受け付ける。
struct Allowed {
    methods: Vec<String>,
    patterns: Vec<&'static Regex>,
}

impl Allowed {
    fn new(context: &RuleContext<'_>) -> Self {
        Self {
            methods: context.setting("AllowedMethods").unwrap_or_default(),
            patterns: crate::rules::naming::support::forbidden_patterns_named(context, "AllowedPatterns"),
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
    let list = named_children_of(arguments, context);
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
    let offense = context.offense(message.replace("\r\n", "\n"), node.byte_range());
    // 本家は引数リストを書き直さず、メソッド名の後ろの空白を消してその前に `(` を挿し、`)` は
    // 最後の引数の後ろに掛ける:
    //
    //   range = node.loc.selector.end.join(node.first_argument.source_range.begin)
    //   corrector.remove(range); corrector.insert_before(range, '(')
    //   corrector.insert_after(node.last_argument, ')')
    //
    // 引数リストを 1 つの置換で出し直すと、その中を補正する別 cop の編集を飲み込む -- Rails では
    // `Layout/IndentationWidth` のブロック本体の字下げが失われた。空白への 2 つの呼び出しを `(` への
    // 置換 1 つに畳むのもいけない。同じ空白を `(` に置換する `Style/NestedParenthesizedCalls` と同じ
    // 置換になって衝突せず、両方の `)` が入って構文を壊す。See `Offense::edit_anchors`.
    let correction = method.map(|method| {
        let space = method.end_byte()..first.start_byte();
        [
            (
                Edit {
                    start: space.start,
                    end: space.end,
                    replacement: String::new(),
                    safe: true,
                },
                None,
            ),
            InsertionAnchor::Before(space).insert("("),
            InsertionAnchor::After(last.byte_range()).insert(")"),
        ]
    });
    offenses.push(match correction {
        Some(edits) => offense.corrected_by_all_anchored(edits),
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
    let Some(call) = node
        .parent_of(context)
        .filter(|parent| parent.kind_str() == "call")
    else {
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
    let Some(&last) = named_children_of(arguments, context).last() else {
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
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "call"
            && send_end(current) == end
            && BLOCK_METHODS.contains(&call_method_name(current, context))
            && !has_arguments(current)
        {
            return Some(current);
        }
        let start = stack.len();
        let _cursor = current.walk();
        stack.extend(named_children_iter(current, context));
        stack[start..].reverse();
    }
    None
}

/// The block written on a call, which upstream holds one level above the call instead.
fn block_of(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind_str() == "call")
        .then(|| node.field("block"))
        .flatten()
}

/// Where the `send` upstream would end: a block belongs to a node above the call there, so a call
/// carrying one ends at its own last token rather than at the block's `}`.
///
/// Without the distinction, `queries.map { |q| q }` ends exactly where the argument list holding it
/// ends, and every such argument looks like the call a trailing `do` block stole its block from.
fn send_end(call: Node<'_>) -> usize {
    let Some(block) = block_of(call) else {
        return call.end_byte();
    };
    let mut cursor = call.walk();
    call.children(&mut cursor)
        .filter(|child| child.id() != block.id())
        .map(|child| child.end_byte())
        .max()
        .unwrap_or_else(|| call.end_byte())
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
