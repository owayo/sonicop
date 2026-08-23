//! `Lint::Utils::NilReceiverChecker`: whether the code before a receiver already proves it is not
//! `nil`.
//!
//! Upstream walks outwards from the receiver, looking for a call that would have raised on `nil`,
//! a condition that tested it, or an earlier statement that did either. Two things make the walk
//! more than a search for the same text.
//!
//! **Occurrences are compared structurally, not by identity.** `foo.bar` followed by `foo&.baz`
//! is the whole point, and those are two different nodes holding the same source.
//!
//! **The same text can name different variables.** A block parameter shadows an outer variable of
//! the same name, so evidence gathered about one says nothing about the other. Upstream guards
//! this with `same_binding_as_receiver?`, and so does this port.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::blocks::BLOCK_KINDS;
use super::nil_methods::NIL_METHODS;

/// `def`, `defs`, `class`, `module`, `sclass`: the walk stops rather than reading across them.
const SCOPE_KINDS: &[&str] = &[
    "method",
    "singleton_method",
    "class",
    "module",
    "singleton_class",
];

/// Kinds whose first child upstream reads as `(begin ...)`.
const SEQUENCE_KINDS: &[&str] = &[
    "begin",
    "body_statement",
    "program",
    "then",
    "block_body",
    // `(foo.bar)` is a `begin` upstream, and the `:begin` arm reads its first child. Left out, a
    // parenthesized condition is walked past rather than into.
    "parenthesized_statements",
];

/// The conditional kinds whose `condition` upstream reads.
const CONDITION_KINDS: &[&str] = &[
    "if",
    "elsif",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
];

/// `cant_be_nil?`.
pub(super) fn cant_be_nil(
    context: &RuleContext<'_>,
    receiver: Node<'_>,
    additional_nil_methods: &[String],
) -> bool {
    let mut checker = NilReceiverChecker::new(context, receiver, additional_nil_methods);
    checker.sole_condition_of_parent_if(receiver) || checker.walk(receiver.parent(), receiver)
}

struct NilReceiverChecker<'a, 'src> {
    context: &'a RuleContext<'src>,
    additional_nil_methods: &'a [String],
    /// `@checked_nodes`: the walk revisits parents from children, so it needs a stop.
    checked: HashSet<usize>,
    /// `@receiver_binding_name` / `@receiver_binding_scope`.
    binding_name: Option<String>,
    binding_scope: Option<usize>,
}

impl<'a, 'src> NilReceiverChecker<'a, 'src> {
    fn new(
        context: &'a RuleContext<'src>,
        receiver: Node<'_>,
        additional_nil_methods: &'a [String],
    ) -> Self {
        let binding_name = binding_name(receiver, context);
        let binding_scope = binding_name
            .as_ref()
            .and_then(|name| binding_scope(receiver, name));
        Self {
            context,
            additional_nil_methods,
            checked: HashSet::new(),
            binding_name,
            binding_scope,
        }
    }

    /// `_cant_be_nil?`.
    fn walk<'tree>(&mut self, node: Option<Node<'tree>>, receiver: Node<'tree>) -> bool {
        let Some(node) = node else {
            return false;
        };
        if !self.checked.insert(node.id()) {
            return false;
        }

