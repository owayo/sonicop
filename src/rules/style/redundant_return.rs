use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// Calls whose block body RuboCop treats as a method body of its own.
const BLOCK_BODY_CALLS: &[&str] = &["define_method", "define_singleton_method", "lambda"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_multiple_return_values: bool = context
        .setting("AllowMultipleReturnValues")
        .unwrap_or(false);
    let mut returns: Vec<Node<'_>> = Vec::new();

    for node in context.nodes_of_any(&["method", "singleton_method", "call", "lambda"]) {
        let body = match node.kind_str() {
            "call" => block_body_of_tracked_call(node, context),
            // `-> { ... }` reaches RuboCop as a call to `lambda` too, so its body is a body.
            "lambda" => node.field("body").and_then(|block| block.field("body")),
            _ => node.field("body"),
        };
        let Some(body) = body else {
            continue;
        };
        check_branch(body, &mut returns);
    }

    // RuboCop keys reported offenses by range, so a `return` reached twice is reported once.
    returns.sort_by_key(Node::start_byte);
    returns.dedup_by_key(|node| node.start_byte());

    for node in returns {
        let arguments = return_arguments(node);
        let multiple_values = arguments.len() > 1 && !braceless_hash(&arguments);
        if allow_multiple_return_values && multiple_values {
            continue;
        }
        let message = if multiple_values {
            "Redundant `return` detected. To return multiple values, use an array."
        } else {
            "Redundant `return` detected."
        };
        offenses.push(
            context
                .offense(
                    message,
                    node.start_byte()..node.start_byte() + "return".len(),
                )
                .corrected_by_all(redundant_return_edits(
                    context,
                    node,
                    &arguments,
                    multiple_values,
                )),
        );
    }
}

/// The body of a `define_method`/`lambda` block, which RuboCop walks like a method body.
fn block_body_of_tracked_call<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    let name = node.field("method")?;
    if !BLOCK_BODY_CALLS.contains(&context.source.node_text(name)) {
        return None;
    }
    let block = node.field("block")?;
    block.field("body")
}

/// RuboCop's `check_branch`: the tail position of a method body, followed through the
/// constructs whose last expression is what the method returns.
///
/// A loop or a plain call is not one of them, so a `return` inside `while` or inside a block
/// other than the tracked calls above stays untouched.
fn check_branch<'tree>(node: Node<'tree>, returns: &mut Vec<Node<'tree>>) {
    match node.kind_str() {
        "return" => returns.push(node),
        "case" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                match child.kind_str() {
                    "when" => {
                        if let Some(body) = child.field("body") {
                            check_branch(body, returns);
                        }
                    }
                    "else" => check_branch(child, returns),
                    _ => {}
                }
            }
        }
        "case_match" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                match child.kind_str() {
                    "in_clause" => {
                        if let Some(body) = child.field("body") {
                            check_branch(body, returns);
                        }
                    }
                    "else" => check_branch(child, returns),
                    _ => {}
                }
            }
        }
        // `elsif` is how the grammar spells the `if` RuboCop finds nested in the else branch.
        "if" | "unless" | "elsif" => {
            for field in ["consequence", "alternative"] {
                if let Some(branch) = node.field(field) {
                    check_branch(branch, returns);
                }
            }
        }
        "if_modifier" | "unless_modifier" => {
            if let Some(body) = node.field("body") {
                check_branch(body, returns);
            }
        }
        // `return +1` reaches the grammar as `(return) + 1`, because a leading `+` on a literal
        // is indistinguishable from an addition without knowing that `return` takes arguments.
        // RuboCop's parser reads it as `return(+1)`, so the keyword is still a redundant return.
        "binary" => {
            if node
                .field("left")
                .is_some_and(|left| left.kind_str() == "return" && left.named_child_count() == 0)
            {
                returns.push(node);
            }
        }
        // Statement sequences. RuboCop sees a `begin` node here and follows its last child;
        // when the sequence carries `rescue`/`else`/`ensure` clauses it sees the `rescue` and
        // `ensure` nodes the parser wraps that body in instead.
        "body_statement"
        | "begin"
        | "parenthesized_statements"
        | "block_body"
        | "then"
        | "else" => check_sequence(node, returns),
        _ => {}
    }
}

/// Named children of a statement list that are not statements of it.
///
/// A heredoc's body is the one that matters here: the grammar hangs it off the statement list
/// beside the statement that opened it, so `return <<~SQL` leaves the body standing *after* the
/// `return`. Counting it makes the `return` no longer the last statement, and the cop then never
/// fires on a method that ends by returning a heredoc.
const NOT_A_STATEMENT: &[&str] = &["comment", "empty_statement", "heredoc_body"];

