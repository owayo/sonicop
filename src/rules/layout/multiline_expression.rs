//! `MultilineExpressionIndentation`: the syntax-tree view and the checks that
//! `Layout/MultilineMethodCallIndentation` and `Layout/MultilineOperationIndentation` share.
//!
//! The mixin walks ancestors, receivers and arguments in upstream's terms, and the grammar
//! disagrees with upstream's parser in three places that matter here:
//!
//! * a call written with a literal block is one node here and two upstream -- a `block` wrapped
//!   around the `send` -- so the send's parent is the block, while a reference to the call from
//!   outside means the block;
//! * `a.b = c` and `a[0] = 1` are an `assignment` over a call here and a single `send` to `b=` or
//!   `[]=` upstream, so the call the assignment was written over is no node of its own;
//! * a trailing run of `key: value` pairs is folded into one `hash` upstream and left as siblings
//!   of the arguments before it here.
//!
//! [`UpNode`] is a grammar node plus the role it stands in, which is what lets the ported walks
//! address the same nodes upstream's do.

use std::ops::Range;

use tree_sitter::Node;

use super::support::{begins_its_line, character_column, line_indentation};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;

/// Which of the nodes upstream's parser builds for one piece of source this stands for.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Role {
    /// The node itself.
    Plain,
    /// The `block` upstream wraps around a call written with a literal block.
    Block,
    /// The `hash` upstream folds a brace-less run of pairs into. The node is the container the
    /// pairs were written in.
    Hash,
    /// The `array` upstream wraps a lone splat assigned to something in: `x = *y` is
    /// `(lvasgn :x (array (splat ...)))` there and a bare splat here.
    Array,
}

/// One node of the tree upstream's parser would have built.
#[derive(Clone, Copy)]
pub(super) struct UpNode<'tree> {
    node: Node<'tree>,
    role: Role,
}

/// The node types the ported code asks about by name.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum UpKind {
    Send,
    Csend,
    Block,
    And,
    Or,
    If,
    While,
    Until,
    For,
    Return,
    Array,
    Kwbegin,
    /// `(...)` and `#{...}`, both of which are a `begin` carrying a `loc.begin`.
    Begin,
    Pair,
    Hash,
    Splat,
    Kwsplat,
    /// `lvasgn`, `ivasgn`, `casgn`, `masgn`, `op_asgn` and the rest of `Node#assignment?`.
    Assignment,
    Other,
}

impl UpKind {
    /// `Node#call_type?`.
    pub(super) fn call_type(self) -> bool {
        matches!(self, Self::Send | Self::Csend)
    }
}

/// Grammar nodes that hold a list of things without being a node upstream.
const TRANSPARENT: [&str; 12] = [
    "argument_list",
    "body_statement",
    "block_body",
    "then",
    "else",
    "do",
    "ensure",
    "rescue",
    "program",
    "in",
    "left_assignment_list",
    "destructured_left_assignment",
];

/// Containers a brace-less run of pairs can be written directly in.
const HASH_CONTAINERS: [&str; 3] = ["argument_list", "array", "element_reference"];

fn is_hash_element(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "pair" | "hash_splat_argument")
}

/// The literal block written on a call, which upstream keeps in a `block` node of its own.
fn block_of<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "lambda" => node
            .field("body")
            .filter(|body| matches!(body.kind_str(), "block" | "do_block")),
        _ => node.field("block"),
    }
}

/// Whether the call was fused into the setter `send` its assignment builds, which leaves it
/// without a node of its own upstream.
fn is_fused_setter_target(node: Node<'_>) -> bool {
    if !matches!(node.kind_str(), "call" | "element_reference") {
        return false;
    }
    node.parent().is_some_and(|parent| {
        parent.kind_str() == "assignment"
            && parent
                .field("left")
                .is_some_and(|left| left.id() == node.id())
    })
}

impl<'tree> UpNode<'tree> {
    /// The node a reference from elsewhere -- a receiver, an argument -- resolves to. A call
    /// written with a block is the `block` node there.
    pub(super) fn of(node: Node<'tree>) -> Self {
        let role = if block_of(node).is_some() {
            Role::Block
        } else {
            Role::Plain
        };
        Self { node, role }
    }

    pub(super) fn plain(node: Node<'tree>) -> Self {
        Self {
            node,
            role: Role::Plain,
        }
    }

