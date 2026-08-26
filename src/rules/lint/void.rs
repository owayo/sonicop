use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, named_children};
use crate::rules::support;

use super::blocks::{BLOCK_KINDS, BlockArgs};
use super::literals::{is_literal, literal_type};
use super::locals::LocalVariables;
use super::statements::{Branch, begin_containers, body_children, has_clause, statements};
use crate::rules::node_ext::NodeExt;

const SELF_MSG: &str = "`self` used in void context.";

/// `BINARY_OPERATORS`, whose result is thrown away when nothing reads it.
const BINARY_OPERATORS: &[&str] = &[
    "*", "/", "%", "+", "-", "==", "===", "!=", "<", ">", "<=", ">=", "<=>",
];

/// `UNARY_OPERATORS`, by the name upstream's parser gives the method.
const UNARY_OPERATORS: &[&str] = &["+@", "-@", "~", "!"];

/// `VOID_CONTEXT_METHODS`: the blocks whose last expression is discarded.
const VOID_CONTEXT_METHODS: &[&str] = &["each", "tap"];

/// `NONMUTATING_METHODS_WITH_BANG_VERSION` and `METHODS_REPLACEABLE_BY_EACH`.
const NONMUTATING_METHODS: &[&str] = &[
    "capitalize",
    "chomp",
    "chop",
    "compact",
    "delete_prefix",
    "delete_suffix",
    "downcase",
    "encode",
    "flatten",
    "gsub",
    "lstrip",
    "merge",
    "next",
    "reject",
    "reverse",
    "rotate",
    "rstrip",
    "scrub",
    "select",
    "shuffle",
    "slice",
    "sort",
    "sort_by",
    "squeeze",
    "strip",
    "sub",
    "succ",
    "swapcase",
    "tr",
    "tr_s",
    "transform_values",
    "unicode_normalize",
    "uniq",
    "upcase",
];
const METHODS_REPLACEABLE_BY_EACH: &[&str] = &["collect", "map"];

/// The conditional kinds `check_expression` looks inside.
const CONDITIONALS: &[&str] = &[
    "if",
    "elsif",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
];

struct Void<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    locals: LocalVariables<'a, 'tree>,
    check_nonmutating: bool,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Void {
        context,
        locals: LocalVariables::new(context),
        check_nonmutating: context
            .setting::<bool>("CheckForMethodsWithNoSideEffects")
            .unwrap_or(false),
    };
    for (container, expressions) in begin_containers(context) {
        cop.check_begin(container, expressions, offenses);
    }
    for block in context.nodes_of_any(&["block", "do_block", "lambda"]) {
        cop.check_block(block, offenses);
    }
    for clause in context.nodes_of("ensure") {
        cop.check_ensure(clause, offenses);
    }
}

impl Void<'_, '_> {
    fn text(&self, node: Node<'_>) -> &str {
        self.context.source.node_text(node)
    }