        let kind = node.kind_str();
        if SCOPE_KINDS.contains(&kind) {
            return false;
        }
        // Upstream's `:send` arm does not take `csend`: `foo&.bar` is not evidence that `foo` is
        // non-nil, it is the very question being asked. The grammar spells both as `call`.
        if (kind == "call" && !is_safe_navigation(node, self.context)) || kind == "identifier" {
            if let Some(inner) = node.field("receiver")
                && structurally_equal(inner, receiver, self.context)
                && self.same_binding_as_receiver(inner)
            {
                return self.non_nil_method(node);
            }
            for argument in call_arguments(node) {
                if self.walk(Some(argument), receiver) {
                    return true;
                }
            }
            if let Some(inner) = node.field("receiver")
                && self.walk(Some(inner), receiver)
            {
                return true;
            }
        } else if SEQUENCE_KINDS.contains(&kind) && !guards_its_body(node) {
            if self.walk(named_children(node).first().copied(), receiver) {
                return true;
            }
        } else if CONDITION_KINDS.contains(&kind) || kind == "case" || kind == "case_match" {
            let condition = node.field("condition").or_else(|| node.field("value"));
            if self.walk(condition, receiver) {
                return true;
            }
        } else if kind == "binary" {
            // `and` / `or`: upstream reads the left-hand side only, because the right one runs
            // conditionally.
            let logical = node.field("operator").is_some_and(|operator| {
                matches!(
                    self.context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                )
            });
            if logical && self.walk(node.field("left"), receiver) {
                return true;
            }
        } else if kind == "pair" {
            if self.walk(node.field("key"), receiver) || self.walk(node.field("value"), receiver) {
                return true;
            }
        } else if kind == "when" {
            for condition in named_children(node) {
                if condition.kind_str() == "then" {
                    break;
                }
                // `node.conditions` are the expressions themselves. The grammar wraps each in a
                // `pattern` node that upstream has no counterpart for, and stopping at the wrapper
                // means the walk never reads the condition it was called for.
                let written = match condition.kind_str() {
                    "pattern" => named_children(condition),
                    _ => vec![condition],
                };
                for expression in written {
                    if self.walk(Some(expression), receiver) {
                        return true;
                    }
                }
            }
        } else if (kind == "assignment" || kind == "operator_assignment")
            && self.walk(node.field("right"), receiver)
        {
            return true;
        }

        if sequentially_reached(node) {
            let mut sibling = node.prev_named_sibling();
            while let Some(previous) = sibling {
                if self.walk(Some(previous), receiver) {
                    return true;
                }
                sibling = previous.prev_named_sibling();
            }
        }

        match node.parent() {
            Some(parent) => self.walk(Some(parent), receiver),
            None => false,
        }
    }

    /// `non_nil_method?`: a method `nil` does not answer would have raised before reaching here.
    fn non_nil_method(&self, node: Node<'_>) -> bool {
        let Some(method) = node.field("method") else {
            return false;
        };
        let name = self.context.source.node_text(method);
        !NIL_METHODS.contains(&name)
            && !self
                .additional_nil_methods
                .iter()
                .any(|allowed| allowed == name)
    }

    /// `sole_condition_of_parent_if?`: an enclosing `if` already tested this receiver.
    fn sole_condition_of_parent_if<'tree>(&self, node: Node<'tree>) -> bool {
        let mut child = node;
        let mut parent = node.parent();

        while let Some(current) = parent {
            let kind = current.kind_str();
            // `parent.if_type?` covers the ternary too, and `unless` is excluded by
            // `unless parent.unless?`.
            if matches!(kind, "if" | "elsif" | "if_modifier" | "conditional") {
                if let Some(condition) = current.field("condition")
                    && condition.id() != child.id()
                    && self.non_nil_condition(condition, node)
                {
                    return true;
                }
                if kind == "elsif" {
                    parent = Some(find_top_if(current));
                }
            } else if is_else_branch(current) {
                parent = current.parent();
            }
            let Some(current) = parent else {
                return false;
            };
            child = current;
            parent = current.parent();
        }
        false
    }

    /// `non_nil_condition?`: the condition is the receiver itself, or a safe navigation whose root
    /// receiver is.
    fn non_nil_condition<'tree>(&self, condition: Node<'tree>, node: Node<'tree>) -> bool {
        if structurally_equal(condition, node, self.context)
            && self.same_binding_as_receiver(condition)
        {
            return true;
        }
        if !is_safe_navigation(condition, self.context) {
            return false;
        }
        csend_root_receiver(condition, self.context).is_some_and(|root| {
            structurally_equal(root, node, self.context) && self.same_binding_as_receiver(root)
        })
    }

    /// `same_binding_as_receiver?`.
    fn same_binding_as_receiver(&self, occurrence: Node<'_>) -> bool {
        let Some(name) = &self.binding_name else {
            return true;
        };
        binding_scope(occurrence, name) == self.binding_scope
    }
}

/// `binding_name`: the name whose binding can differ between structurally equal occurrences.
fn binding_name(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match node.kind_str() {
        "identifier" => Some(context.source.node_text(node).to_owned()),
        // `it` parses as a receiverless call below 3.4 even where it is the block parameter.
        "call" => {
            let text = context.source.node_text(node);
            (text == "it").then(|| text.to_owned())
        }
        _ => None,
    }
}

