//! A mutable object assigned to a constant is frozen, so that nothing can change it in place.
//!
//! What counts as mutable moved with the language: regexp and range literals became frozen objects
//! in Ruby 3.0, and the same release let a `shareable_constant_value` magic comment freeze whatever
//! a constant is given. Both are read off the configured target version rather than assumed.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node;

use super::frozen_string::{is_frozen, kind_of, literals_enabled};
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Freeze mutable objects assigned to constants.";

/// `MUTABLE_LITERALS`, as the node kinds that spell them. `regexp` and `irange`/`erange` are on the
/// list, and dropped again for a target that freezes them.
const MUTABLE_LITERAL_KINDS: &[&str] = &[
    "string",
    "chained_string",
    "character",
    "heredoc_beginning",
    "subshell",
    "file",
    "array",
    "string_array",
    "symbol_array",
    // `A = 1, 2` and `A = *items` are each one `array` upstream, written without brackets.
    "right_assignment_list",
    "splat_argument",
    "hash",
    "regex",
    "range",
];

/// `IMMUTABLE_LITERALS`, which `strict` style lets stand.
const IMMUTABLE_LITERAL_KINDS: &[&str] = &[
    "integer",
    "line",
    "float",
    "rational",
    "complex",
    "simple_symbol",
    "delimited_symbol",
    "true",
    "false",
    "nil",
];

/// `{:+ :- :* :** :/ :% :<<}`: what a numeric literal on the left may be operated on with.
const NUMERIC_LEFT_OPERATORS: &[&str] = &["+", "-", "*", "**", "/", "%", "<<"];

/// The same without `<<`, which appends rather than computes.
const NUMERIC_RIGHT_OPERATORS: &[&str] = &["+", "-", "*", "**", "/", "%"];

/// `{:== :=== :!= :<= :>= :< :>}`.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", "<", ">"];

/// `{:count :length :size}`.
const SIZE_METHODS: &[&str] = &["count", "length", "size"];

/// The values `shareable_constant_value_enabled?` accepts.
const SHAREABLE_ENABLED: &[&str] = &["literal", "experimental_everything", "experimental_copy"];

/// Every value `valid_shareable_constant_value?` accepts, `none` included: a later `none` turns the
/// directive back off.
const SHAREABLE_VALID: &[&str] = &[
    "none",
    "literal",
    "experimental_everything",
    "experimental_copy",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        strict: context
            .setting::<String>("EnforcedStyle")
            .is_some_and(|style| style == "strict"),
        recursive: context.setting("Recursive").unwrap_or(false),
        frozen_strings: literals_enabled(context),
        shareable: shareable_lines(context),
    };
    for node in context.nodes_of_any(&["assignment", "operator_assignment"]) {
        let Some(left) = node.field("left") else {
            continue;
        };
        if !is_constant(left) {
            continue;
        }
        // A `casgn` without an expression is only ever corrected through `CONST ||= ...`; every
        // other operator leaves the constant holding what it already held.
        if node.kind_str() == "operator_assignment"
            && node
                .field("operator")
                .is_none_or(|operator| context.source.node_text(operator) != "||=")
        {
            continue;
        }
        let Some(value) = node.field("right") else {
            continue;
        };
        cop.on_assignment(value, offenses);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    strict: bool,
    recursive: bool,
    frozen_strings: bool,
    /// Whether the `shareable_constant_value` directive is in force at each line, or `None` when
    /// the file never mentions it.
    shareable: Option<Vec<bool>>,
}

