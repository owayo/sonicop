//! The syntax tree RuboCop's complexity metrics are computed over.
//!
//! `Metrics/AbcSize`, `Metrics/CyclomaticComplexity` and `Metrics/PerceivedComplexity` all walk the
//! body of a method and count nodes by their type. The types they count are the parser's, and
//! several of them have no node of their own here: `a[i]` is a `send` upstream, `->(){}` is a
//! `block` on a call to `lambda`, `begin … rescue … end` holds a `rescue` node that tree-sitter
//! spells as a clause beside the statements it guards. This module replays a body as the sequence
//! of parser nodes it would have been, so each cop is left with nothing but its own arithmetic.
//!
//! Both traversal orders upstream uses are reproduced: `MethodComplexity#complexity` walks
//! `each_node`, which is depth-first pre-order, while `AbcSizeCalculator` walks `visit_depth_last`,
//! which is post-order. The difference is observable through the repeated-`&.` discount, whose
//! answer depends on which of two calls was reached first.

use tree_sitter::Node;

use super::fragments::Fragments;
use super::locals::{
    Locals, field_name, folded_parameter_list, is_keyword_literal, named_children, operator,
    split_match_operator,
};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::spurious_assignment_list;
use crate::source::SourceFile;

/// The parser node types the three complexity cops count. Types they all ignore are never emitted,
/// which is why `while_post`, `numblock` and `kwbegin` are absent: reproducing them would change
/// nothing and only invite a cop to count them by mistake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Send,
    Csend,
    Yield,
    /// `lvasgn`, which both counts as an assignment and clears the repeated-`&.` discount.
    Lvasgn,
    /// `ivasgn`, `cvasgn`, `gvasgn` or `casgn`, none of which the discount cares about.
    Asgn,
    Masgn,
    OpAsgn,
    OrAsgn,
    AndAsgn,
    /// `arg` and its relatives, including a block's parameters and `shadowarg`.
    Arg,
    If,
    While,
    Until,
    For,
    Block,
    BlockPass,
    Rescue,
    When,
    InPattern,
    And,
    Or,
    Case,
    CaseMatch,
}

/// One parser node, with everything the cops ask of it already answered. The questions are cheap
/// here and awkward later: whether a send is `==` rather than a call worth counting, whether an
/// `if` is closed by `else` or by `elsif`, whether the method a block belongs to iterates.
#[derive(Clone, Copy)]
pub(super) struct Emit<'a> {
    pub kind: Kind,
    pub node: Node<'a>,
    /// `Send`/`Csend`: the method is one of `== === != <= >= > <`.
    pub comparison: bool,
    /// `Send`/`Csend`: an assignment-form call such as `a.b = 1`, which is an assignment too.
    pub setter: bool,
    /// `If`: an `else` keyword closes it, rather than an `elsif` or nothing.
    pub has_else: bool,
    /// `Block`/`BlockPass`: `Some(false)` when the method it belongs to is known not to iterate,
    /// `None` when there is no method name to judge, as for a block on `super`.
    pub iterating: Option<bool>,
    /// `Masgn`/`OpAsgn`/`OrAsgn`/`AndAsgn`: what `compound_assignment` adds for the children the
    /// calculator would otherwise miss.
    pub miscounted: usize,
    /// `Lvasgn`: the name written. `Csend`: the receiver's variable name, if it is one.
    pub name: Option<&'a str>,
    /// `Lvasgn`/`Arg`: the name is neither absent nor `_`-prefixed, so it counts.
    pub capturing: bool,
}

impl<'a> Emit<'a> {
    fn of(kind: Kind, node: Node<'a>) -> Self {
        Self {
            kind,
            node,
            comparison: false,
            setter: false,
            has_else: false,
            iterating: None,
            miscounted: 0,
            name: None,
            capturing: false,
        }
    }
}

/// The order the cops walk in. `each_node` yields a node before its children, `visit_depth_last`
/// after them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Order {
    Pre,
    Post,
}

pub(super) struct Walk<'a> {
    source: &'a SourceFile,
    locals: &'a Locals,
    fragments: &'a Fragments,
    order: Order,
}

impl<'a> Walk<'a> {
    pub(super) fn new(
        context: &'a RuleContext<'_>,
        locals: &'a Locals,
        fragments: &'a Fragments,
        order: Order,
    ) -> Self {
        Self {
            source: context.source,
            locals,
            fragments,
            order,
        }
    }

    pub(super) fn run<F: FnMut(Emit<'a>)>(&self, body: Node<'a>, sink: &mut F) {
        self.visit(body, sink);
    }

    fn text(&self, node: Node<'_>) -> &'a str {
        self.source.node_text(node)
    }

    fn children<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        for child in named_children(node) {
            self.visit(child, sink);
        }
    }