    /// `on_begin` and `on_kwbegin`.
    fn check_begin<'tree>(
        &self,
        container: Node<'tree>,
        mut expressions: Vec<Node<'tree>>,
        offenses: &mut Vec<Offense>,
    ) {
        let inside_each = self.inside_each_block(container);
        if !self.in_void_context(container) || inside_each || self.setter_method(container) {
            expressions.pop();
        }
        for expression in expressions {
            if !inside_each {
                self.check_void_op(expression, offenses);
            }
            self.check_expression(expression, offenses);
        }
    }

    /// `on_block`, for the block whose body is one expression.
    fn check_block(&self, block: Node<'_>, offenses: &mut Vec<Offense>) {
        let Some(container) = block.field("body") else {
            return;
        };
        let body = body_children(container);
        // A body of several statements is a `begin` upstream, which `on_begin` already saw, and so
        // is one written in parentheses.
        let [body] = body[..] else {
            return;
        };
        if body.kind_str() == "parenthesized_statements"
            || !self.in_void_context(container)
            || self.block_method(block) == Some("each")
        {
            return;
        }
        self.check_void_op(body, offenses);
        self.check_expression(body, offenses);
    }

    /// `on_ensure`, for the clause whose body is one expression.
    fn check_ensure(&self, clause: Node<'_>, offenses: &mut Vec<Offense>) {
        let body = statements(clause);
        let [body] = body[..] else {
            return;
        };
        if body.kind_str() == "parenthesized_statements" {
            return;
        }
        self.check_expression(body, offenses);
    }

    /// `in_void_context?`: whether the sequence's last expression is discarded.
    ///
    /// The `begin` upstream builds hangs off the container's parent, except inside a `rescue` or
    /// an `ensure`, which the parser puts between the two -- and there the sequence is no longer
    /// the last child, so nothing about it is void.
    fn in_void_context(&self, container: Node<'_>) -> bool {
        if container.kind_str() == "ensure" {
            return true;
        }
        if has_clause(container) {
            return false;
        }
        let Some(parent) = container.parent_of(self.context) else {
            return false;
        };
        match parent.kind_str() {
            "method" => self
                .method_name(parent)
                .is_some_and(|name| name == "initialize" || name.ends_with('=')),
            "singleton_method" => self
                .method_name(parent)
                .is_some_and(|name| name.ends_with('=')),
            kind if BLOCK_KINDS.contains(&kind) || kind == "lambda" => self
                .block_method(parent)
                .is_some_and(|name| VOID_CONTEXT_METHODS.contains(&name)),
            "for" => true,
            _ => false,
        }
    }

    /// `setter_method?`: the last expression of `def foo=(value)` is the argument Ruby returns.
    fn setter_method(&self, container: Node<'_>) -> bool {
        if container.kind_str() == "ensure" || has_clause(container) {
            return false;
        }
        container.parent_of(self.context).is_some_and(|parent| {
            matches!(parent.kind_str(), "method" | "singleton_method")
                && self
                    .method_name(parent)
                    .is_some_and(|name| name.ends_with('='))
        })
    }

    fn method_name<'a>(&'a self, node: Node<'_>) -> Option<&'a str> {
        node.field("name").map(|name| self.text(name))
    }

    /// The method a block was passed to, which is the call it hangs off.
    fn block_method<'a>(&'a self, block: Node<'_>) -> Option<&'a str> {
        if block.kind_str() == "lambda" {
            return Some("lambda");
        }
        block
            .parent()
            .filter(|call| call.kind_str() == "call")
            .and_then(|call| call.field("method"))
            .map(|method| self.text(method))
    }

    /// `node.each_ancestor(:any_block).first&.method?(:each)`.
    fn inside_each_block(&self, node: Node<'_>) -> bool {
        let mut current = node.parent_of(self.context);
        while let Some(ancestor) = current {
            if BLOCK_KINDS.contains(&ancestor.kind_str()) || ancestor.kind_str() == "lambda" {
                return self.block_method(ancestor) == Some("each");
            }
            current = ancestor.parent_of(self.context);
        }
        false
    }

    /// `check_void_op`: an operator whose result nothing reads.
    fn check_void_op(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let mut current = node;
        while current.kind_str() == "parenthesized_statements" {
            match statements(current).first() {
                Some(first) => current = *first,
                None => return,
            }
        }
        let Some(operator) = self.operator_call(current) else {
            return;
        };
        if !UNARY_OPERATORS.contains(&operator.method.as_str())
            && operator.dot.is_some()
            && operator.argument_count == 0
        {
            return;
        }
        let offense = self
            .context
            .offense(
                format!("Operator `{}` used in void context.", operator.method),
                operator.selector.clone(),
            )
            .corrections_anchored_at(current.byte_range());
        offenses.push(match operator.correction(current, self.context) {
            Some(edits) => offense.corrected_by_all(edits),
            None => offense,
        });
    }

    /// The call the node is, in upstream's terms, when its method is one of the operators.
    fn operator_call(&self, node: Node<'_>) -> Option<OperatorCall> {
        match node.kind_str() {
            "binary" => {
                let operator = node.field("operator")?;
                let method = self.text(operator).to_owned();
                BINARY_OPERATORS
                    .contains(&method.as_str())
                    .then(|| OperatorCall {
                        method,
                        selector: operator.byte_range(),
                        receiver: node.field("left").map(|left| left.byte_range()),
                        argument_count: 1,
                        dot: None,
                    })
            }
            "unary" => {
                let operator = node.field("operator")?;
                // **A sign in front of a number is part of the number upstream.** `-3` parses to
                // `(int -3)`, not to a `-@` send, so it is a literal here and no operator at all --
                // the grammar keeps the two apart and would otherwise report both.
                if matches!(self.text(operator), "-" | "+")
                    && node.field("operand").is_some_and(|operand| {
                        matches!(
                            operand.kind_str(),
                            "integer" | "float" | "rational" | "complex"
                        )
                    })
                {
                    return None;
                }
                let method = match self.text(operator) {
                    "-" => "-@",
                    "+" => "+@",
                    // `not x` is a `!` send upstream, reported at the keyword.
                    "not" => "!",
                    other @ ("~" | "!") => other,
                    _ => return None,
                }
                .to_owned();
                Some(OperatorCall {
                    method,
                    selector: operator.byte_range(),
                    receiver: node.field("operand").map(|operand| operand.byte_range()),
                    argument_count: 0,
                    dot: None,
                })
            }
            "call" => {
                let method = node.field("method")?;
                let name = self.text(method).to_owned();
                if !BINARY_OPERATORS.contains(&name.as_str())
                    && !UNARY_OPERATORS.contains(&name.as_str())
                {
                    return None;
                }
                Some(OperatorCall {
                    method: name,
                    selector: method.byte_range(),
                    receiver: node.field("receiver").map(|receiver| receiver.byte_range()),
                    argument_count: arguments(node).len(),
                    dot: node.field("operator").map(|operator| operator.byte_range()),
                })
            }
            _ => None,
        }
    }

    /// `check_expression`, which follows a conditional into the branch that produces the value.
    fn check_expression(&self, expression: Node<'_>, offenses: &mut Vec<Offense>) {
        if CONDITIONALS.contains(&expression.kind_str()) {
            // `IfNode#body` is the branch the parser normalises to the truthy one.
            if let Branch::One(body) = Branch::of(
                expression
                    .field("consequence")
                    .or_else(|| expression.field("body")),
            ) {
                self.check_void_expression_nodes(body, offenses);
            }
            return;
        }
        if matches!(expression.kind_str(), "case" | "case_match") {
            for child in named_children(expression) {
                match child.kind_str() {
                    "when" | "in_clause" => {
                        if let Branch::One(body) = Branch::of(child.field("body")) {
                            self.check_expression(body, offenses);
                        }
                    }
                    "else" => {
                        if let Branch::One(body) = Branch::of(Some(child)) {
                            self.check_expression(body, offenses);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        self.check_void_expression_nodes(expression, offenses);
    }

    fn check_void_expression_nodes(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        self.check_literal(node, offenses);
        self.check_var(node, offenses);
        self.check_self(node, offenses);
        self.check_void_expression(node, offenses);
        if self.check_nonmutating {
            self.check_nonmutating_send(node, offenses);
        }
    }

    fn check_literal(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        if !self.entirely_literal(node)
            || matches!(
                literal_type(node, self.context),
                Some("xstr" | "irange" | "erange" | "nil")
            )
        {
            return;
        }
        offenses.push(self.expression_offense(
            node,
            format!("Literal `{}` used in void context.", self.text(node)),
        ));
    }

    fn check_var(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let constant = matches!(node.kind_str(), "constant" | "scope_resolution");
        let variable = matches!(
            node.kind_str(),
            "instance_variable" | "global_variable" | "class_variable"
        ) || (node.kind_str() == "identifier"
            && (self.locals.is_lvar(node)
                || self.is_block_parameter(node)
                // `special_keyword?`: upstream parses these as `const` and reports them with the
                // variable message. The grammar spells them as plain identifiers.
                || matches!(self.text(node), "__FILE__" | "__LINE__" | "__ENCODING__")));
        if !constant && !variable {
            return;
        }
        // `__ENCODING__` is a `const` whose source is a keyword, which reads as a variable.
        let message = if constant && self.text(node) != "__ENCODING__" {
            format!("Constant `{}` used in void context.", self.text(node))
        } else {
            format!("Variable `{}` used in void context.", self.text(node))
        };
        offenses.push(self.expression_offense(node, message));
    }

    /// Whether the name is one of the parameters the innermost block never spelled out.
    fn is_block_parameter(&self, node: Node<'_>) -> bool {
        let mut current = node.parent_of(self.context);
        while let Some(ancestor) = current {
            if BLOCK_KINDS.contains(&ancestor.kind_str()) {
                return match BlockArgs::of(ancestor, self.context, &self.locals) {
                    BlockArgs::Numbered(highest) => {
                        (1..=highest).any(|index| self.text(node) == format!("_{index}"))
                    }
                    BlockArgs::It => self.text(node) == "it",
                    BlockArgs::Written(_) => false,
                };
            }
            current = ancestor.parent_of(self.context);
        }
        false
    }

    fn check_self(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        if node.kind_str() != "self" {
            return;
        }
        offenses.push(self.expression_offense(node, SELF_MSG.to_owned()));
    }

    fn check_void_expression(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        if !self.is_defined(node) && !self.is_lambda_or_proc(node) {
            return;
        }
        offenses.push(
            self.expression_offense(node, format!("`{}` used in void context.", self.text(node))),
        );
    }

    /// `check_nonmutating`: a method whose result is the only thing it produces.
    fn check_nonmutating_send(&self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let call = match node.kind_str() {
            "call" => node,
            kind if BLOCK_KINDS.contains(&kind) => match node.parent_of(self.context) {
                Some(parent) if parent.kind_str() == "call" => parent,
                _ => return,
            },
            _ => return,
        };
        let Some(method) = call.field("method") else {
            return;
        };
        let name = self.text(method);
        let suggestion = if METHODS_REPLACEABLE_BY_EACH.contains(&name) {
            "each".to_owned()
        } else if NONMUTATING_METHODS.contains(&name) {
            format!("{name}!")
        } else {
            return;
        };
        offenses.push(
            self.context
                .offense(
                    format!("Method `#{name}` used in void context. Did you mean `#{suggestion}`?"),
                    node.byte_range(),
                )
                .corrections_anchored_at(node.byte_range())
                .corrected_by(Edit {
                    start: method.start_byte(),
                    end: method.end_byte(),
                    replacement: suggestion,
                    safe: true,
                }),
        );
    }

    /// The offense the four expression checks share, with the removal upstream attaches to it.
    fn expression_offense(&self, node: Node<'_>, message: String) -> Offense {
        let offense = self.context.offense(message, node.byte_range());
        match self.void_expression_correction(node) {
            Some(edit) => offense
                .corrections_anchored_at(node.byte_range())
                .corrected_by(edit),
            None => offense,
        }
    }

    /// `autocorrect_void_expression`.
    fn void_expression_correction(&self, node: Node<'_>) -> Option<Edit> {
        // Referencing a constant can trigger autoloading, so removing it may change behaviour.
        if matches!(node.kind_str(), "constant" | "scope_resolution")
            && self.text(node) != "__ENCODING__"
        {
            return None;
        }
        // `node.parent` upstream is the conditional itself: a branch holding one statement *is*
        // that statement there, where the grammar wraps it in a `then` or an `else`.
        let mut parent = node.parent_of(self.context);
        while let Some(container) = parent
            .filter(|container| matches!(container.kind_str(), "then" | "else"))
            .filter(|container| statements(*container).len() == 1)
        {
            parent = container.parent_of(self.context);
        }
        if parent.is_some_and(|parent| {
            matches!(
                parent.kind_str(),
                "if" | "elsif"
                    | "unless"
                    | "if_modifier"
                    | "unless_modifier"
                    | "conditional"
                    | "case"
                    | "when"
                    | "case_match"
                    | "in_clause"
            )
        }) {
            return None;
        }
        let mut current = node.parent_of(self.context);
        while let Some(ancestor) = current {
            if matches!(ancestor.kind_str(), "method" | "singleton_method") {
                if self
                    .method_name(ancestor)
                    .is_some_and(|name| name.ends_with('='))
                {
                    return None;
                }
                break;
            }
            current = ancestor.parent_of(self.context);
        }
        let start = self.expand_left(node.start_byte());
        Some(Edit {
            start,
            end: node.end_byte(),
            replacement: String::new(),
            safe: true,
        })
    }

    /// `range_with_surrounding_space(range: node.source_range, side: :left)`, whose keywords are
    /// upstream's defaults: `newlines: true`, `whitespace: false`, `continuations: false`.
    ///
    /// **The stages do not run again.** Walking one run of spaces and then one run of line breaks is
    /// not the same as walking whatever whitespace is there: with a line holding only spaces above,
    /// eating past its break reaches those spaces, and eating them reaches the break above *them*.
    /// The blank line then disappears with the void expression, which upstream leaves alone.
    fn expand_left(&self, start: usize) -> usize {
        support::final_pos(self.context.source.text(), start, false, false, true, false)
    }

    /// `entirely_literal?`.
    fn entirely_literal(&self, node: Node<'_>) -> bool {
        match node.kind_str() {
            "array" | "string_array" | "symbol_array" => named_children(node)
                .into_iter()
                .all(|child| self.entirely_literal(child)),
            "hash" => named_children(node)
                .into_iter()
                .filter(|child| child.kind_str() == "pair")
                .all(|pair| {
                    ["key", "value"].iter().all(|field| {
                        pair.field(field)
                            .is_some_and(|part| self.entirely_literal(part))
                    })
                }),
            "call" => {
                node.field("method")
                    .is_some_and(|method| self.text(method) == "freeze")
                    && node
                        .field("receiver")
                        .is_some_and(|receiver| self.entirely_literal(receiver))
            }
            _ => is_literal(node, self.context),
        }
    }

    fn is_defined(&self, node: Node<'_>) -> bool {
        node.kind_str() == "unary"
            && node
                .field("operator")
                .is_some_and(|operator| self.text(operator) == "defined?")
    }

    /// `lambda_or_proc?`.
    fn is_lambda_or_proc(&self, node: Node<'_>) -> bool {
        if node.kind_str() == "lambda" {
            return true;
        }
        if node.kind_str() != "call" {
            return false;
        }
        let Some(method) = node.field("method") else {
            return false;
        };
        let name = self.text(method);
        let block = node
            .field("block")
            .is_some_and(|block| BLOCK_KINDS.contains(&block.kind_str()));
        let receiver = node.field("receiver");
        match (name, receiver) {
            ("lambda" | "proc", None) => block,
            ("new", Some(receiver)) => {
                crate::rules::send_node::top_level_constant(receiver, "Proc", self.context)
            }
            _ => false,
        }
    }
}

/// One operator call, as `check_void_op` reads it.
struct OperatorCall {
    method: String,
    selector: Range<usize>,
    receiver: Option<Range<usize>>,
    argument_count: usize,
    dot: Option<Range<usize>>,
}

impl OperatorCall {
    /// `autocorrect_void_op`.
    fn correction(&self, node: Node<'_>, context: &RuleContext<'_>) -> Option<Vec<Edit>> {
        if self.argument_count == 0 {
            let receiver = self.receiver.clone()?;
            return Some(vec![Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: context.source.slice(receiver).to_owned(),
                safe: true,
            }]);
        }
        let mut edits = Vec::new();
        if let Some(dot) = self.dot.clone() {
            edits.push(Edit {
                start: dot.start,
                end: dot.end,
                replacement: String::new(),
                safe: true,
            });
        }
        let text = context.source.text().as_bytes();
        let mut start = self.selector.start;
        while start > 0 && matches!(text[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        let mut end = self.selector.end;
        while end < text.len() && matches!(text[end], b' ' | b'\t') {
            end += 1;
        }
        edits.push(Edit {
            start,
            end,
            replacement: "\n".to_owned(),
            safe: true,
        });
        Some(edits)
    }
}
