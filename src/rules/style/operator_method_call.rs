use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;
use crate::rules::support;

const MSG: &str = "Redundant dot detected.";

/// `RESTRICT_ON_SEND`.
const OPERATORS: [&str; 23] = [
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "!", "!=", "!~",
];

/// `INVALID_SYNTAX_ARG_TYPES`: arguments that cannot be written without the parentheses.
const INVALID_ARGUMENTS: [&str; 4] = [
    "splat_argument",
    "hash_splat_argument",
    "block_argument",
    "forward_argument",
];

/// `foo.+(bar)`, where the dot is what a plain `foo + bar` does without.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        // `node.loc.dot`, and `on_csend` is not aliased, so `&.` never reaches the cop.
        let Some(dot) = node.field("operator") else {
            continue;
        };
        if !send_node::is_plain_send(node, context) {
            continue;
        }
        let Some(selector) = node.field("method") else {
            continue;
        };
        if !OPERATORS.contains(&context.source.node_text(selector)) {
            continue;
        }
        // `node.receiver.const_type?`.
        let Some(receiver) = node.field("receiver") else {
            continue;
        };
        if matches!(receiver.kind_str(), "constant" | "scope_resolution") {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let [rhs] = arguments.as_slice() else {
            continue;
        };
        let parenthesized = node
            .field("arguments")
            .is_some_and(|list| context.source.node_text(list).starts_with('('));
        // `method_call_with_parenthesized_arg?`.
        let enclosing = enclosing_call(node);
        if enclosing.is_some() && has_first_child(*rhs, &locals) && parenthesized {
            continue;
        }
        // `invalid_syntax_argument?`.
        if INVALID_ARGUMENTS.contains(&rhs.kind_str()) {
            continue;
        }
        let mut edits = Vec::new();
        // `wrap_in_parentheses_if_chained`: the operation has to be bracketed before whatever is
        // chained onto it can read the right thing.
        let chained = enclosing.is_some_and(|call| !is_first_argument(call, node));
        if chained && parenthesized {
            let list = node.field("arguments").expect("checked");
            let text = context.source.text();
            // `ParenthesesCorrector.correct`: the argument's own parentheses go away.
            edits.push(remove(
                list.start_byte()
                    ..support::final_pos(text, list.start_byte() + 1, true, false, false, true),
            ));
            edits.push(remove(
                support::final_pos(text, list.end_byte() - 1, false, false, true, false)..list.end_byte(),
            ));
            edits.push(insert(selector.end_byte(), " "));
            edits.push(insert(node.start_byte(), "("));
            edits.push(insert(node.end_byte(), ")"));
        }
        edits.push(Edit {
            start: dot.start_byte(),
            end: dot.end_byte(),
            replacement: " ".to_owned(),
            safe: true,
        });
        if insert_space_after(selector, *rhs, enclosing.is_some(), context) {
            edits.push(insert(selector.end_byte(), " "));
        }
        offenses.push(
            context
                .offense(MSG, dot.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// `insert_space_after?`.
fn insert_space_after(
    selector: Node<'_>,
    rhs: Node<'_>,
    chained: bool,
    context: &RuleContext<'_>,
) -> bool {
    if selector.end_byte() == rhs.start_byte() {
        return true;
    }
    if chained {
        return false;
    }
    // A `/` followed by `(` would read as the start of a regexp without a space.
    let between = &context.source.text()[selector.end_byte()..rhs.start_byte()];
    context.source.node_text(selector) == "/" && between == "("
}

/// `argument.children.first`, which is `nil` for a receiverless call and for the keyword literals
/// that hold nothing.
fn has_first_child(node: Node<'_>, locals: &LocalVariables<'_, '_>) -> bool {
    match node.kind_str() {
        // A bare name is an `lvar` when it names a variable and a receiverless `send` otherwise.
        "identifier" => locals.is_lvar(node),
        "call" => node.field("receiver").is_some(),
        // `(const nil :CONST)`: the namespace is the first child, and a plain constant has none.
        // `::Foo` and `Foo::Bar` both carry one, which the grammar spells as a `scope_resolution`.
        "constant" => false,
        // An empty literal has no children either.
        "array" | "hash" | "string_array" | "symbol_array" => node.named_child_count() > 0,
        "nil" | "true" | "false" | "self" | "super" => false,
        _ => true,
    }
}

/// The call this one is written inside, with the argument list stepped over.
///
/// Upstream reads an operator written infix as a `send` too, so `a.+(b) + c` puts this node inside
/// one; here that is a `binary`, and a subscript is an `element_reference`.
fn enclosing_call<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    let parent = if parent.kind_str() == "argument_list" {
        parent.parent()?
    } else {
        parent
    };
    matches!(
        parent.kind_str(),
        "call" | "binary" | "unary" | "element_reference"
    )
    .then_some(parent)
}

/// `node.parent.first_argument == node`.
fn is_first_argument(call: Node<'_>, node: Node<'_>) -> bool {
    let first = match call.kind_str() {
        // An infix operator's one argument is its right operand.
        "binary" => call.field("right"),
        // A unary operator takes none at all.
        "unary" => None,
        "element_reference" => super::nodes::children(call).get(1).copied(),
        _ => call
            .field("arguments")
            .map(super::nodes::children)
            .and_then(|arguments| arguments.first().copied()),
    };
    first.is_some_and(|first| first.id() == node.id())
}

fn remove(range: std::ops::Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}

fn insert(at: usize, text: &str) -> Edit {
    Edit {
        start: at,
        end: at,
        replacement: text.to_owned(),
        safe: true,
    }
}