    fn field<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, field: &str, sink: &mut F) {
        if let Some(child) = node.field(field) {
            self.visit(child, sink);
        }
    }

    /// Emits `outer` around whatever `inner` walks, on the side the traversal order calls for.
    fn around<F: FnMut(Emit<'a>)>(
        &self,
        outer: Emit<'a>,
        sink: &mut F,
        inner: impl FnOnce(&Self, &mut F),
    ) {
        if self.order == Order::Pre {
            sink(outer);
        }
        inner(self, sink);
        if self.order == Order::Post {
            sink(outer);
        }
    }

    fn visit<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        match node.kind_str() {
            "call" => self.visit_call(node, sink),
            "lambda" => self.visit_lambda(node, sink),
            // `a[i]` is a call to `[]` upstream, with the subscript as its argument.
            "element_reference" => {
                self.around(Emit::of(Kind::Send, node), sink, |walk, sink| {
                    walk.children(node, sink);
                });
            }
            "binary" => self.visit_binary(node, sink),
            "unary" => self.visit_unary(node, sink),
            "assignment" => self.visit_assignment(node, sink),
            "operator_assignment" => self.visit_operator_assignment(node, sink),
            "identifier" => {
                if self.receiverless_call(node) {
                    sink(Emit::of(Kind::Send, node));
                }
            }
            // `{ name: }` stands for `{ name: name }`, whose value is a call unless the name is
            // already a variable.
            "pair" if node.field("value").is_none() => {
                if let Some(key) = node.field("key").filter(|key| !self.locals.is_lvar(*key)) {
                    sink(Emit::of(Kind::Send, key));
                }
            }
            "yield" => self.around(Emit::of(Kind::Yield, node), sink, |walk, sink| {
                walk.children(node, sink);
            }),
            // `super` and `super(…)` are `zsuper` and `super` upstream, neither of which is a call.
            "super" => {}
            "if" | "elsif" | "unless" => {
                let mut emit = Emit::of(Kind::If, node);
                emit.has_else = node
                    .field("alternative")
                    .is_some_and(|alternative| alternative.kind_str() == "else");
                self.around(emit, sink, |walk, sink| walk.children(node, sink));
            }
            "conditional" => {
                self.around(Emit::of(Kind::If, node), sink, |walk, sink| {
                    walk.children(node, sink);
                });
            }
            "if_modifier" | "unless_modifier" => {
                self.around(Emit::of(Kind::If, node), sink, |walk, sink| {
                    walk.field(node, "condition", sink);
                    walk.field(node, "body", sink);
                });
            }
            "while" | "until" => {
                let kind = if node.kind_str() == "while" {
                    Kind::While
                } else {
                    Kind::Until
                };
                self.around(Emit::of(kind, node), sink, |walk, sink| {
                    walk.children(node, sink);
                });
            }
            "while_modifier" | "until_modifier" => self.visit_modifier_loop(node, sink),
            "for" => self.visit_for(node, sink),
            "case" => {
                self.around(Emit::of(Kind::Case, node), sink, |walk, sink| {
                    walk.children(node, sink);
                });
            }
            "case_match" => {
                self.around(Emit::of(Kind::CaseMatch, node), sink, |walk, sink| {
                    walk.children(node, sink);
                });
            }
            "when" => self.around(Emit::of(Kind::When, node), sink, |walk, sink| {
                walk.children(node, sink);
            }),
            "in_clause" => self.visit_in_clause(node, sink),
            "rescue_modifier" => {
                self.around(Emit::of(Kind::Rescue, node), sink, |walk, sink| {
                    walk.field(node, "body", sink);
                    walk.field(node, "handler", sink);
                });
            }
            "block_argument" => self.visit_block_argument(node, sink),
            "method_parameters" | "block_parameters" | "lambda_parameters" => {
                self.visit_parameters(node, sink);
            }
            "exception_variable" => self.visit_exception_variable(node, sink),
            "scope_resolution" => self.field(node, "scope", sink),
            // Names, not expressions: `alias a b` and `undef a` reach RuboCop as symbols.
            "alias" | "undef" | "setter" => {}
            // A comment is no part of the tree upstream, unless the grammar mistook one for the
            // interpolations written inside a heredoc.
            "comment" => self.visit_swallowed(node, sink),
            "chained_string" => {
                for part in named_children(node) {
                    if self.fragments.swallowed(part) {
                        self.visit_swallowed(part, sink);
                    } else {
                        self.visit(part, sink);
                    }
                }
            }
            _ => {
                if let Some(rescue) = rescue_of(node) {
                    self.around(Emit::of(Kind::Rescue, rescue), sink, |walk, sink| {
                        walk.children(node, sink);
                    });
                } else {
                    self.children(node, sink);
                }
            }
        }
    }

    /// The expressions the grammar swallowed, with the call they were the argument of when that
    /// is what they were: `"%d" % [n]` is one call here that no node stands for.
    fn visit_swallowed<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let roots = self.fragments.roots(node);
        if !self.fragments.is_operator(node) {
            for root in roots {
                self.visit(root, sink);
            }
            return;
        }
        self.around(Emit::of(Kind::Send, node), sink, |walk, sink| {
            for root in roots {
                walk.visit(root, sink);
            }
        });
    }

    /// A call, and the block written on it. Upstream nests the two the other way round -- the
    /// `send` is a child of the `block` -- so the block has to be emitted around the call rather
    /// than after it.
    fn visit_call<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let block = node
            .field("block")
            .filter(|block| matches!(block.kind_str(), "block" | "do_block"));
        let Some(block) = block else {
            self.visit_bare_call(node, sink);
            return;
        };
        if self.is_numbered_block(block) {
            // A block using `_1` is a `numblock`, which none of these cops counts.
            self.visit_bare_call(node, sink);
            self.visit_block_body(block, sink);
            return;
        }
        let mut emit = Emit::of(Kind::Block, block);
        emit.iterating = self.iterating_call(node);
        self.around(emit, sink, |walk, sink| {
            walk.visit_bare_call(node, sink);
            walk.visit_block_body(block, sink);
        });
    }

    /// The call itself, without the block hanging off it.
    fn visit_bare_call<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let method = node.field("method");
        // `super(…)` wears the shape of a call here but is its own node type upstream.
        if method.is_some_and(|method| method.kind_str() == "super") {
            self.field(node, "arguments", sink);
            return;
        }
        let mut emit = Emit::of(call_kind(node), node);
        emit.comparison = method.is_some_and(|method| is_comparison(self.text(method)));
        emit.name = self.csend_receiver(node);
        if let Some(operand) = self.binary_operator_on_a_local(node) {
            self.around(emit, sink, |walk, sink| walk.children(operand, sink));
            return;
        }
        self.around(emit, sink, |walk, sink| {
            walk.field(node, "receiver", sink);
            walk.field(node, "arguments", sink);
        });
    }

    /// The right operand of `local & expr` or `local * expr`. Ruby resolves the ambiguity by what
    /// it has already seen: once the name is a local variable, the `&` is an operator rather than
    /// the start of a block-pass argument. tree-sitter has no scope to consult and always reads
    /// the argument form, which would otherwise leave a `block_pass` that upstream never built.
    fn binary_operator_on_a_local(&self, node: Node<'a>) -> Option<Node<'a>> {
        if node.field("receiver").is_some() {
            return None;
        }
        let method = node.field("method")?;
        let arguments = node.field("arguments")?;
        if method.kind_str() != "identifier"
            || self.text(arguments).starts_with('(')
            || arguments.named_child_count() != 1
            || !self.locals.is_lvar(method)
        {
            return None;
        }
        arguments
            .named_child(0)
            .filter(|child| matches!(child.kind_str(), "block_argument" | "splat_argument"))
    }

    fn visit_block_body<F: FnMut(Emit<'a>)>(&self, block: Node<'a>, sink: &mut F) {
        self.field(block, "parameters", sink);
        self.field(block, "body", sink);
    }

    /// `->(x) { … }` reaches RuboCop as a block on a call to `lambda`, parameters and all.
    fn visit_lambda<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let parts = |walk: &Self, sink: &mut F| {
            sink(Emit::of(Kind::Send, node));
            walk.field(node, "parameters", sink);
            match node.field("body") {
                Some(body) if matches!(body.kind_str(), "block" | "do_block") => {
                    walk.visit_block_body(body, sink);
                }
                Some(body) => walk.visit(body, sink),
                None => {}
            }
        };
        // `-> { _1 }` is a `numblock`, which is not one of the types these cops count.
        let numbered = node.field("parameters").is_none()
            && node
                .field("body")
                .is_some_and(|body| self.is_numbered_block(body));
        if numbered {
            parts(self, sink);
            return;
        }
        let mut emit = Emit::of(Kind::Block, node);
        emit.iterating = Some(false);
        self.around(emit, sink, parts);
    }

    fn visit_binary<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let symbol = operator(node).unwrap_or("");
        let kind = match symbol {
            "&&" | "and" => Some(Kind::And),
            "||" | "or" => Some(Kind::Or),
            _ => None,
        };
        if let Some(kind) = kind {
            self.around(Emit::of(kind, node), sink, |walk, sink| {
                walk.children(node, sink);
            });
            return;
        }
        // `/…/ =~ text` is a `match_with_lvasgn`, not a call.
        if symbol == "=~" && static_regexp_match(node) {
            self.children(node, sink);
            return;
        }
        let mut emit = Emit::of(Kind::Send, node);
        emit.comparison = is_comparison(symbol);
        self.around(emit, sink, |walk, sink| walk.children(node, sink));
    }

    fn visit_unary<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let symbol = operator(node).unwrap_or("");
        // `defined?(x)` is a node of its own upstream, and `-1` is folded into the literal.
        if symbol == "defined?" || folds_into_literal(node, symbol) {
            self.children(node, sink);
            return;
        }
        self.around(Emit::of(Kind::Send, node), sink, |walk, sink| {
            walk.children(node, sink);
        });
    }

    fn visit_assignment<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let Some(left) = node.field("left") else {
            self.children(node, sink);
            return;
        };
        if left.kind_str() == "left_assignment_list" {
            if spurious_assignment_list(left) {
                self.visit_swallowed_list(node, left, sink);
            } else {
                self.visit_multiple_assignment(node, left, sink);
            }
            return;
        }
        // `a[i] =~ /…/` is one call, not a write: the grammar split the `=~` into the `=` of an
        // assignment and a unary `~`, and the `~` is what stands in for the call.
        if let Some(right) = node
            .field("right")
            .filter(|right| split_match_operator(self.source, node, *right))
        {
            self.visit_swallowed_item(left, sink);
            self.visit(right, sink);
            return;
        }
        self.visit_single_assignment(left, node.field("right"), sink);
    }

    /// One target and the value written to it.
    fn visit_single_assignment<F: FnMut(Emit<'a>)>(
        &self,
        left: Node<'a>,
        right: Option<Node<'a>>,
        sink: &mut F,
    ) {
        let value = |walk: &Self, sink: &mut F| {
            if let Some(right) = right {
                walk.visit(right, sink);
            }
        };
        match left.kind_str() {
            "identifier" => {
                let mut emit = Emit::of(Kind::Lvasgn, left);
                let name = self.text(left);
                emit.name = Some(name);
                emit.capturing = capturing(name);
                self.around(emit, sink, value);
            }
            "instance_variable" | "class_variable" | "global_variable" => {
                self.around(Emit::of(Kind::Asgn, left), sink, value);
            }
            "constant" | "scope_resolution" => {
                self.around(Emit::of(Kind::Asgn, left), sink, |walk, sink| {
                    walk.field(left, "scope", sink);
                    value(walk, sink);
                });
            }
            // `a.b = 1` and `a[0] = 1` are one call each, with the value as their last argument.
            _ => {
                let mut emit = Emit::of(setter_kind(left), left);
                emit.setter = true;
                emit.name = self.csend_receiver(left);
                self.around(emit, sink, |walk, sink| {
                    walk.visit_target_operands(left, sink);
                    value(walk, sink);
                });
            }
        }
    }

    fn visit_multiple_assignment<F: FnMut(Emit<'a>)>(
        &self,
        node: Node<'a>,
        left: Node<'a>,
        sink: &mut F,
    ) {
        let mut emit = Emit::of(Kind::Masgn, node);
        emit.miscounted = multiple_assignment_targets(left)
            .into_iter()
            .filter(|target| self.dispatches_without_operator(*target))
            .count();
        self.around(emit, sink, |walk, sink| {
            walk.visit_targets(left, sink);
            walk.field(node, "right", sink);
        });
    }

    /// A comma-separated list the grammar mistook for a multiple assignment. `foo(a, b = c)`
    /// assigns only `b`; everything written before it is an ordinary argument standing beside it.
    fn visit_swallowed_list<F: FnMut(Emit<'a>)>(
        &self,
        node: Node<'a>,
        left: Node<'a>,
        sink: &mut F,
    ) {
        let items = named_children(left);
        let Some((&target, leading)) = items.split_last() else {
            return;
        };
        for item in leading {
            self.visit_swallowed_item(*item, sink);
        }
        let Some(right) = node.field("right") else {
            self.visit_swallowed_item(target, sink);
            return;
        };
        // The same misreading splits `=~` into an `=` and a unary `~`. Nothing is assigned then:
        // the name on the left is read, and the `~` stands in for the call the two made together.
        if split_match_operator(self.source, node, right) {
            self.visit_swallowed_item(target, sink);
            self.visit(right, sink);
            return;
        }
        self.visit_single_assignment(target, Some(right), sink);
    }

    /// One item of a swallowed list, which stands where an expression was written rather than
    /// where a name is assigned, so a bare name in it is read.
    fn visit_swallowed_item<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        if node.kind_str() == "identifier" {
            if !self.locals.is_lvar(node) && !is_keyword_literal(self.text(node)) {
                sink(Emit::of(Kind::Send, node));
            }
            return;
        }
        self.visit(node, sink);
    }

    fn visit_targets<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        match node.kind_str() {
            "identifier" => {
                let mut emit = Emit::of(Kind::Lvasgn, node);
                let name = self.text(node);
                emit.name = Some(name);
                emit.capturing = capturing(name);
                sink(emit);
            }
            "instance_variable" | "class_variable" | "global_variable" | "constant" => {
                sink(Emit::of(Kind::Asgn, node));
            }
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                for child in named_children(node) {
                    self.visit_targets(child, sink);
                }
            }
            // A `a.b` or `a[0]` target is a plain call here: only a lone assignment gives one the
            // operator that makes `setter_method?` true.
            "call" | "element_reference" => {
                let mut emit = Emit::of(setter_kind(node), node);
                emit.name = self.csend_receiver(node);
                self.around(emit, sink, |walk, sink| {
                    walk.visit_target_operands(node, sink);
                });
            }
            "scope_resolution" => {
                self.around(Emit::of(Kind::Asgn, node), sink, |walk, sink| {
                    walk.field(node, "scope", sink);
                });
            }
            _ => self.visit(node, sink),
        }
    }

    /// The parts of an assignment target that are evaluated where they stand: the receiver of
    /// `a.b = 1` and the subscript of `a[i] = 1`.
    fn visit_target_operands<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        match node.kind_str() {
            "call" => {
                self.field(node, "receiver", sink);
                self.field(node, "arguments", sink);
            }
            _ => self.children(node, sink),
        }
    }

    fn visit_operator_assignment<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let symbol = operator(node).unwrap_or("");
        let kind = match symbol {
            "||=" => Kind::OrAsgn,
            "&&=" => Kind::AndAsgn,
            _ => Kind::OpAsgn,
        };
        let mut emit = Emit::of(kind, node);
        let left = node.field("left");
        let right = node.field("right");
        emit.miscounted = [left, right]
            .into_iter()
            .flatten()
            .filter(|child| self.dispatches_without_operator(*child))
            .count();
        self.around(emit, sink, |walk, sink| {
            if let Some(left) = left {
                walk.visit_targets(left, sink);
            }
            if let Some(right) = right {
                walk.visit(right, sink);
            }
        });
    }

    fn visit_modifier_loop<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        // `begin … end while cond` is a `while_post`, which none of these cops counts.
        let post = node
            .field("body")
            .is_some_and(|body| body.kind_str() == "begin");
        let walk_parts = |walk: &Self, sink: &mut F| {
            walk.field(node, "condition", sink);
            walk.field(node, "body", sink);
        };
        if post {
            walk_parts(self, sink);
            return;
        }
        let kind = if node.kind_str() == "while_modifier" {
            Kind::While
        } else {
            Kind::Until
        };
        self.around(Emit::of(kind, node), sink, walk_parts);
    }

    fn visit_for<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        self.around(Emit::of(Kind::For, node), sink, |walk, sink| {
            if let Some(pattern) = node.field("pattern") {
                walk.visit_targets(pattern, sink);
            }
            walk.field(node, "value", sink);
            walk.field(node, "body", sink);
        });
    }

    fn visit_in_clause<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        self.around(Emit::of(Kind::InPattern, node), sink, |walk, sink| {
            if let Some(pattern) = node.field("pattern") {
                walk.visit_pattern(pattern, sink);
            }
            walk.field(node, "guard", sink);
            walk.field(node, "body", sink);
        });
    }

    /// A pattern binds names rather than reading them, so nothing in it is a call -- except the
    /// expressions `^(…)` and `^name` reach back out to.
    fn visit_pattern<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        match node.kind_str() {
            "identifier" => {}
            "variable_reference_pattern" | "expression_reference_pattern" => {
                self.children(node, sink);
            }
            "array_pattern"
            | "find_pattern"
            | "hash_pattern"
            | "alternative_pattern"
            | "as_pattern"
            | "parenthesized_pattern"
            | "keyword_pattern"
            | "splat_parameter" => {
                for child in named_children(node) {
                    self.visit_pattern(child, sink);
                }
            }
            _ => self.visit(node, sink),
        }
    }

    fn visit_block_argument<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let mut emit = Emit::of(Kind::BlockPass, node);
        emit.iterating = node
            .parent()
            .and_then(|arguments| arguments.parent())
            .filter(|call| call.kind_str() == "call")
            .and_then(|call| self.iterating_call(call));
        self.around(emit, sink, |walk, sink| walk.children(node, sink));
    }

    /// A parameter list. The block locals of `{ |x; buffer| }` are `shadowarg` nodes upstream,
    /// which belong to the same family as every other parameter and so are counted alongside them.
    fn visit_parameters<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        for child in named_children(node) {
            self.visit_parameter(child, sink);
        }
    }

    fn visit_parameter<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        match node.kind_str() {
            "identifier" => {
                let mut emit = Emit::of(Kind::Arg, node);
                emit.capturing = capturing(self.text(node));
                sink(emit);
            }
            "destructured_parameter" => {
                for child in named_children(node) {
                    self.visit_parameter(child, sink);
                }
            }
            "optional_parameter"
            | "keyword_parameter"
            | "splat_parameter"
            | "hash_splat_parameter"
            | "block_parameter" => {
                let mut emit = Emit::of(Kind::Arg, node);
                emit.capturing = node
                    .field("name")
                    .is_some_and(|name| capturing(self.text(name)));
                self.around(emit, sink, |walk, sink| {
                    if let Some(value) = node.field("value") {
                        walk.visit_default(value, sink);
                    }
                });
            }
            // `def m(...)` is a `forward_arg`, which names nothing.
            "forward_parameter" => sink(Emit::of(Kind::Arg, node)),
            _ => self.visit(node, sink),
        }
    }

    /// A default value, and the parameters the grammar folded into it: `def m(a = nil, b = nil)`
    /// parses as one parameter whose default swallowed `b`.
    fn visit_default<F: FnMut(Emit<'a>)>(&self, value: Node<'a>, sink: &mut F) {
        let Some(list) = folded_parameter_list(value) else {
            self.visit(value, sink);
            return;
        };
        let items = named_children(list);
        let Some((first, swallowed)) = items.split_first() else {
            self.visit(value, sink);
            return;
        };
        self.visit(*first, sink);
        for parameter in swallowed {
            self.visit_parameter(*parameter, sink);
        }
        if let Some(right) = value.field("right") {
            self.visit_default(right, sink);
        }
    }

    fn visit_exception_variable<F: FnMut(Emit<'a>)>(&self, node: Node<'a>, sink: &mut F) {
        let Some(target) = node.named_child(0) else {
            return;
        };
        self.visit_targets(target, sink);
    }

    /// `iterating_block?` for the call a block or a block-pass belongs to. A block on `super`
    /// answers `:super` as its method name, which is not one of the known iterating methods.
    fn iterating_call(&self, call: Node<'_>) -> Option<bool> {
        let name = match call.field("method") {
            Some(method) if method.kind_str() == "super" => "super",
            Some(method) => self.text(method),
            // `a.()` calls `call`.
            None => "call",
        };
        Some(is_iterating_method(name))
    }

    /// Whether the parser node here would be a `send`, `csend`, `yield`, `super` or `defined?` --
    /// the nodes `compound_assignment` counts, because they answer `setter_method?` while every
    /// other node type it can meet does not.
    ///
    /// A call carrying a block is a `block` node upstream rather than the call itself, and answers
    /// nothing, which is why `features -= plugins.map { … }` counts one child rather than two.
    fn dispatches_without_operator(&self, node: Node<'_>) -> bool {
        match node.kind_str() {
            "call" => node.field("block").is_none(),
            "element_reference" | "super" | "yield" => true,
            "identifier" => self.receiverless_call(node),
            "binary" => {
                !matches!(operator(node), Some("&&" | "||" | "and" | "or"))
                    && !static_regexp_match(node)
            }
            "unary" => !folds_into_literal(node, operator(node).unwrap_or("")),
            "assignment" => node
                .field("left")
                .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference")),
            _ => false,
        }
    }

    /// The receiver of a `&.` call when it is a plain local variable, which is what the repeated
    /// safe-navigation discount is keyed on.
    fn csend_receiver(&self, call: Node<'a>) -> Option<&'a str> {
        if call_kind(call) != Kind::Csend {
            return None;
        }
        let receiver = call.field("receiver")?;
        (receiver.kind_str() == "identifier" && self.locals.is_lvar(receiver))
            .then(|| self.text(receiver))
    }

    /// Whether the identifier stands where the parser would have built `(send nil :name)`.
    fn receiverless_call(&self, node: Node<'_>) -> bool {
        is_receiverless_call(node)
            && !self.locals.is_lvar(node)
            && !is_keyword_literal(self.text(node))
    }

    /// Whether a block takes its parameters implicitly through `_1`, which makes it a `numblock`
    /// rather than a `block`, and so uncounted. A numbered parameter belongs to the innermost
    /// block around it, so a nested block's `_1` is that block's, not this one's.
    fn is_numbered_block(&self, block: Node<'_>) -> bool {
        if block.field("parameters").is_some() {
            return false;
        }
        block
            .field("body")
            .is_some_and(|body| self.holds_numbered_parameter(body))
    }

    fn holds_numbered_parameter(&self, node: Node<'_>) -> bool {
        named_children(node).into_iter().any(|child| {
            if matches!(child.kind_str(), "block" | "do_block" | "lambda") {
                return false;
            }
            if child.kind_str() == "identifier" {
                return is_numbered_parameter(self.text(child));
            }
            self.holds_numbered_parameter(child)
        })
    }
}