impl<'tree> Cop<'_, 'tree> {
    fn on_assignment(&self, value: Node<'tree>, offenses: &mut Vec<Offense>) {
        for node in self.mutable_nodes(value) {
            offenses.push(
                self.context
                    .offense(MSG, node.byte_range())
                    .corrected_by_all(self.correct(node)),
            );
        }
    }

    /// `mutable_nodes`: the value itself, unless it is a literal already frozen in place and the
    /// recursive option asks for what it holds to be frozen too.
    fn mutable_nodes(&self, value: Node<'tree>) -> Vec<Node<'tree>> {
        if self.recursive
            && let Some(receiver) = self.explicitly_frozen_literal(value)
        {
            return literal_children(receiver)
                .into_iter()
                .flat_map(|child| self.mutable_nodes(child))
                .collect();
        }
        match self.offending(value) {
            true => vec![value],
            false => Vec::new(),
        }
    }

    fn offending(&self, value: Node<'tree>) -> bool {
        match self.strict {
            true => self.strict_check(value),
            false => self.literal_check(value),
        }
    }

    fn literal_check(&self, value: Node<'tree>) -> bool {
        self.mutable_or_unfrozen_range(value)
            && !self.frozen_string_literal(value)
            && !self.shareable_constant_value(value)
    }

    fn strict_check(&self, value: Node<'tree>) -> bool {
        !self.immutable_literal(value)
            && !self.operation_produces_immutable_object(value)
            && !self.frozen_string_literal(value)
            && !self.shareable_constant_value(value)
    }

    fn mutable_or_unfrozen_range(&self, value: Node<'tree>) -> bool {
        if self.mutable_literal(value) {
            return true;
        }
        // Before 3.0 a range was mutable, and one written in parentheses reaches the cop wrapped in
        // a `begin` that no literal test matches.
        self.context.target_ruby_version() <= RubyVersion::new(2, 7)
            && value.kind_str() == "parenthesized_statements"
            && matches!(
                send_node::named_children(value).as_slice(),
                [only] if only.kind_str() == "range"
            )
    }

    fn mutable_literal(&self, value: Node<'tree>) -> bool {
        !self.frozen_regexp_or_range_literal(value)
            && MUTABLE_LITERAL_KINDS.contains(&kind_of(value, self.context))
    }

    fn immutable_literal(&self, value: Node<'tree>) -> bool {
        if self.frozen_regexp_or_range_literal(value) {
            return true;
        }
        if IMMUTABLE_LITERAL_KINDS.contains(&kind_of(value, self.context)) {
            return true;
        }
        // A sign written against a numeric literal is folded into the literal upstream.
        value.kind_str() == "unary"
            && value
                .field("operator")
                .is_some_and(|operator| {
                    matches!(self.context.source.node_text(operator), "-" | "+")
                })
            && value
                .field("operand")
                .is_some_and(|operand| IMMUTABLE_LITERAL_KINDS.contains(&operand.kind_str()))
    }

    fn frozen_regexp_or_range_literal(&self, value: Node<'tree>) -> bool {
        self.context.target_ruby_version() >= RubyVersion::new(3, 0)
            && matches!(value.kind_str(), "regex" | "range")
    }

    fn frozen_string_literal(&self, value: Node<'tree>) -> bool {
        self.frozen_strings && is_frozen(self.context, value)
    }

    /// `shareable_constant_value?`: whether a magic comment above the value has already declared
    /// what constants hold to be shareable, and so frozen.
    fn shareable_constant_value(&self, value: Node<'tree>) -> bool {
        if self.context.target_ruby_version() < RubyVersion::new(3, 0) {
            return false;
        }
        let Some(lines) = &self.shareable else {
            return false;
        };
        let (last, _) = self.context.source.line_column(value.end_byte());
        lines.get(last - 1).copied().unwrap_or(false)
    }

    /// `explicitly_frozen_literal?`, answered with the literal the `.freeze` was sent to.
    fn explicitly_frozen_literal(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        if node.kind_str() != "call" || !send_node::is_plain_send(node, self.context) {
            return None;
        }
        let method = node.field("method")?;
        if self.context.source.node_text(method) != "freeze" {
            return None;
        }
        let receiver = node.field("receiver")?;
        self.mutable_literal(receiver).then_some(receiver)
    }

    /// `operation_produces_immutable_object?`: the calls whose result this cop treats as frozen,
    /// whether or not every one of them really is.
    fn operation_produces_immutable_object(&self, node: Node<'tree>) -> bool {
        // `(const _ _)`: any constant, however it was reached. `__ENCODING__` is one of them, built
        // as `Encoding::UTF_8` by the builder configuration RuboCop uses.
        if matches!(
            kind_of(node, self.context),
            "constant" | "scope_resolution" | "encoding"
        ) {
            return true;
        }
        // `(send (const {nil? cbase} :ENV) :[] _)`, alone or as the left half of an `or`.
        if self.env_lookup(node) {
            return true;
        }
        if node.kind_str() == "binary"
            && node.field("operator").is_some_and(|operator| {
                matches!(self.context.source.node_text(operator), "||" | "or")
            })
            && node
                .field("left")
                .is_some_and(|left| self.env_lookup(left))
        {
            return true;
        }
        let Some((receiver, name, arguments, block)) = self.dispatch(node) else {
            return false;
        };
        // `Struct.new` and `Data.define` build a value type, with or without a body.
        if let Some(receiver) = receiver {
            let constructor = match name {
                "new" => send_node::top_level_constant(receiver, "Struct", self.context),
                "define" => send_node::top_level_constant(receiver, "Data", self.context),
                _ => false,
            };
            if constructor {
                return true;
            }
        }
        // `(send _ {:count :length :size} ...)`, likewise with or without a block.
        if SIZE_METHODS.contains(&name) && receiver.is_some() {
            return true;
        }
        if block {
            return false;
        }
        let Some(receiver) = receiver else {
            return false;
        };
        if name == "freeze" && arguments.is_empty() {
            return true;
        }
        let [argument] = arguments.as_slice() else {
            return false;
        };
        COMPARISON_OPERATORS.contains(&name)
            || (NUMERIC_LEFT_OPERATORS.contains(&name) && self.numeric_literal(receiver))
            || (NUMERIC_RIGHT_OPERATORS.contains(&name) && self.numeric_literal(*argument))
    }

    /// One `send`, however it was written: `1 + 2` and `1.+(2)` are the same node upstream, and a
    /// block written after a call is wrapped around it rather than held by it.
    #[allow(clippy::type_complexity)]
    fn dispatch(
        &self,
        node: Node<'tree>,
    ) -> Option<(Option<Node<'tree>>, &'tree str, Vec<Node<'tree>>, bool)> {
        match node.kind_str() {
            "binary" => Some((
                Some(node.field("left")?),
                self.context
                    .source
                    .node_text(node.field("operator")?),
                vec![node.field("right")?],
                false,
            )),
            "call" if send_node::is_plain_send(node, self.context) => Some((
                node.field("receiver"),
                self.context
                    .source
                    .node_text(node.field("method")?),
                send_node::arguments(node)
                    .into_iter()
                    .map(|argument| argument.first())
                    .collect(),
                node.field("block").is_some(),
            )),
            _ => None,
        }
    }

    /// `(send (const {nil? cbase} :ENV) :[] _)`. This builder leaves an index read as a `send` of
    /// `:[]` rather than an `index` node.
    fn env_lookup(&self, node: Node<'tree>) -> bool {
        if node.kind_str() != "element_reference" {
            return false;
        }
        let children = send_node::named_children(node);
        matches!(children.as_slice(), [receiver, _]
            if send_node::top_level_constant(*receiver, "ENV", self.context))
    }

    /// `{float int}`, sign and all.
    fn numeric_literal(&self, node: Node<'tree>) -> bool {
        match node.kind_str() {
            "integer" | "float" => true,
            "unary" => {
                node.field("operator")
                    .is_some_and(|operator| {
                        matches!(self.context.source.node_text(operator), "-" | "+")
                    })
                    && node
                        .field("operand")
                        .is_some_and(|operand| matches!(operand.kind_str(), "integer" | "float"))
            }
            _ => false,
        }
    }

    fn correct(&self, node: Node<'tree>) -> Vec<Edit> {
        let mut edits = Vec::new();
        self.freeze(node, &mut edits);
        edits
    }

    fn freeze(&self, node: Node<'tree>, edits: &mut Vec<Edit>) {
        let expr = node.byte_range();
        if let Some(splat) = self.splat_value(node) {
            // `[*range]` already reads as a list; anything else needs parentheses of its own before
            // `to_a` can be sent to it.
            let source = self.context.source.node_text(splat);
            let parenthesized = splat.kind_str() == "parenthesized_statements"
                && matches!(
                    send_node::named_children(splat).as_slice(),
                    [only] if only.kind_str() == "range"
                );
            let replacement = match parenthesized {
                true => format!("{source}.to_a"),
                false => format!("({source}).to_a"),
            };
            edits.push(edit(expr.clone(), replacement));
            edits.push(edit(expr.end..expr.end, ".freeze".to_owned()));
            return;
        }
        if is_unbracketed_array(node) {
            edits.push(edit(expr.start..expr.start, "[".to_owned()));
            edits.push(edit(expr.end..expr.end, "]".to_owned()));
        } else if self.requires_parentheses(node) {
            edits.push(edit(expr.start..expr.start, "(".to_owned()));
            edits.push(edit(expr.end..expr.end, ")".to_owned()));
        }
        edits.push(edit(expr.end..expr.end, ".freeze".to_owned()));
        if self.recursive {
            self.freeze_nested_literals(node, edits);
        }
    }

    /// `freeze_nested_literals`: every literal held inside one, skipping the subtrees that already
    /// froze themselves but still looking under them.
    fn freeze_nested_literals(&self, node: Node<'tree>, edits: &mut Vec<Edit>) {
        for child in literal_children(node) {
            if let Some(receiver) = self.explicitly_frozen_literal(child) {
                self.freeze_nested_literals(receiver, edits);
            } else if !self.frozen_string_literal(child)
                && !self.shareable_constant_value(child)
                && self.mutable_literal(child)
            {
                self.freeze(child, edits);
            }
        }
    }

    /// `(array (splat $_))`: an array holding nothing but one splat. Written without brackets, that
    /// array reaches tree-sitter as the splat itself.
    fn splat_value(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        let splat = match node.kind_str() {
            "splat_argument" => node,
            "array" | "right_assignment_list" => {
                match send_node::named_children(node).as_slice() {
                    [only] if only.kind_str() == "splat_argument" => *only,
                    _ => return None,
                }
            }
            _ => return None,
        };
        send_node::named_children(splat).first().copied()
    }

    /// `requires_parentheses?`: `node.range_type? || (node.send_type? && node.loc.dot.nil?)`. A
    /// call written without a dot binds more loosely than the one `.freeze` would be written with.
    fn requires_parentheses(&self, node: Node<'tree>) -> bool {
        match kind_of(node, self.context) {
            "range" | "binary" | "identifier" | "unary" => true,
            "call" => node.field("operator").is_none(),
            _ => false,
        }
    }
}

