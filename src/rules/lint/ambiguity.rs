//! The two diagnostics the parser's lexer raises for an argument that begins with an operator.
//!
//! Upstream reads `processed_source.diagnostics` and reports what the lexer already decided:
//! reaching a `/`, `*`, `**`, `&`, `+` or `-` while it expects the first argument of a command
//! call, with a space in front of it and none behind. Nothing here parses that state -- the tree
//! records it, since only a lexer in that state produces an argument list written without
//! parentheses, and the operator is then the first character of its first argument.
//!
//! Two things the tree records as an argument list are nevertheless never lexed in that state, so
//! neither warning reaches them: an argument list belonging to a keyword that is not a call, and a
//! `->` that opens a lambda literal rather than a unary minus.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Edit;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

/// One ambiguity the lexer would have warned about.
pub(super) struct Ambiguity<'tree> {
    /// The operator itself, which is what the diagnostic points at.
    pub operator: Range<usize>,
    /// The call, `yield` or `super` whose arguments the correction parenthesizes.
    pub owner: Node<'tree>,
    /// The span the parentheses go around: the arguments as written.
    arguments: Range<usize>,
}

/// Keywords whose arguments the lexer never reads from `expr_arg`.
///
/// `return`, `break` and `next` leave the lexer in `expr_mid`, where an operator opens a literal
/// with nothing to guess at, and `redo` and `retry` take no arguments at all. `yield` and `super`
/// are absent on purpose: to the lexer they are ordinary command calls and they do warn.
const KEYWORDS_WITHOUT_ARGUMENTS: &[&str] = &["break", "next", "redo", "retry", "return"];

/// The call spellings a leading `-` is never ambiguous after. `super` and `yield` are keywords
/// rather than identifiers, so `super -1` cannot be read as a subtraction -- while `super *a` and
/// `yield /re/` still can be a multiplication and a division.
const KEYWORD_CALLS: &[&str] = &["super", "yield"];

/// Whether the call is one of those keywords, spelled either as the `method` of a call or as a node.
fn is_keyword_call(owner: Node<'_>) -> bool {
    KEYWORD_CALLS.contains(&owner.kind_str())
        || owner
            .field("method")
            .is_some_and(|method| KEYWORD_CALLS.contains(&method.kind_str()))
}

/// Every argument list written without parentheses whose first argument opens with `prefixes`.
pub(super) fn scan<'tree>(
    context: &'tree RuleContext<'_>,
    prefixes: &[&str],
) -> Vec<Ambiguity<'tree>> {
    let mut found = Vec::new();
    for list in context.nodes_of("argument_list") {
        // A list written with parentheses leaves the lexer nothing to guess at.
        if list
            .child(0)
            .is_some_and(|first| context.source.node_text(first) == "(")
        {
            continue;
        }
        let Some(owner) = list.parent_of(context) else {
            continue;
        };
        if KEYWORDS_WITHOUT_ARGUMENTS.contains(&owner.kind_str()) {
            continue;
        }

        let Some(first) = named_children_of(list, context).into_iter().next() else {
            continue;
        };
        let text = context.source.node_text(first);
        let Some(prefix) = prefixes
            .iter()
            .find(|prefix| text.starts_with(**prefix))
            .copied()
        else {
            continue;
        };
        // `super -1` and `yield -1`: a keyword cannot be the left operand of a subtraction, so the
        // lexer raises no `ambiguous_prefix` for a leading `-` after one. Every other prefix still
        // can be -- `super *a` is a multiplication as readily as a splat.
        if prefix == "-" && is_keyword_call(owner) {
            continue;
        }
        let start = first.start_byte();
        let bytes = context.source.text().as_bytes();
        // `->` is matched as a lambda literal before the `-` can be read as a prefix.
        if prefix == "-" && bytes.get(start + 1) == Some(&b'>') {
            continue;
        }
        // A space in front and none behind is what makes the operator ambiguous.
        if start == 0 || !matches!(bytes[start - 1], b' ' | b'\t') {
            continue;
        }
        let after = start + prefix.len();
        if bytes
            .get(after)
            .is_none_or(|byte| byte.is_ascii_whitespace())
        {
            continue;
        }
        found.push(Ambiguity {
            operator: start..after,
            owner,
            arguments: list.byte_range(),
        });
    }
    found.extend(binary_operands(context, prefixes));
    found
}

/// The same ambiguity written in a shape the grammar reads as a binary operator.
///
/// `do_something +42` leaves the lexer in `expr_arg` and is a command call whose argument opens
/// with a unary `+`; the grammar reads it as `do_something + 42`. **The two differ only in whether
/// the name is a local variable** -- which is the question the lexer answers as well -- so the
/// variable analysis settles it here.
fn binary_operands<'tree>(
    context: &'tree RuleContext<'_>,
    prefixes: &[&str],
) -> Vec<Ambiguity<'tree>> {
    let bytes = context.source.text().as_bytes();
    let mut found = Vec::new();
    for node in context.nodes_of("binary") {
        let (Some(operator), Some(left), Some(right)) = (
            node.field("operator"),
            node.field("left"),
            node.field("right"),
        ) else {
            continue;
        };
        if !prefixes.contains(&context.source.node_text(operator)) {
            continue;
        }
        // A local variable on the left makes this arithmetic, exactly as it does for the lexer.
        if left.kind_str() != "identifier" || context.variable_analysis().names_a_local(left) {
            continue;
        }
        // `do_something&.* -1` is a `csend`, which `on_send` never reaches.
        if !crate::rules::send_node::is_plain_send(node, context) {
            continue;
        }
        let start = operator.start_byte();
        if start == 0 || !matches!(bytes[start - 1], b' ' | b'\t') {
            continue;
        }
        if bytes
            .get(operator.end_byte())
            .is_none_or(|byte| byte.is_ascii_whitespace())
        {
            continue;
        }
        found.push(Ambiguity {
            operator: operator.byte_range(),
            owner: node,
            arguments: operator.start_byte()..right.end_byte(),
        });
    }
    found
}

impl Ambiguity<'_> {
    /// `add_parentheses`: the space that opened the argument list becomes the `(`.
    ///
    /// **`args_end` is the end of the arguments, which is not the end of the call.** Upstream's
    /// `send` stops at the last argument and holds any `do ... end` in a separate node above it,
    /// while this grammar keeps the block inside the `call`. Closing at the call would put the
    /// `)` after the `end` and write `p(/pattern/ do ... end)`, which Ruby rejects.
    /// `corrector.wrap(node, '(', ')')`, the arm `add_parentheses` takes for a node that answers
    /// nothing to `arguments`. The space in front of the arguments is left where it was.
    pub(super) fn wrap(&self) -> Vec<Edit> {
        vec![
            Edit {
                start: self.arguments.start,
                end: self.arguments.start,
                replacement: "(".to_owned(),
                safe: true,
            },
            Edit {
                start: self.arguments.end,
                end: self.arguments.end,
                replacement: ")".to_owned(),
                safe: true,
            },
        ]
    }

    pub(super) fn parenthesize(&self, context: &RuleContext<'_>) -> Vec<Edit> {
        let opening = self.arguments.start - 1;
        let closing = self.arguments.end;
        let _ = context;
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