fn is_numbered_parameter(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 2 && bytes[0] == b'_' && bytes[1].is_ascii_digit() && bytes[1] != b'0'
}

fn capturing(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('_')
}

/// `Utils::RepeatedCsendDiscount`: `my_var&.foo` written twice is one decision rather than two,
/// and an assignment to the variable in between starts the count over.
#[derive(Default)]
pub(super) struct CsendDiscount<'a> {
    seen: Vec<&'a str>,
}

impl<'a> CsendDiscount<'a> {
    /// Whether this `&.` repeats one already counted on the same variable. A receiver that is not
    /// a local variable is never discounted, and never remembered.
    pub(super) fn repeats(&mut self, receiver: Option<&'a str>) -> bool {
        let Some(name) = receiver else {
            return false;
        };
        if self.seen.contains(&name) {
            return true;
        }
        self.seen.push(name);
        false
    }

    pub(super) fn reset(&mut self, name: Option<&str>) {
        if let Some(name) = name {
            self.seen.retain(|seen| *seen != name);
        }
    }
}

/// One thing a complexity cop measures: a method, or the block of a `define_method`.
pub(super) struct Measured<'a> {
    /// The name the message reports under.
    pub name: &'a str,
    /// The body the metric is computed over, which is `node.body` upstream.
    pub body: Node<'a>,
    /// The node the offense is reported against: `MethodComplexity#location` answers the whole
    /// definition, and a block's own range starts at the call that takes it.
    pub location: Node<'a>,
}