fn edit(range: Range<usize>, replacement: String) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement,
        // `SafeAutoCorrect: false`: freezing an object turns a mutation that was accepted into a
        // `FrozenError`.
        safe: false,
    }
}

/// `node.array_type? && !node.bracketed?`: `A = 1, 2` is an array with no brackets to freeze
/// through.
fn is_unbracketed_array(node: Node<'_>) -> bool {
    node.kind_str() == "right_assignment_list"
}

/// `literal_children`: what an array or hash holds that may itself want freezing. A percent array
/// is skipped -- `.freeze` cannot be written against one of its words.
fn literal_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    match node.kind_str() {
        // A percent array's words are not nodes `.freeze` can be written against.
        "string_array" | "symbol_array" => Vec::new(),
        "array" | "right_assignment_list" => send_node::named_children(node),
        "hash" => send_node::named_children(node)
            .into_iter()
            .filter(|child| child.kind_str() == "pair")
            .flat_map(|pair| send_node::named_children(pair))
            .collect(),
        _ => Vec::new(),
    }
}

/// `(casgn ...)`: an assignment target that names a constant rather than a method or a variable.
fn is_constant(left: Node<'_>) -> bool {
    match left.kind_str() {
        "constant" => true,
        "scope_resolution" => left
            .field("name")
            .is_some_and(|name| name.kind_str() == "constant"),
        _ => false,
    }
}

/// Whether the `shareable_constant_value` directive is in force at each line of the file, or `None`
/// when no line carries one.
///
/// Upstream reads the lines above every constant afresh; the answer only ever depends on the last
/// directive at or above the line, so one scan settles the file.
fn shareable_lines(context: &RuleContext<'_>) -> Option<Vec<bool>> {
    let text = context.source.text();
    if !text.to_ascii_lowercase().contains("shareable") {
        return None;
    }
    let mut enabled = false;
    let mut lines = Vec::with_capacity(context.source.line_count());
    for number in 1..=context.source.line_count() {
        if let Some(value) = MagicComment::parse(context.source.line(number))
            .shareable_constant_value()
            .filter(|value| SHAREABLE_VALID.contains(&value.as_str()))
        {
            enabled = SHAREABLE_ENABLED.contains(&value.as_str());
        }
        lines.push(enabled);
    }
    Some(lines)
}