    pub(super) fn same(self, other: Self) -> bool {
        self.node.id() == other.node.id() && self.role == other.role
    }

    pub(super) fn kind(self, context: &RuleContext<'_>) -> UpKind {
        match self.role {
            Role::Block => UpKind::Block,
            Role::Hash => UpKind::Hash,
            Role::Array => UpKind::Array,
            Role::Plain => plain_kind(context, self.node),
        }
    }

    /// `node.source_range`.
    pub(super) fn range(self, context: &RuleContext<'_>) -> Range<usize> {
        match self.role {
            Role::Block | Role::Array => self.node.byte_range(),
            Role::Hash => hash_run_range(self.node),
            Role::Plain => {
                if self.node.kind_str() == "lambda" {
                    // `-> () {}` is `(block (send nil :lambda) ...)`: the send is the arrow alone.
                    return self
                        .node
                        .child(0)
                        .map_or_else(|| self.node.byte_range(), |arrow| arrow.byte_range());
                }
                let Some(block) = block_of(self.node) else {
                    return self.node.byte_range();
                };
                let text = context.source.text().as_bytes();
                let mut end = block.start_byte();
                while end > self.node.start_byte() && text[end - 1].is_ascii_whitespace() {
                    end -= 1;
                }
                self.node.start_byte()..end
            }
        }
    }

    pub(super) fn line(self, context: &RuleContext<'_>) -> usize {
        context.source.line_column(self.range(context).start).0
    }

    pub(super) fn last_line(self, context: &RuleContext<'_>) -> usize {
        context.source.line_column(self.range(context).end).0
    }

    /// `Node#single_line?`, which `BlockNode` overrides to compare the block's own delimiters
    /// rather than the span of the call it hangs off: `foo\n  .a { |x| x }` is a *single-line*
    /// block, and the two chain walks that ask turn on exactly that.
    pub(super) fn single_line(self, context: &RuleContext<'_>) -> bool {
        if self.role == Role::Block
            && let Some(block) = block_of(self.node)
            && let (Some(open), Some(close)) = (
                block.child(0),
                block
                    .child_count()
                    .checked_sub(1)
                    .and_then(|last| block.child(last as u32)),
            )
        {
            return open.start_position().row == close.start_position().row;
        }
        self.line(context) == self.last_line(context)
    }

    pub(super) fn multiline(self, context: &RuleContext<'_>) -> bool {
        !self.single_line(context)
    }

    /// `Node#receiver`, whose pattern only matches a `send` and the `block` wrapped around one, so
    /// everything else -- `super`, `yield`, `defined?` -- answers with nothing.
    pub(super) fn receiver(self, context: &RuleContext<'_>) -> Option<Self> {
        let node = self.node;
        if self.role != Role::Plain && self.role != Role::Block {
            return None;
        }
        if !plain_kind(context, node).call_type() {
            return None;
        }
        match node.kind_str() {
            "call" | "method_call" => node.field("receiver").map(Self::of),
            "element_reference" => node.field("object").map(Self::of),
            "binary" => node.field("left").map(Self::of),
            "unary" => node.field("operand").map(Self::of),
            "assignment" => {
                let left = node.field("left")?;
                Self::plain(left).receiver(context)
            }
            _ => None,
        }
    }

    /// `node.loc.dot`.
    pub(super) fn dot(self, context: &RuleContext<'_>) -> Option<Range<usize>> {
        if self.role != Role::Plain {
            return None;
        }
        let node = match self.node.kind_str() {
            "assignment" => self.node.field("left")?,
            _ => self.node,
        };
        if !matches!(node.kind_str(), "call" | "method_call") {
            return None;
        }
        let operator = node.field("operator")?;
        matches!(context.source.node_text(operator), "." | "&." | "::")
            .then(|| operator.byte_range())
    }

    /// `node.loc.selector`: the method name as written.
    pub(super) fn selector(self) -> Option<Range<usize>> {
        if self.role != Role::Plain {
            return None;
        }
        let node = match self.node.kind_str() {
            "assignment" => self.node.field("left")?,
            _ => self.node,
        };
        match node.kind_str() {
            "call" | "method_call" => node.field("method").map(|m| m.byte_range()),
            "binary" | "unary" => node.field("operator").map(|operator| operator.byte_range()),
            _ => None,
        }
    }

