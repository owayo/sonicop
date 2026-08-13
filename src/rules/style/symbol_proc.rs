//! `Style/SymbolProc`: `&:name` in place of a block that only calls one method on its parameter.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

/// `unsafe_hash_usage?`: `{}.reject` hands the block a key and a value, so `&:sym` is not the same
/// call.
const UNSAFE_HASH_METHODS: &[&str] = &["reject", "select"];

/// `unsafe_array_usage?`: `[].min` hands the block two elements to compare.
const UNSAFE_ARRAY_METHODS: &[&str] = &["min", "max"];

const ARRAY_KINDS: &[&str] = &["array", "string_array", "symbol_array"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed_methods: Vec<String> = context
        .setting("AllowedMethods")
        .unwrap_or_else(|| vec!["define_method".to_owned()]);
    let allowed_patterns: Vec<regex::Regex> = context
        .setting::<Vec<String>>("AllowedPatterns")
        .unwrap_or_default()
        .iter()
        .filter_map(|pattern| regex::Regex::new(pattern).ok())
        .collect();
    let allow_with_arguments: bool = context
        .setting("AllowMethodsWithArguments")
        .unwrap_or(false);
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(false);
    let locals = LocalVariables::new(context);

    for block in context.nodes_of_any(&["block", "do_block"]) {
        let Some(dispatch) = Dispatch::new(context, block) else {
            continue;
        };
        // `unsafe_hash_usage?` / `unsafe_array_usage?`.
        if let Some(receiver) = dispatch.receiver {
            if receiver.kind_str() == "hash" && UNSAFE_HASH_METHODS.contains(&dispatch.method.as_str())
            {
                continue;
            }
            if ARRAY_KINDS.contains(&receiver.kind_str())
                && UNSAFE_ARRAY_METHODS.contains(&dispatch.method.as_str())
            {
                continue;
            }
        }
        // `allowed_method_name?`.
        if allowed_methods.contains(&dispatch.method)
            || allowed_patterns
                .iter()
                .any(|pattern| pattern.is_match(&dispatch.method))
        {
            continue;
        }
        // `allow_if_method_has_argument?`.
        if allow_with_arguments && !send_node::arguments(dispatch.call).is_empty() {
            continue;
        }
        let Some(parameter) = block_parameter(context, &locals, block, &dispatch) else {
            continue;
        };
        let Some(called) = single_call(context, block, &parameter) else {
            continue;
        };
        let (open, close) = braces(block);
        // `allow_comments?`. The `comments_contain_disables?` half of it reads the run's directive
        // analysis, which a cop is not handed here; without it a block whose only comment is the
        // `rubocop:disable` that asked for this offense would be let through.
        if allow_comments && contains_comments(context, dispatch.call, close) {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!(
                        "Pass `&:{}` as an argument to `{}` instead of a block.",
                        called, dispatch.method
                    ),
                    open.start_byte()..close.end_byte(),
                )
                .corrected_by_all(autocorrect(context, block, &dispatch, &called)),
        );
    }
}

/// The call a block hangs off, as `MethodDispatchNode` presents it.
struct Dispatch<'t> {
    /// The node the block is written on: the call, or the `lambda` node of a `->` literal.
    call: Node<'t>,
    receiver: Option<Node<'t>>,
    method: String,
    /// `lambda_literal?`: written as `->`, so a rewrite has to spell the method out.
    arrow: bool,
}

impl<'t> Dispatch<'t> {
    fn new(context: &RuleContext<'_>, block: Node<'t>) -> Option<Self> {
        let call = block.parent()?;
        match call.kind_str() {
            // `-> (x) { }` dispatches `lambda` upstream however the arrow is written.
            "lambda" => Some(Self {
                call,
                receiver: None,
                method: "lambda".to_owned(),
                arrow: true,
            }),
            "call" => {
                let method = call.field("method")?;
                Some(Self {
                    call,
                    receiver: call.field("receiver"),
                    // `super` and `zsuper` both answer `:super`.
                    method: match method.kind_str() {
                        "super" => "super".to_owned(),
                        _ => context.source.node_text(method).to_owned(),
                    },
                    arrow: false,
                })
            }
            _ => None,
        }
    }

}

/// `(args (arg _var))`, `(numblock _ 1 _)` or `(itblock _ :it _)`: the one name the body may call
/// a method on.
fn block_parameter(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    block: Node<'_>,
    dispatch: &Dispatch<'_>,
) -> Option<String> {
    let parameters = match dispatch.arrow {
        true => dispatch.call.field("parameters"),
        false => block.field("parameters"),
    };
    let Some(parameters) = parameters else {
        return implicit_parameter(context, locals, block);
    };
    let written = super::nodes::children(parameters);
    let [only] = written.as_slice() else {
        return None;
    };
    if only.kind_str() != "identifier" {
        return None;
    }
    // `destructuring_block_argument?`: `|a,|` takes the first element of what it is handed.
    if context.source.node_text(parameters).contains(',') {
        return None;
    }
    Some(context.source.node_text(*only).to_owned())
}

/// The parameter a block that wrote none still has: `_1`, or `it` from 3.4 on.
fn implicit_parameter(
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    block: Node<'_>,
) -> Option<String> {
    let body = block.field("body")?;
    let mut highest = 0;
    let mut uses_it = false;
    scan_implicit(context, body, &mut highest, &mut uses_it, locals);
    if highest > 0 {
        // `(numblock _ $1 _)`: a block reading `_2` takes two parameters, not one.
        return (highest == 1).then(|| "_1".to_owned());
    }
    // `it` names the parameter only once the parser reads it as one.
    (uses_it && context.target_ruby_version() >= RubyVersion::new(3, 4)).then(|| "it".to_owned())
}

