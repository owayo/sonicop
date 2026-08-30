//! What `Style/SelectByRegexp`, `Style/SelectByKind` and `Style/SelectByRange` have in common: a
//! `select` or `reject` whose block does nothing but one test, which `grep` already spells.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::arguments;

/// `SELECT_METHODS`, which keep what matches.
pub(super) const SELECT_METHODS: &[&str] = &["select", "filter", "find_all"];

/// The version that made `_1` a block parameter rather than a receiverless call.
const NUMBERED_VERSION: RubyVersion = RubyVersion::new(2, 7);

/// The version that made `it` a block parameter rather than a receiverless call.
const IT_VERSION: RubyVersion = RubyVersion::new(3, 4);

/// A `select`/`reject` written with a one-statement block.
pub(super) struct Selection<'tree> {
    /// The call the block hangs off, which is `node` upstream.
    pub(super) call: Node<'tree>,
    /// The one statement the block body is.
    pub(super) statement: Node<'tree>,
    /// The name the block reads its element by, which is `_1` or `it` when it declares none.
    pub(super) argument: String,
    /// The block's closing delimiter, where the correction reaches to.
    pub(super) end: usize,
    /// Whether the block named its parameter, which the implicit `_1` and `it` do not.
    pub(super) declared: bool,
    method: Node<'tree>,
}

impl<'tree> Selection<'tree> {
    /// The name the call was written with, which the message quotes.
    pub(super) fn method_name(&self, context: &RuleContext<'_>) -> String {
        context.source.node_text(self.method).to_owned()
    }

    /// `add_offense(block_node)`: upstream's block node spans the receiver through the closing
    /// delimiter, which is exactly the call node the grammar wrote the block inside.
    pub(super) fn report(
        &self,
        context: &RuleContext<'_>,
        message: String,
        replacement: Option<String>,
    ) -> Offense {
        let offense = context.offense(message, self.call.byte_range());
        match replacement {
            // `range_between(node.loc.selector.begin_pos, block_node.loc.end.end_pos)`.
            Some(replacement) => offense.corrected_by(Edit {
                start: self.method.start_byte(),
                end: self.end,
                replacement,
                safe: true,
            }),
            None => offense,
        }
    }

    /// `SELECT_METHODS.include?(method_name)`.
    pub(super) fn keeps_matches(&self, context: &RuleContext<'_>) -> bool {
        SELECT_METHODS.contains(&context.source.node_text(self.method))
    }
}

/// The shared opening of all three `on_send` handlers.
pub(super) fn selection<'tree>(
    context: &RuleContext<'_>,
    call: Node<'tree>,
    methods: &[&str],
) -> Option<Selection<'tree>> {
    let method = call.field("method")?;
    if !methods.contains(&context.source.node_text(method)) {
        return None;
    }
    let block = call.field("block")?;
    if !matches!(block.kind_str(), "block" | "do_block") {
        return None;
    }
    // `block_node.body&.begin_type?`: a block that does more than one thing is not a `grep`.
    let statements = body_statements(block);
    let [statement] = statements.as_slice() else {
        return None;
    };
    if receiver_allowed(call.field("receiver"), context) {
        return None;
    }
    let argument = block_argument(context, block)?;
    Some(Selection {
        call,
        statement: *statement,
        argument,
        end: block.end_byte(),
        declared: block.field("parameters").is_some(),
        method,
    })
}

/// `receiver_allowed?`: a hash yields pairs rather than elements, so `grep` would see something
/// else than the block did.
fn receiver_allowed(receiver: Option<Node<'_>>, context: &RuleContext<'_>) -> bool {
    let Some(receiver) = receiver else {
        return false;
    };
    match receiver.kind_str() {
        "hash" => true,
        // `(call _ {:to_h :to_hash} ...)` matches a receiverless call too -- `to_h.reject { … }`
        // is `(send (send nil :to_h) :reject)`. The grammar writes a bare name as an `identifier`,
        // which fell through to "not a hash".
        "identifier" => matches!(context.source.node_text(receiver), "to_h" | "to_hash"),
        // `env_const?`: `(const {nil? cbase} :ENV)`.
        "constant" => context.source.node_text(receiver) == "ENV",
        "scope_resolution" => {
            receiver.field("scope").is_none()
                && receiver
                    .field("name")
                    .is_some_and(|name| context.source.node_text(name) == "ENV")
        }
        // `(call (const _ :Hash) :[] ...)`, which the grammar writes as an index.
        "element_reference" => receiver
            .child(0)
            .is_some_and(|object| is_hash_constant(object, context)),
        "call" => {
            let Some(method) = receiver.field("method") else {
                return false;
            };
            match context.source.node_text(method) {
                "to_h" | "to_hash" => true,
                // `Hash.new(...)`, with or without a block.
                "new" => receiver
                    .field("receiver")
                    .is_some_and(|object| is_hash_constant(object, context)),
                _ => false,
            }
        }
        _ => false,
    }
}