    /// `node.loc.begin`: the `(` a call's arguments were written in, when there is one.
    pub(super) fn arguments_begin(self, context: &RuleContext<'_>) -> Option<Range<usize>> {
        let list = self.node.field("arguments")?;
        let open = list.child(0)?;
        (context.source.node_text(open) == "(").then(|| open.byte_range())
    }

    /// `ParameterizedNode#parenthesized?`.
    pub(super) fn parenthesized(self, context: &RuleContext<'_>) -> bool {
        self.role == Role::Plain
            && matches!(self.node.kind_str(), "call" | "method_call")
            && self.arguments_begin(context).is_some()
    }

    pub(super) fn method_name(self, context: &RuleContext<'_>) -> Option<String> {
        if self.role != Role::Plain {
            return None;
        }
        let (node, setter) = match self.node.kind_str() {
            "assignment" => (self.node.field("left")?, true),
            _ => (self.node, false),
        };
        let name = match node.kind_str() {
            "call" | "method_call" => {
                let method = node.field("method")?;
                context.source.node_text(method).to_owned()
            }
            "element_reference" => "[]".to_owned(),
            "binary" | "unary" => {
                let operator = node.field("operator")?;
                context.source.node_text(operator).to_owned()
            }
            _ => return None,
        };
        Some(if setter { format!("{name}=") } else { name })
    }

    /// `MethodIdentifierPredicates#assignment_method?`.
    fn assignment_method(self, context: &RuleContext<'_>) -> bool {
        let Some(name) = self.method_name(context) else {
            return false;
        };
        name.ends_with('=') && !matches!(name.as_str(), "==" | "!=" | "<=" | ">=" | "===")
    }

    /// `MethodIdentifierPredicates#operator_method?`.
    pub(super) fn operator_method(self, context: &RuleContext<'_>) -> bool {
        let Some(name) = self.method_name(context) else {
            return false;
        };
        OPERATOR_METHODS.contains(&name.as_str())
    }

    /// `MethodDispatchNode#setter_method?`, which is `loc.operator` being set: only the sends an
    /// assignment builds carry one.
    fn setter_method(self) -> bool {
        self.role == Role::Plain
            && self.node.kind_str() == "assignment"
            && self
                .node
                .field("left")
                .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference"))
    }