/// Every method the complexity cops measure, in source order.
///
/// `MethodComplexity` hooks `on_def`, `on_defs` and `on_block`, and the block hook only fires for
/// `define_method` called with a literal name -- a computed name is measured by nothing at all.
pub(super) fn measured<'a>(context: &'a RuleContext<'_>, allowed: &Allowed) -> Vec<Measured<'a>> {
    let mut found = Vec::new();
    for node in context.nodes_of_any(&["method", "singleton_method", "block", "do_block"]) {
        let measured = match node.kind_str() {
            "method" | "singleton_method" => node
                .field("name")
                .map(|name| context.source.node_text(name))
                .and_then(|name| {
                    Some(Measured {
                        name,
                        body: statements(node)?,
                        location: node,
                    })
                }),
            _ => define_method_name(context, node).and_then(|name| {
                Some(Measured {
                    name,
                    body: statements(node)?,
                    location: super::support::block_location(node),
                })
            }),
        };
        match measured {
            Some(measured) if !allowed.matches(measured.name) => found.push(measured),
            _ => {}
        }
    }
    found
}

/// The body a definition holds, or `None` when it holds nothing: RuboCop accepts empty methods
/// without measuring them, and `def m; ; end` has no statement either.
fn statements<'a>(node: Node<'a>) -> Option<Node<'a>> {
    let body = node.field("body")?;
    named_children(body)
        .iter()
        .any(|child| !matches!(child.kind_str(), "empty_statement" | "comment"))
        .then_some(body)
}