/// The tail of a statement sequence, including the exception-handling clauses it may carry.
///
/// An `ensure` body is never in tail position -- RuboCop's `check_ensure_node` looks only at the
/// protected body -- so it is skipped even though it is a child here.
fn check_sequence<'tree>(node: Node<'tree>, returns: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    let children: Vec<Node<'tree>> = node.named_children(&mut cursor)
        .filter(|child| !NOT_A_STATEMENT.contains(&child.kind_str()))
        .collect();

    let mut statements: Vec<Node<'tree>> = Vec::new();
    let mut rescues: Vec<Node<'tree>> = Vec::new();
    let mut else_clause = None;
    for child in children {
        match child.kind_str() {
            "rescue" => rescues.push(child),
            "else" => else_clause = Some(child),
            "ensure" => {}
            _ if rescues.is_empty() && else_clause.is_none() => statements.push(child),
            _ => {}
        }
    }

    if rescues.is_empty() {
        // The `else` of a bare `begin ... else ... end` is unreachable dead code, so a sequence
        // without a rescue keeps its last statement as the tail.
        if let Some(last) = statements.last() {
            check_branch(*last, returns);
        }
        return;
    }

    for rescue in rescues {
        if let Some(body) = rescue.field("body") {
            check_branch(body, returns);
        }
    }
    // With an `else` the protected body's value is discarded, so only the `else` is in tail
    // position. Without one the body itself is.
    match else_clause {
        Some(clause) => check_branch(clause, returns),
        None => {
            if let Some(last) = statements.last() {
                check_branch(*last, returns);
            }
        }
    }
}

/// The values a `return` yields. RuboCop reads them off the `return` node
/// itself, where a braceless trailing hash has already been folded into one
/// `hash` argument, while tree-sitter keeps its `pair`s separate.
fn return_arguments<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    // The `return +1` shape above: the value RuboCop sees as the sole argument is the right
    // operand the grammar split off.
    if node.kind_str() == "binary" {
        return node.field("right").into_iter().collect();
    }
    let Some(list) = node
        .named_child(0)
        .filter(|child| child.kind_str() == "argument_list")
    else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    list.named_children(&mut cursor).collect()
}

fn braceless_hash(arguments: &[Node<'_>]) -> bool {
    !arguments.is_empty()
        && arguments
            .iter()
            .all(|argument| argument.kind_str() == "pair")
}

/// Mirrors RuboCop's autocorrection: an argument-less `return` becomes `nil`,
/// multiple values gain `[]`, a braceless hash gains `{}`, a leading splat is
/// unwrapped, and the keyword plus its trailing space goes away. Dropping the
/// keyword alone would leave `return a, b` as the syntax error `a, b`.
/// The corrector calls upstream makes, one Edit each.
///
/// Upstream emits **up to four** -- `insert_before` / `insert_after` for the brackets or braces,
/// a `replace` for a splat, and a `remove` for the keyword. Folding them into one Edit that spans
/// the whole `return` makes this cop swallow every other cop's correction inside that span: the
/// engine drops the inner ones and defers them to the next pass. That is survivable on its own,
/// but it stops being survivable as soon as two offenses have to agree with each other.
fn redundant_return_edits(
    context: &RuleContext<'_>,
    node: Node<'_>,
    arguments: &[Node<'_>],
    multiple_values: bool,
) -> Vec<Edit> {
    // `correct_without_arguments`: `corrector.replace(return_node, 'nil')`.
    let (Some(first), Some(last)) = (arguments.first(), arguments.last()) else {
        return vec![Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: "nil".to_owned(),
            safe: true,
        }];
    };
    let wrapper = if multiple_values {
        Some(("[", "]"))
    } else if braceless_hash(arguments) {
        Some(("{", "}"))
    } else {
        None
    };
    let splat = first.kind_str() == "splat_argument";

    let mut edits = Vec::new();
    // `add_brackets` / `add_braces` open at the first argument -- **the same byte the splat's
    // `replace` starts at**, and `apply_edits` refuses an insertion there. Upstream's corrector
    // allows the pair, so the two are folded into one replacement to keep the same output.
    let opening = wrapper.map(|(open, _)| open).unwrap_or("");
    if splat {
        let text = context.source.node_text(*first);
        edits.push(Edit {
            start: first.start_byte(),
            end: first.end_byte(),
            replacement: format!("{opening}{}", text.strip_prefix('*').unwrap_or(text)),
            safe: true,
        });
    } else if let Some((open, _)) = wrapper {
        edits.push(insert(first.start_byte(), open));
    }
    if let Some((_, close)) = wrapper {
        edits.push(insert(last.end_byte(), close));
    }

    // `corrector.remove(range_with_surrounding_space(return_node.loc.keyword, side: :right))`.
    let text = context.source.node_text(node);
    let keyword_end = node.start_byte() + "return".len();
    let whitespace_end = keyword_end
        + text["return".len()..]
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
    edits.push(Edit {
        start: node.start_byte(),
        end: whitespace_end,
        replacement: String::new(),
        safe: true,
    });
    edits
}

fn insert(at: usize, text: &str) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text.to_owned(),
        safe: true,
    }
}