/// `binding_scope`: the block or definition the name is bound by.
fn binding_scope(occurrence: Node<'_>, name: &str) -> Option<usize> {
    let mut node = occurrence;
    while let Some(parent) = node.parent() {
        let kind = parent.kind_str();
        if BLOCK_KINDS.contains(&kind) {
            // Only the body is inside the block's scope; the call and the block's own parameters
            // evaluate outside it.
            if parent
                .field("body")
                .is_some_and(|body| body.id() == node.id())
                && block_binds_name(parent, name)
            {
                return Some(parent.id());
            }
        } else if SCOPE_KINDS.contains(&kind) {
            return Some(parent.id());
        }
        node = parent;
    }
    None
}

/// `block_binds_name?`.
fn block_binds_name(block: Node<'_>, name: &str) -> bool {
    let Some(parameters) = block.field("parameters") else {
        // `it` is the implicit parameter of a block that declares none.
        return name == "it";
    };
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .any(|parameter| parameter.utf8_text(&[]).is_ok() && parameter.kind_str() == "identifier")
}

/// `sequentially_reached?`: control reaches the node by falling through its left siblings.
///
/// Upstream writes this as `!else_branch?(node) || (node.if_type? && !node.elsif?)`, where an
/// `elsif` is the else-branch `if` of the clause above it. The grammar gives `elsif` a kind of its
/// own, so the same rule reads as: an `elsif` is entered by branching, and so is the body of an
/// `else`; an `if` written inside an `else` is not.
fn sequentially_reached(node: Node<'_>) -> bool {
    // **The grammar makes `rescue` and `ensure` siblings of the body they guard; upstream makes
    // them its parent.** Walking left from either would read that body as if it had run to
    // completion, which is exactly what a rescue clause cannot assume.
    if matches!(node.kind_str(), "rescue" | "ensure") || is_elsif(node) {
        return false;
    }
    if let Some(parent) = node.parent() {
        if parent.kind_str() == "ensure" {
            return false;
        }
        // `foo.bar rescue foo&.baz`: the handler is a child of the modifier here, and a `resbody`
        // upstream.
        if parent.kind_str() == "rescue_modifier"
            && parent
                .field("handler")
                .is_some_and(|handler| handler.id() == node.id())
        {
            return false;
        }
    }
    !is_else_branch(node)
}

/// A `body_statement` that carries a `rescue` / `ensure` / `else` is upstream's `kwbegin`, whose
/// first child is the `rescue` node rather than the first statement. Reading the statement there
/// would treat a guarded body as if nothing could interrupt it.
fn guards_its_body(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| matches!(child.kind_str(), "rescue" | "ensure" | "else"))
}

/// `else_branch?`: the node **is** the else arm of an `if`, which the grammar spells two ways --
/// an `else` node for the block form, and the third operand itself for a ternary.
fn is_else_branch(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    matches!(
        parent.kind_str(),
        "if" | "elsif" | "if_modifier" | "unless" | "unless_modifier" | "conditional"
    ) && parent
        .field("alternative")
        .is_some_and(|alternative| alternative.id() == node.id())
}

fn is_elsif(node: Node<'_>) -> bool {
    node.kind_str() == "elsif"
}

/// `find_top_if`.
fn find_top_if(node: Node<'_>) -> Node<'_> {
    let mut current = node;
    while is_elsif(current) {
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    current
}

/// `csend_root_receiver`: the receiver at the bottom of a chain of calls.
fn csend_root_receiver<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    let mut receiver = node.field("receiver")?;
    while receiver.kind_str() == "call"
        && let Some(inner) = receiver.field("receiver")
    {
        let _ = context;
        receiver = inner;
    }
    Some(receiver)
}

fn is_safe_navigation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "call"
        && (0..node.child_count())
            .filter_map(|index| node.child(index as u32))
            .any(|child| context.source.node_text(child) == "&.")
}

/// Upstream compares nodes with `==`, which is structural. Two occurrences of the same source in
/// the same shape are the same node to it.
fn structurally_equal(left: Node<'_>, right: Node<'_>, context: &RuleContext<'_>) -> bool {
    left.kind_str() == right.kind_str()
        && context.source.node_text(left) == context.source.node_text(right)
}

fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind_str() != "comment")
        .collect()
}

fn call_arguments<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    match node.field("arguments") {
        Some(list) => named_children(list),
        None => Vec::new(),
    }
}