/// The literal name a `define_method` block defines, when it defines one.
fn define_method_name<'a>(context: &'a RuleContext<'_>, block: Node<'a>) -> Option<&'a str> {
    let call = block
        .parent_of(context)
        .filter(|parent| parent.kind_str() == "call")?;
    if call.field("receiver").is_some() {
        return None;
    }
    let method = call.field("method")?;
    if context.source.node_text(method) != "define_method" {
        return None;
    }
    let arguments = call.field("arguments")?;
    if arguments.named_child_count() != 1 {
        return None;
    }
    let argument = arguments.named_child(0)?;
    literal_name(context.source, argument)
}

/// The text of a symbol or string literal, which is what the `define_method` pattern captures. An
/// interpolated one is a `dsym` or `dstr` upstream and matches nothing.
fn literal_name<'a>(source: &'a SourceFile, node: Node<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "simple_symbol" => Some(source.node_text(node).trim_start_matches(':')),
        "delimited_symbol" | "string" => {
            let children = named_children(node);
            match children.as_slice() {
                [] => Some(""),
                [only] if only.kind_str() == "string_content" => Some(source.node_text(*only)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `AllowedMethods` and `AllowedPatterns`, which every complexity cop consults before measuring.
pub(super) struct Allowed {
    methods: Vec<String>,
    patterns: Vec<regex::Regex>,
}

impl Allowed {
    pub(super) fn new(context: &RuleContext<'_>) -> Self {
        let methods: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
        let patterns: Vec<String> = context.setting("AllowedPatterns").unwrap_or_default();
        Self {
            methods,
            patterns: patterns
                .iter()
                .filter_map(|pattern| regex::Regex::new(pattern).ok())
                .collect(),
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.methods.iter().any(|allowed| allowed == name)
            || self.patterns.iter().any(|pattern| pattern.is_match(name))
    }
}

const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

fn is_comparison(name: &str) -> bool {
    COMPARISON_OPERATORS.contains(&name)
}

fn call_kind(call: Node<'_>) -> Kind {
    let mut cursor = call.walk();
    let safe = call
        .children(&mut cursor)
        .any(|child| !child.is_named() && child.kind_str() == "&.");
    if safe { Kind::Csend } else { Kind::Send }
}

fn setter_kind(target: Node<'_>) -> Kind {
    if target.kind_str() == "call" {
        call_kind(target)
    } else {
        Kind::Send
    }
}

/// Whether `=~` is applied to a regexp the parser can compile while it reads the file, which it
/// turns into a `match_with_lvasgn` rather than a call. One holding an interpolation is not known
/// until the program runs and stays an ordinary call.
fn static_regexp_match(node: Node<'_>) -> bool {
    node.field("left")
        .filter(|left| left.kind_str() == "regex")
        .is_some_and(|left| {
            !named_children(left)
                .iter()
                .any(|part| part.kind_str() == "interpolation")
        })
}

/// The targets of a multiple assignment, flattened the way `MasgnNode#assignments` flattens them.
fn multiple_assignment_targets<'a>(node: Node<'a>) -> Vec<Node<'a>> {
    let mut targets = Vec::new();
    collect_targets(node, &mut targets);
    targets
}

fn collect_targets<'a>(node: Node<'a>, targets: &mut Vec<Node<'a>>) {
    match node.kind_str() {
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
            for child in named_children(node) {
                collect_targets(child, targets);
            }
        }
        _ => targets.push(node),
    }
}