    /// `SendNode#arguments`, with the brace-less hash folded back into one argument.
    pub(super) fn arguments(self, context: &RuleContext<'_>) -> Vec<Self> {
        if self.role != Role::Plain {
            return Vec::new();
        }
        match self.node.kind_str() {
            "call" | "method_call" => match self.node.field("arguments") {
                Some(list) => fold_arguments(list, &children_of(list)),
                None => Vec::new(),
            },
            "element_reference" => {
                let indices: Vec<Node<'tree>> =
                    children_of(self.node).into_iter().skip(1).collect();
                fold_arguments(self.node, &indices)
            }
            "binary" if plain_kind(context, self.node) == UpKind::Send => self
                .node
                .field("right")
                .map_or_else(Vec::new, |right| vec![Self::of(right)]),
            "assignment" => {
                let Some(left) = self.node.field("left") else {
                    return Vec::new();
                };
                let mut arguments = if left.kind_str() == "element_reference" {
                    Self::plain(left).arguments(context)
                } else {
                    Vec::new()
                };
                if let Some(right) = self.node.field("right") {
                    arguments.push(Self::of(right));
                }
                arguments
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn first_argument(self, context: &RuleContext<'_>) -> Option<Self> {
        self.arguments(context).first().copied()
    }

    fn last_argument(self, context: &RuleContext<'_>) -> Option<Self> {
        self.arguments(context).last().copied()
    }

    /// `SendNode#block_node`: the `block` upstream wrapped this call in.
    pub(super) fn block_node(self) -> Option<Self> {
        (self.role == Role::Plain && block_of(self.node).is_some()).then_some(Self {
            node: self.node,
            role: Role::Block,
        })
    }

    /// `BlockNode#send_node`.
    pub(super) fn send_node(self) -> Self {
        match self.role {
            Role::Block => Self::plain(self.node),
            _ => self,
        }
    }

    /// `BlockNode#body`: the statements between the delimiters, which upstream has as a single
    /// node and the grammar keeps as a list.
    pub(super) fn body(self) -> Option<Range<usize>> {
        let block = block_of(self.node)?;
        let body = block.field("body")?;
        let statements = children_of(body)
            .into_iter()
            .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
            .collect::<Vec<_>>();
        let first = statements.first()?;
        let last = statements.last()?;
        Some(first.start_byte()..last.end_byte())
    }

    /// `block_node.loc.end`: the `}` or `end` the block closes with.
    pub(super) fn block_end(self) -> Option<Range<usize>> {
        let block = block_of(self.node)?;
        let last = block.child(block.child_count().checked_sub(1)? as u32)?;
        matches!(last.kind_str(), "}" | "end").then(|| last.byte_range())
    }

    /// `node.parent`, in the shape upstream's parser gives the tree.
    /// The grammar's own name for the node, which the ported code only asks for where upstream
    /// tests a type the model has no case for.
    pub(super) fn ts_kind(self) -> &'static str {
        self.node.kind_str()
    }

    /// The grammar node itself, for the analyses that work on the tree rather than on the model.
    pub(super) fn raw(self) -> Node<'tree> {
        self.node
    }

    /// Whether the call was fused into the setter send its assignment builds.
    pub(super) fn is_fused_setter_target(self) -> bool {
        self.role == Role::Plain && is_fused_setter_target(self.node)
    }

    /// One of the node's own fields, for the few places the ported code reads a part the model
    /// does not otherwise name.
    pub(super) fn node_field(self, field: &str) -> Option<Range<usize>> {
        self.node.field(field).map(|child| child.byte_range())
    }

    /// `node.each_descendant(:any_block).first`: the first literal block written inside the node,
    /// which is a `block` wrapped around a call upstream and a child of that call here.
    pub(super) fn first_descendant_block(self) -> Option<Self> {
        let mut stack = vec![self.node];
        while let Some(node) = stack.pop() {
            if !node.id().eq(&self.node.id()) && block_of(node).is_some() {
                return Some(Self {
                    node,
                    role: Role::Block,
                });
            }
            let mut cursor = node.walk();
            let mut children: Vec<Node<'tree>> = node.named_children(&mut cursor).collect();
            children.reverse();
            stack.extend(children);
        }
        None
    }

    pub(super) fn parent(self) -> Option<Self> {
        match self.role {
            Role::Block | Role::Array => raw_parent(self.node),
            Role::Hash => {
                if self.node.kind_str() == "argument_list" {
                    raw_parent(self.node)
                } else {
                    Some(Self::plain(self.node))
                }
            }
            Role::Plain => {
                if block_of(self.node).is_some() {
                    return Some(Self {
                        node: self.node,
                        role: Role::Block,
                    });
                }
                if is_lone_assigned_splat(self.node) {
                    return Some(Self {
                        node: self.node,
                        role: Role::Array,
                    });
                }
                raw_parent(self.node)
            }
        }
    }

    pub(super) fn ancestors(self) -> Ancestors<'tree> {
        Ancestors {
            current: Some(self),
        }
    }
}

/// `node.each_ancestor`.
pub(super) struct Ancestors<'tree> {
    current: Option<UpNode<'tree>>,
}

impl<'tree> Iterator for Ancestors<'tree> {
    type Item = UpNode<'tree>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.current?.parent();
        self.current = next;
        next
    }
}

/// The method names `Node::OPERATOR_METHODS` lists.
const OPERATOR_METHODS: [&str; 27] = [
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "!", "!=", "!~", "+@", "-@", "[]", "[]=",
];

fn children_of<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
        .collect()
}