fn scan_implicit(
    context: &RuleContext<'_>,
    node: Node<'_>,
    highest: &mut usize,
    uses_it: &mut bool,
    locals: &LocalVariables<'_, '_>,
) {
    for child in super::nodes::children(node) {
        // A nested block's implicit parameters are its own.
        if matches!(child.kind_str(), "block" | "do_block" | "lambda") {
            continue;
        }
        if child.kind_str() == "identifier" {
            let name = context.source.node_text(child);
            let bytes = name.as_bytes();
            if bytes.len() == 2 && bytes[0] == b'_' && bytes[1].is_ascii_digit() && bytes[1] != b'0'
            {
                *highest = (*highest).max(usize::from(bytes[1] - b'0'));
            } else if name == "it" && !locals.is_lvar(child) {
                *uses_it = true;
            }
            continue;
        }
        scan_implicit(context, child, highest, uses_it, locals);
    }
}

/// `(send (lvar _var) $_)`: the one method the block body calls on its parameter, with nothing
/// passed to it.
fn single_call(context: &RuleContext<'_>, block: Node<'_>, parameter: &str) -> Option<String> {
    let body = block.field("body")?;
    let statements = super::nodes::children(body);
    let [only] = statements.as_slice() else {
        return None;
    };
    match only.kind_str() {
        "call" => {
            let receiver = only.field("receiver")?;
            if receiver.kind_str() != "identifier" || context.source.node_text(receiver) != parameter {
                return None;
            }
            // `&.` builds a `csend`, which the pattern never matches.
            if !send_node::is_plain_send(*only, context) {
                return None;
            }
            if only.field("block").is_some() {
                return None;
            }
            if !send_node::arguments(*only).is_empty() {
                return None;
            }
            let method = only.field("method")?;
            Some(context.source.node_text(method).to_owned())
        }
        // `!x` and `-x` are calls upstream, named after the operator.
        "unary" => {
            let operand = only.field("operand")?;
            if operand.kind_str() != "identifier" || context.source.node_text(operand) != parameter {
                return None;
            }
            let operator = only.field("operator")?;
            Some(
                match context.source.node_text(operator) {
                    "!" | "not" => "!",
                    "-" => "-@",
                    "+" => "+@",
                    "~" => "~@",
                    _ => return None,
                }
                .to_owned(),
            )
        }
        _ => None,
    }
}

/// The block's own delimiters, which is what the offense points at.
fn braces(block: Node<'_>) -> (Node<'_>, Node<'_>) {
    let mut cursor = block.walk();
    let children: Vec<Node<'_>> = block.children(&mut cursor).collect();
    let open = children[0];
    let close = children[children.len() - 1];
    (open, close)
}

fn autocorrect(
    context: &RuleContext<'_>,
    block: Node<'_>,
    dispatch: &Dispatch<'_>,
    called: &str,
) -> Vec<Edit> {
    // `->(x) { x.foo }` has no room for `&:foo`, so the whole literal is rewritten.
    if dispatch.arrow {
        return vec![Edit {
            start: dispatch.call.start_byte(),
            end: dispatch.call.end_byte(),
            replacement: format!("lambda(&:{called})"),
            safe: true,
        }];
    }
    let arguments = send_node::arguments(dispatch.call);
    let removed = block_range_with_space(context, block, dispatch);
    match arguments.last() {
        Some(last) => {
            let range = range_with_trailing_comma(context, last.range());
            let mut replacement = format!(" &:{called}");
            if !context.source.slice(range.clone()).ends_with(',') {
                replacement.insert(0, ',');
            }
            vec![
                Edit {
                    start: range.end,
                    end: range.end,
                    replacement,
                    safe: true,
                },
                Edit {
                    start: removed.start,
                    end: removed.end,
                    replacement: String::new(),
                    safe: true,
                },
            ]
        }
        None => vec![Edit {
            start: removed.start,
            end: removed.end,
            replacement: format!("(&:{called})"),
            safe: true,
        }],
    }
}

/// `block_range_with_space`: the block and the space before it, reaching back over an empty
/// argument list the `&:sym` is about to fill.
fn block_range_with_space(
    context: &RuleContext<'_>,
    block: Node<'_>,
    dispatch: &Dispatch<'_>,
) -> std::ops::Range<usize> {
    let (open, close) = braces(block);
    let start = match dispatch.call.field("arguments") {
        Some(list)
            if send_node::arguments(dispatch.call).is_empty()
                && list
                    .child(0)
                    .is_some_and(|first| context.source.node_text(first) == "(") =>
        {
            list.start_byte()
        }
        _ => open.start_byte(),
    };
    super::ranges::extended_left(context.source.text(), start, true)..close.end_byte()
}

/// `range_with_surrounding_comma(range, :right)`.
fn range_with_trailing_comma(
    context: &RuleContext<'_>,
    range: std::ops::Range<usize>,
) -> std::ops::Range<usize> {
    let bytes = context.source.text().as_bytes();
    let mut end = range.end;
    while end < bytes.len() && bytes[end] == b',' {
        end += 1;
    }
    range.start..end
}

/// `contains_comments?`: a comment on any line the block spans, up to but not including the line
/// its closing delimiter sits on.
fn contains_comments(context: &RuleContext<'_>, call: Node<'_>, close: Node<'_>) -> bool {
    let first = context.source.line_column(call.start_byte()).0;
    let last = context.source.line_column(close.start_byte()).0;
    context.comment_ranges().iter().any(|comment| {
        let line = context.source.line_column(comment.start).0;
        (first..last).contains(&line)
    })
}