/// Whether the identifier stands where the parser would have built `(send nil :name)` rather than
/// a name being declared, written or called on something else.
fn is_receiverless_call(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    match parent.kind_str() {
        "call" => field_name(node, parent) != Some("method"),
        "method" | "singleton_method" => field_name(node, parent) != Some("name"),
        "assignment" | "operator_assignment" => field_name(node, parent) != Some("left"),
        "left_assignment_list"
        | "rest_assignment"
        | "destructured_left_assignment"
        | "method_parameters"
        | "block_parameters"
        | "lambda_parameters"
        | "destructured_parameter"
        | "exception_variable"
        | "alias"
        | "undef"
        | "setter" => false,
        "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter" => field_name(node, parent) != Some("name"),
        "for" => field_name(node, parent) != Some("pattern"),
        _ => true,
    }
}

/// `-1` is one integer literal upstream, while `-x` is a call to `-@`.
fn folds_into_literal(node: Node<'_>, symbol: &str) -> bool {
    if !matches!(symbol, "-" | "+") {
        return false;
    }
    node.field("operand")
        .is_some_and(|operand| matches!(operand.kind_str(), "integer" | "float" | "rational"))
}

/// The `rescue` clause a container holds, when it holds one. Upstream wraps the guarded statements
/// and every clause in a single `rescue` node, which is what these cops count -- once, however many
/// clauses were written.
fn rescue_of<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if !matches!(node.kind_str(), "body_statement" | "begin" | "block_body") {
        return None;
    }
    named_children(node)
        .into_iter()
        .find(|child| child.kind_str() == "rescue")
}