/// Groups a run of `key: value` pairs into the one `hash` argument upstream's parser folds it into.
fn fold_arguments<'tree>(container: Node<'tree>, children: &[Node<'tree>]) -> Vec<UpNode<'tree>> {
    let mut arguments = Vec::new();
    let mut index = 0;
    while index < children.len() {
        if is_hash_element(children[index]) {
            while index < children.len() && is_hash_element(children[index]) {
                index += 1;
            }
            arguments.push(UpNode {
                node: container,
                role: Role::Hash,
            });
            continue;
        }
        arguments.push(UpNode::of(children[index]));
        index += 1;
    }
    arguments
}

/// The span of the last run of brace-less pairs in a container, which is the only run Ruby's
/// grammar allows.
fn hash_run_range(container: Node<'_>) -> Range<usize> {
    let children = children_of(container);
    let mut end = children.len();
    while end > 0 && !is_hash_element(children[end - 1]) {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_hash_element(children[start - 1]) {
        start -= 1;
    }
    if start == end {
        return container.byte_range();
    }
    children[start].start_byte()..children[end - 1].end_byte()
}

/// Climbs to the node upstream would call this one's parent, stepping over the containers the
/// parser has no node for.
fn raw_parent<'tree>(node: Node<'tree>) -> Option<UpNode<'tree>> {
    let mut current = node;
    loop {
        let parent = current.parent()?;
        if parent.kind_str() == "block" || parent.kind_str() == "do_block" {
            // The body of a literal block hangs off the `block` node upstream, whose own parent is
            // the parent of the call the block was written on.
            return Some(UpNode {
                node: parent.parent()?,
                role: Role::Block,
            });
        }
        if is_hash_element(current) && HASH_CONTAINERS.contains(&parent.kind_str()) {
            return Some(UpNode {
                node: parent,
                role: Role::Hash,
            });
        }
        if TRANSPARENT.contains(&parent.kind_str())
            || is_fused_setter_target(parent)
            || is_defined_parentheses(parent)
        {
            current = parent;
            continue;
        }
        return Some(UpNode::plain(parent));
    }
}

fn plain_kind(context: &RuleContext<'_>, node: Node<'_>) -> UpKind {
    match node.kind_str() {
        // `super args` and `super(args)` are one call node here and a `super` upstream, which is
        // no `send` at all: nothing that looks for an enclosing method call may stop at one.
        "call" | "method_call" => match node.field("method") {
            Some(method) if method.kind_str() == "super" => UpKind::Other,
            _ => call_kind(context, node),
        },
        "element_reference" => UpKind::Send,
        "binary" => match node.field("operator") {
            Some(operator) => match context.source.node_text(operator) {
                "&&" | "and" => UpKind::And,
                "||" | "or" => UpKind::Or,
                _ => UpKind::Send,
            },
            None => UpKind::Send,
        },
        // `defined?` is a node of its own upstream rather than a call to `!`.
        "unary" => match node.field("operator") {
            Some(operator) if context.source.node_text(operator) == "defined?" => UpKind::Other,
            _ => UpKind::Send,
        },
        "assignment" => match node.field("left") {
            Some(left) if left.kind_str() == "call" => call_kind(context, left),
            Some(left) if left.kind_str() == "element_reference" => UpKind::Send,
            _ => UpKind::Assignment,
        },
        "operator_assignment" => UpKind::Assignment,
        "if" | "unless" | "elsif" | "if_modifier" | "unless_modifier" | "conditional" => UpKind::If,
        // `begin ... end while cond` is a `while_post`, which is not the `while` the keyword walks
        // look for.
        "while" => UpKind::While,
        "until" => UpKind::Until,
        "while_modifier" => match post_loop(node) {
            true => UpKind::Other,
            false => UpKind::While,
        },
        "until_modifier" => match post_loop(node) {
            true => UpKind::Other,
            false => UpKind::Until,
        },
        "for" => UpKind::For,
        "return" => UpKind::Return,
        "array" | "right_assignment_list" => UpKind::Array,
        "begin" => UpKind::Kwbegin,
        // The parentheses of `defined?(x)` belong to the `defined?` node upstream, so they are not
        // the `begin` a written-out group would be.
        "parenthesized_statements" if is_defined_parentheses(node) => UpKind::Other,
        "parenthesized_statements" | "interpolation" => UpKind::Begin,
        "pair" => UpKind::Pair,
        "hash" => UpKind::Hash,
        "splat_argument" => UpKind::Splat,
        "hash_splat_argument" => UpKind::Kwsplat,
        _ => UpKind::Other,
    }
}

fn call_kind(context: &RuleContext<'_>, node: Node<'_>) -> UpKind {
    match node.field("operator") {
        Some(operator) if context.source.node_text(operator) == "&." => UpKind::Csend,
        _ => UpKind::Send,
    }
}