fn is_hash_constant(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == "Hash",
        "scope_resolution" => node
            .field("name")
            .is_some_and(|name| context.source.node_text(name) == "Hash"),
        _ => false,
    }
}

/// The one name the block reads its element by: `(args (arg $_))`, a `numblock` reading `_1`, or
/// an `itblock` reading `it`.
fn block_argument(context: &RuleContext<'_>, block: Node<'_>) -> Option<String> {
    if let Some(parameters) = block.field("parameters") {
        let declared = super::nodes::children_in(parameters, context);
        let [only] = declared.as_slice() else {
            return None;
        };
        return (only.kind_str() == "identifier")
            .then(|| context.source.node_text(*only).to_owned());
    }
    let body = block.field("body")?;
    let mut numbered = 0;
    let mut it = false;
    scan_implicit(context, body, &mut numbered, &mut it);
    if numbered == 1 && context.target_ruby_version() >= NUMBERED_VERSION {
        return Some("_1".to_owned());
    }
    (numbered == 0 && it && context.target_ruby_version() >= IT_VERSION).then(|| "it".to_owned())
}

/// How many numbered parameters the body reads, and whether it reads `it`. A nested block's names
/// belong to that block, not to this one.
fn scan_implicit(context: &RuleContext<'_>, node: Node<'_>, numbered: &mut usize, it: &mut bool) {
    for child in super::nodes::children_in(node, context) {
        if matches!(child.kind_str(), "block" | "do_block" | "lambda") {
            continue;
        }
        if child.kind_str() == "identifier" {
            let name = context.source.node_text(child).as_bytes();
            match name {
                [b'_', digit @ b'1'..=b'9'] => {
                    *numbered = (*numbered).max(usize::from(digit - b'0'));
                }
                b"it" => *it = true,
                _ => {}
            }
            continue;
        }
        scan_implicit(context, child, numbered, it);
    }
}

/// The statements a block body holds, which the grammar wraps in a node of its own.
pub(super) fn body_statements<'tree>(block: Node<'tree>) -> Vec<Node<'tree>> {
    match block.field("body") {
        Some(body) => match body.kind_str() {
            "block_body" | "body_statement" => super::nodes::children(body),
            _ => vec![body],
        },
        None => Vec::new(),
    }
}

/// `unwrap_negation`: what `!` was written in front of, with the parentheses around it dropped.
pub(super) fn unwrap_negation<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Node<'tree> {
    let Some(operand) = negation_operand(node, context) else {
        return node;
    };
    match operand.kind_str() {
        "parenthesized_statements" => super::nodes::children(operand)
            .first()
            .copied()
            .unwrap_or(operand),
        _ => operand,
    }
}

/// What `!x` was written around, when the node is one.
pub(super) fn negation_operand<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<Node<'tree>> {
    if node.kind_str() != "unary" {
        return None;
    }
    let operator = node.field("operator")?;
    (context.source.node_text(operator) == "!").then(|| node.field("operand"))?
}

/// The receiver of the test, whichever way it was written.
pub(super) fn test_receiver<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "binary" => node.field("left"),
        _ => node.field("receiver"),
    }
}

/// The arguments the test was handed.
pub(super) fn test_arguments<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    match node.kind_str() {
        "binary" => node.field("right").into_iter().collect(),
        _ => arguments(node)
            .iter()
            .flat_map(|argument| argument.parts().to_vec())
            .collect(),
    }
}

/// The name the test was written with: a method name or a binary operator.
pub(super) fn test_method<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let name = match node.kind_str() {
        "binary" => node.field("operator")?,
        "call" => node.field("method")?,
        _ => return None,
    };
    Some(context.source.node_text(name))
}

/// Whether the node names the block's one parameter.
pub(super) fn reads_argument(node: Node<'_>, argument: &str, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "identifier" && context.source.node_text(node) == argument
}

/// `calls_lvar?`: the test either reads the element or is handed it.
pub(super) fn calls_argument(node: Node<'_>, argument: &str, context: &RuleContext<'_>) -> bool {
    test_receiver(node).is_some_and(|receiver| reads_argument(receiver, argument, context))
        || test_arguments(node)
            .last()
            .is_some_and(|last| reads_argument(*last, argument, context))
}