fn is_iterating_method(name: &str) -> bool {
    ITERATING_METHODS.binary_search(&name).is_ok()
}

/// `Utils::IteratingBlock::KNOWN_ITERATING_METHODS`, sorted so it can be searched.
const ITERATING_METHODS: &[&str] = &[
    "all?",
    "any?",
    "bsearch",
    "bsearch_index",
    "chain",
    "chunk",
    "chunk_while",
    "collect",
    "collect!",
    "collect_concat",
    "combination",
    "count",
    "cycle",
    "d_permutation",
    "delete_if",
    "detect",
    "drop",
    "drop_while",
    "each",
    "each_cons",
    "each_entry",
    "each_index",
    "each_key",
    "each_pair",
    "each_slice",
    "each_value",
    "each_with_index",
    "each_with_object",
    "entries",
    "fetch",
    "fetch_values",
    "filter",
    "filter_map",
    "find",
    "find_all",
    "find_index",
    "flat_map",
    "grep",
    "grep_v",
    "group_by",
    "has_key?",
    "inject",
    "keep_if",
    "lazy",
    "map",
    "map!",
    "max",
    "max_by",
    "merge",
    "merge!",
    "min",
    "min_by",
    "minmax",
    "minmax_by",
    "none?",
    "one?",
    "partition",
    "permutation",
    "product",
    "reduce",
    "reject",
    "reject!",
    "repeat",
    "repeated_combination",
    "reverse_each",
    "select",
    "select!",
    "slice_after",
    "slice_before",
    "slice_when",
    "sort",
    "sort!",
    "sort_by",
    "sum",
    "take",
    "take_while",
    "tally",
    "to_h",
    "transform_keys",
    "transform_keys!",
    "transform_values",
    "transform_values!",
    "uniq",
    "with_index",
    "with_object",
    "zip",
];