/// Whether the splat is the whole right-hand side of an assignment, which upstream's parser wraps
/// in an `array` -- one of the types that stops the walk looking for an assignment to align under.
fn is_lone_assigned_splat(node: Node<'_>) -> bool {
    node.kind_str() == "splat_argument"
        && node.parent().is_some_and(|parent| {
            parent.kind_str() == "assignment"
                && parent
                    .field("right")
                    .is_some_and(|right| right.id() == node.id())
        })
}

/// Whether the group is the argument list `defined?` was written with, which upstream keeps in the
/// `defined?` node's own location rather than as a node.
fn is_defined_parentheses(node: Node<'_>) -> bool {
    node.kind_str() == "parenthesized_statements"
        && node.parent().is_some_and(|parent| {
            parent.kind_str() == "unary"
                && parent
                    .field("operator")
                    .is_some_and(|operator| operator.kind_str() == "defined?")
        })
}

fn post_loop(node: Node<'_>) -> bool {
    node.field("body")
        .is_some_and(|body| body.kind_str() == "begin")
}

/// `Util#within_node?`.
pub(super) fn within(inner: &Range<usize>, outer: &Range<usize>) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
}

/// The state the mixin's checks need from the cop that includes it.
pub(super) struct Mixin<'a, 'tree> {
    pub(super) context: &'a RuleContext<'tree>,
    /// `Alignment#configured_indentation_width`.
    pub(super) width: i64,
    /// `Layout/IndentationWidth`'s own `Width`, which prefix keywords add on top.
    pub(super) keyword_width: i64,
    /// Which bare identifiers upstream's parser reads as local variables rather than as calls
    /// without a receiver. The analysis is deferred until a chain actually asks.
    locals: LocalVariables<'a, 'tree>,
}

/// The tail `operation_description` appends to a message.
pub(super) enum Tail {
    Keyword(String),
    Assignment,
    Default,
}

impl std::fmt::Display for Tail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keyword(text) => formatter.write_str(text),
            Self::Assignment => formatter.write_str("an expression in an assignment"),
            Self::Default => formatter.write_str("an expression"),
        }
    }
}

impl<'tree> Mixin<'_, 'tree> {
    pub(super) fn new<'a>(
        context: &'a RuleContext<'tree>,
        cop_width: Option<i64>,
    ) -> Mixin<'a, 'tree> {
        let keyword_width = context
            .setting_of::<i64>("Layout/IndentationWidth", "Width")
            .unwrap_or(2);
        Mixin {
            context,
            width: cop_width.unwrap_or(keyword_width),
            keyword_width,
            locals: LocalVariables::new(context),
        }
    }

    /// `Node#call_type?`, including the bare identifiers upstream's parser turns into a `send`
    /// without a receiver. Only a name it has seen assigned in the enclosing scope is an `lvar`.
    pub(super) fn call_type(&self, node: UpNode<'tree>) -> bool {
        if node.kind(self.context).call_type() {
            return true;
        }
        node.ts_kind() == "identifier" && !self.locals.is_lvar(node.raw())
    }

    /// `MultilineExpressionIndentation#left_hand_side`: in a chain of calls the top call is the
    /// base every following line is indented against.
    pub(super) fn left_hand_side(&self, lhs: UpNode<'tree>) -> UpNode<'tree> {
        let mut lhs = lhs;
        while let Some(parent) = lhs.parent() {
            if parent.kind(self.context).call_type()
                && parent.dot(self.context).is_some()
                && !parent.assignment_method(self.context)
            {
                lhs = parent;
            } else {
                break;
            }
        }
        lhs
    }

    /// `MultilineExpressionIndentation#correct_indentation`.
    pub(super) fn correct_indentation(&self, node: UpNode<'tree>) -> i64 {
        match self.keyword_ancestor(node) {
            Some(keyword) if !self.postfix_conditional(keyword) => self.width + self.keyword_width,
            _ => self.width,
        }
    }

    /// `MultilineExpressionIndentation#indentation`: where the line the node starts on begins.
    pub(super) fn indentation(&self, node: UpNode<'tree>) -> i64 {
        line_indentation(self.context, node.range(self.context).start)
    }

    /// `MultilineExpressionIndentation#kw_node_with_special_indentation`.
    pub(super) fn keyword_ancestor(&self, node: UpNode<'tree>) -> Option<UpNode<'tree>> {
        let range = node.range(self.context);
        node.ancestors().find(|ancestor| {
            if !matches!(
                ancestor.kind(self.context),
                UpKind::For | UpKind::If | UpKind::While | UpKind::Until | UpKind::Return
            ) {
                return false;
            }
            if ancestor.node.kind_str() == "conditional" {
                return false;
            }
            self.indented_keyword_expression(*ancestor)
                .is_some_and(|expression| within(&range, &expression))
        })
    }

    /// `MultilineExpressionIndentation#indented_keyword_expression`.
    pub(super) fn indented_keyword_expression(&self, node: UpNode<'tree>) -> Option<Range<usize>> {
        if node.kind(self.context) == UpKind::For {
            let value = node.node.field("value")?;
            let collection = children_of(value).into_iter().next()?;
            return Some(collection.byte_range());
        }
        match node.node.kind_str() {
            "return" => children_of(node.node)
                .into_iter()
                .next()
                .and_then(|list| match list.kind_str() {
                    "argument_list" => children_of(list).into_iter().next(),
                    _ => Some(list),
                })
                .map(|first| first.byte_range()),
            _ => node
                .node
                .field("condition")
                .map(|condition| condition.byte_range()),
        }
    }

    /// `MultilineExpressionIndentation#postfix_conditional?`.
    fn postfix_conditional(&self, node: UpNode<'tree>) -> bool {
        matches!(node.node.kind_str(), "if_modifier" | "unless_modifier")
    }

    /// `MultilineExpressionIndentation#operation_description`.
    pub(super) fn operation_description(&self, node: UpNode<'tree>, rhs: &Range<usize>) -> Tail {
        if let Some(keyword) = self.keyword_ancestor(node) {
            return Tail::Keyword(self.keyword_message_tail(keyword));
        }
        if self.part_of_assignment_rhs(node, Some(rhs)).is_some() {
            return Tail::Assignment;
        }
        Tail::Default
    }

    /// `MultilineExpressionIndentation#keyword_message_tail`.
    fn keyword_message_tail(&self, node: UpNode<'tree>) -> String {
        let keyword = self.keyword_source(node);
        let kind = if keyword == "for" {
            "collection"
        } else {
            "condition"
        };
        let article = if keyword.starts_with('i') || keyword.starts_with('u') {
            "an"
        } else {
            "a"
        };
        format!("a {kind} in {article} `{keyword}` statement")
    }

    /// `node.loc.keyword.source`.
    fn keyword_source(&self, node: UpNode<'tree>) -> String {
        let mut cursor = node.node.walk();
        let keyword = node
            .node
            .children(&mut cursor)
            .find(|child| !child.is_named() && !child.kind_str().is_empty());
        match keyword {
            Some(keyword) => self.context.source.node_text(keyword).to_owned(),
            None => node.node.kind_str().to_owned(),
        }
    }

    /// `MultilineExpressionIndentation#argument_in_method_call`.
    pub(super) fn argument_in_method_call(
        &self,
        node: UpNode<'tree>,
        with_parentheses: bool,
    ) -> Option<UpNode<'tree>> {
        let range = node.range(self.context);
        for ancestor in node.ancestors() {
            match ancestor.kind(self.context) {
                UpKind::Block => {
                    // A block between the node and the call means the node is part of the block,
                    // not an argument of anything.
                    if !is_numbered_block(self.context, ancestor.node) {
                        return None;
                    }
                    continue;
                }
                UpKind::Send => {}
                _ => continue,
            }
            if ancestor.setter_method() {
                continue;
            }
            if with_parentheses && !ancestor.parenthesized(self.context) {
                continue;
            }
            if ancestor
                .arguments(self.context)
                .into_iter()
                .any(|argument| within(&range, &argument.range(self.context)))
            {
                return Some(ancestor);
            }
        }
        None
    }

    /// `MultilineExpressionIndentation#part_of_assignment_rhs`.
    pub(super) fn part_of_assignment_rhs(
        &self,
        node: UpNode<'tree>,
        candidate: Option<&Range<usize>>,
    ) -> Option<UpNode<'tree>> {
        for ancestor in node.ancestors() {
            if self.disqualified_rhs(candidate, ancestor) {
                return None;
            }
            if self.valid_rhs(candidate, ancestor) {
                return Some(ancestor);
            }
        }
        None
    }

    fn disqualified_rhs(&self, candidate: Option<&Range<usize>>, ancestor: UpNode<'tree>) -> bool {
        if matches!(
            ancestor.kind(self.context),
            UpKind::If
                | UpKind::While
                | UpKind::Until
                | UpKind::For
                | UpKind::Return
                | UpKind::Array
                | UpKind::Kwbegin
        ) {
            return true;
        }
        if ancestor.kind(self.context) != UpKind::Block
            || is_numbered_block(self.context, ancestor.node)
        {
            return false;
        }
        match (candidate, ancestor.body()) {
            (Some(candidate), Some(body)) => within(candidate, &body),
            _ => false,
        }
    }

    fn valid_rhs(&self, candidate: Option<&Range<usize>>, ancestor: UpNode<'tree>) -> bool {
        match ancestor.kind(self.context) {
            UpKind::Send | UpKind::Csend => {
                ancestor.setter_method()
                    && self.valid_rhs_candidate(
                        candidate,
                        ancestor
                            .last_argument(self.context)
                            .map(|last| last.range(self.context)),
                    )
            }
            UpKind::Assignment => self.valid_rhs_candidate(
                candidate,
                self.assignment_rhs(ancestor)
                    .map(|rhs| rhs.range(self.context)),
            ),
            _ => false,
        }
    }

    fn valid_rhs_candidate(
        &self,
        candidate: Option<&Range<usize>>,
        node: Option<Range<usize>>,
    ) -> bool {
        match (candidate, node) {
            (None, _) => true,
            (Some(candidate), Some(node)) => within(candidate, &node),
            (Some(_), None) => false,
        }
    }

    /// `MultilineExpressionIndentation#assignment_rhs`.
    pub(super) fn assignment_rhs(&self, node: UpNode<'tree>) -> Option<UpNode<'tree>> {
        match node.kind(self.context) {
            UpKind::Send | UpKind::Csend => node.last_argument(self.context),
            _ => node.node.field("right").map(UpNode::of),
        }
    }

    /// `MultilineExpressionIndentation#not_for_this_cop?`.
    pub(super) fn not_for_this_cop(&self, node: UpNode<'tree>) -> bool {
        let range = node.range(self.context);
        node.ancestors().any(|ancestor| {
            self.grouped_expression(ancestor) || self.inside_arg_list_parentheses(&range, ancestor)
        })
    }

    /// `MultilineExpressionIndentation#grouped_expression?`.
    pub(super) fn grouped_expression(&self, node: UpNode<'tree>) -> bool {
        node.kind(self.context) == UpKind::Begin
    }

    /// `MultilineExpressionIndentation#inside_arg_list_parentheses?`.
    pub(super) fn inside_arg_list_parentheses(
        &self,
        range: &Range<usize>,
        ancestor: UpNode<'tree>,
    ) -> bool {
        if !ancestor.kind(self.context).call_type() || !ancestor.parenthesized(self.context) {
            return false;
        }
        let Some(begin) = ancestor.arguments_begin(self.context) else {
            return false;
        };
        range.start > begin.start && range.end < ancestor.range(self.context).end
    }

    /// `Util#begins_its_line?`.
    pub(super) fn begins_its_line(&self, range: &Range<usize>) -> bool {
        begins_its_line(self.context, range.start)
    }

    pub(super) fn column(&self, offset: usize) -> i64 {
        character_column(self.context, offset)
    }

    pub(super) fn line(&self, offset: usize) -> usize {
        self.context.source.line_column(offset).0
    }
}

/// Whether the block upstream would build a `numblock` or an `itblock` rather than a `block`,
/// which is what the two strict `block_type?` tests in the mixin turn on.
///
/// A block with no parameter list whose body names `_1`..`_9` takes those as its parameters, and a
/// nested block captures the names for itself.
fn is_numbered_block(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    let Some(block) = block_of(call) else {
        return false;
    };
    if block.field("parameters").is_some() {
        return false;
    }
    let Some(body) = block.field("body") else {
        return false;
    };
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if node.kind_str() == "identifier" {
            let text = context.source.node_text(node).as_bytes();
            if text.len() == 2 && text[0] == b'_' && text[1].is_ascii_digit() && text[1] != b'0' {
                return true;
            }
        }
        if matches!(node.kind_str(), "block" | "do_block") {
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    false
}
