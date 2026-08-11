//! Which identifiers name a local variable.
//!
//! tree-sitter spells a receiverless call and a local variable read with the same node type, while
//! the parser upstream decides between them as it goes: a name is a variable once the current
//! static scope has seen it declared, and a method call until then. Every cop here that counts
//! method calls -- `Metrics/AbcSize` counts each one as a branch -- has to make the same
//! distinction, so the declarations are replayed in the order Ruby's own parser would have met
//! them.
//!
//! Only the question "is this identifier a variable read?" is answered. Where an assignment is
//! read afterwards, and whether it is ever read at all, belong to `Lint/UselessAssignment` and are
//! deliberately not tracked here.

use std::collections::HashSet;

use tree_sitter::Node;

use super::fragments::Fragments;
use crate::rules::RuleContext;
use crate::source::SourceFile;

pub(super) struct Locals {
    lvars: HashSet<usize>,
}

impl Locals {
    pub(super) fn new(context: &RuleContext<'_>, fragments: &Fragments) -> Self {
        let mut walker = Walker {
            source: context.source,
            fragments,
            stack: vec![Frame::new(false)],
            lvars: HashSet::new(),
        };
        walker.visit(context.root_node());
        Self {
            lvars: walker.lvars,
        }
    }

    /// Whether the parser upstream would have built an `lvar` here rather than `(send nil :name)`.
    pub(super) fn is_lvar(&self, node: Node<'_>) -> bool {
        self.lvars.contains(&node.id())
    }
}

/// One static scope. A block sees the names of the scope around it; a method, class or module
/// starts from nothing.
struct Frame {
    names: HashSet<String>,
    block: bool,
}

impl Frame {
    fn new(block: bool) -> Self {
        Self {
            names: HashSet::new(),
            block,
        }
    }
}

struct Walker<'a> {
    source: &'a SourceFile,
    fragments: &'a Fragments,
    stack: Vec<Frame>,
    lvars: HashSet<usize>,
}

/// The node kinds that open a static scope, with the fields that are still evaluated outside it:
/// the receiver of `def obj.name`, a superclass expression, the value of `class << expr`.
fn scope_kind(kind: &str) -> Option<(bool, &'static [&'static str])> {
    match kind {
        "method" => Some((false, &[])),
        "singleton_method" => Some((false, &["object"])),
        "class" => Some((false, &["name", "superclass"])),
        "module" => Some((false, &["name"])),
        "singleton_class" => Some((false, &["value"])),
        "block" | "do_block" | "lambda" => Some((true, &[])),
        _ => None,
    }
}

impl Walker<'_> {
    fn text(&self, node: Node<'_>) -> &str {
        self.source.node_text(node)
    }

    fn declare(&mut self, name: &str) {
        if let Some(frame) = self.stack.last_mut() {
            frame.names.insert(name.to_owned());
        }
    }

    fn declared(&self, name: &str) -> bool {
        for frame in self.stack.iter().rev() {
            if frame.names.contains(name) {
                return true;
            }
            if !frame.block {
                return false;
            }
        }
        false
    }

    fn visit_children(&mut self, node: Node<'_>) {
        for child in named_children(node) {
            self.visit(child);
        }
    }

    fn visit_field(&mut self, node: Node<'_>, field: &str) {
        if let Some(child) = node.child_by_field_name(field) {
            self.visit(child);
        }
    }

    fn visit(&mut self, node: Node<'_>) {
        if scope_kind(node.kind()).is_some() && !inline_block(node) {
            self.visit_scope(node);
            return;
        }
        match node.kind() {
            "assignment" => self.visit_assignment(node),
            "operator_assignment" => self.visit_operator_assignment(node),
            "identifier" => self.visit_identifier(node),
            "call" => self.visit_call(node),
            "method_parameters" | "block_parameters" | "lambda_parameters" => {
                self.visit_parameters(node);
            }
            "exception_variable" => self.visit_exception_variable(node),
            // The grammar reads the interpolations after a `#` inside a heredoc as a comment, and
            // a `%` applied to a string as one more string; both hold names that are read.
            "comment" | "chained_string" => self.visit_swallowed(node),
            // `{ name: }` is Ruby's shorthand for `{ name: name }`, so the key reads the variable.
            "pair" if node.child_by_field_name("value").is_none() => {
                if let Some(key) = node.child_by_field_name("key")
                    && self.declared(self.text(key).trim_end_matches(':'))
                {
                    self.lvars.insert(key.id());
                }
            }
            "for" => {
                self.visit_field(node, "value");
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.declare_targets(pattern);
                }
                self.visit_field(node, "body");
            }
            "binary" => self.visit_binary(node),
            "in_clause" | "match_pattern" => {
                self.visit_field(node, "value");
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.declare_pattern(pattern);
                }
                self.visit_field(node, "guard");
                self.visit_field(node, "body");
            }
            _ => self.visit_children(node),
        }
    }

    /// `->(x) { }` holds its parameters one node above its body, so the braces are not a scope of
    /// their own: the whole lambda is.
    fn visit_scope(&mut self, node: Node<'_>) {
        let (block, outer_fields) = scope_kind(node.kind()).expect("checked by the caller");
        let mut outer = Vec::new();
        for field in outer_fields {
            if let Some(child) = node.child_by_field_name(field) {
                outer.push(child.id());
                self.visit(child);
            }
        }
        self.stack.push(Frame::new(block));
        // A block written without parameters that reaches for `_1` gets them implicitly, and the
        // parser upstream reads every such name as a variable of that block.
        if block && node.child_by_field_name("parameters").is_none() {
            for index in 1..=9 {
                self.declare(&format!("_{index}"));
            }
        }
        for child in named_children(node) {
            if !outer.contains(&child.id()) {
                self.visit(child);
            }
        }
        self.stack.pop();
    }

    fn visit_assignment(&mut self, node: Node<'_>) {
        let Some(left) = node.child_by_field_name("left") else {
            self.visit_children(node);
            return;
        };
        if left.kind() == "left_assignment_list" && spurious_assignment_list(left) {
            self.visit_swallowed_list(node, left);
            return;
        }
        // A `=~` the grammar split in two writes to nothing: its left-hand side is read.
        if let Some(right) = node
            .child_by_field_name("right")
            .filter(|right| split_match_operator(self.source, node, *right))
        {
            self.reference_item(left);
            self.visit(right);
            return;
        }
        // The target is declared before the value is read, which is what makes the `a` of `a = a`
        // a variable rather than a call.
        self.declare_targets(left);
        if left.kind() != "identifier" {
            self.visit_target_expressions(left);
        }
        self.visit_field(node, "right");
    }

    /// A comma-separated list the grammar mistook for a multiple assignment: only its last item
    /// is written to, and a `=~` it split in two writes to nothing at all.
    fn visit_swallowed_list(&mut self, node: Node<'_>, left: Node<'_>) {
        let items = named_children(left);
        let Some((&target, leading)) = items.split_last() else {
            return;
        };
        for item in leading {
            self.reference_item(*item);
        }
        let matched = node
            .child_by_field_name("right")
            .is_some_and(|right| split_match_operator(self.source, node, right));
        if matched {
            self.reference_item(target);
        } else {
            self.declare_targets(target);
            if target.kind() != "identifier" {
                self.visit_target_expressions(target);
            }
        }
        self.visit_field(node, "right");
    }

    /// One item of a swallowed list, which is an expression rather than a name being written.
    fn reference_item(&mut self, node: Node<'_>) {
        if node.kind() == "identifier" {
            if self.declared(self.text(node)) {
                self.lvars.insert(node.id());
            }
            return;
        }
        self.visit(node);
    }

    fn visit_operator_assignment(&mut self, node: Node<'_>) {
        let Some(left) = node.child_by_field_name("left") else {
            self.visit_children(node);
            return;
        };
        if left.kind() == "identifier" {
            self.declare(&self.text(left).to_owned());
        } else {
            self.visit(left);
        }
        self.visit_field(node, "right");
    }

    /// The names an assignment target declares. Anything that is not a bare name -- a call, an
    /// index, an instance variable -- declares nothing.
    fn declare_targets(&mut self, node: Node<'_>) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node).to_owned();
                self.declare(&name);
            }
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                for child in named_children(node) {
                    self.declare_targets(child);
                }
            }
            _ => {}
        }
    }

    /// The parts of an assignment target that are ordinary expressions: the receiver of `a.b = 1`
    /// and the subscript of `a[i] = 1` are both evaluated where they stand.
    fn visit_target_expressions(&mut self, node: Node<'_>) {
        match node.kind() {
            "identifier" => {}
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                for child in named_children(node) {
                    self.visit_target_expressions(child);
                }
            }
            _ => self.visit(node),
        }
    }

    /// A call, with the one shape the grammar cannot resolve on its own. `local & expr` reads as a
    /// call taking a block-pass argument until the name is known to be a variable, at which point
    /// the `&` is an operator and the name a read.
    fn visit_call(&mut self, node: Node<'_>) {
        if node.child_by_field_name("receiver").is_none()
            && let Some(method) = node.child_by_field_name("method")
            && let Some(arguments) = node.child_by_field_name("arguments")
            && method.kind() == "identifier"
            && !self.text(arguments).starts_with('(')
            && arguments.named_child_count() == 1
            && arguments
                .named_child(0)
                .is_some_and(|child| matches!(child.kind(), "block_argument" | "splat_argument"))
            && self.declared(self.text(method))
        {
            self.lvars.insert(method.id());
        }
        self.visit_children(node);
    }

    /// The expressions the grammar swallowed, walked where they were written so that they see the
    /// variables the scope around them had declared by then.
    fn visit_swallowed(&mut self, node: Node<'_>) {
        for child in named_children(node) {
            if self.fragments.swallowed(child) {
                for root in self.fragments.roots(child) {
                    self.visit(root);
                }
            } else {
                self.visit(child);
            }
        }
        for root in self.fragments.roots(node) {
            self.visit(root);
        }
    }

    fn visit_identifier(&mut self, node: Node<'_>) {
        if !is_variable_read(node) {
            return;
        }
        if self.declared(self.text(node)) {
            self.lvars.insert(node.id());
        }
    }

    fn visit_parameters(&mut self, node: Node<'_>) {
        for child in named_children(node) {
            self.declare_parameter(child);
        }
    }

    fn declare_parameter(&mut self, node: Node<'_>) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node).to_owned();
                self.declare(&name);
            }
            "destructured_parameter" => {
                for child in named_children(node) {
                    self.declare_parameter(child);
                }
            }
            "optional_parameter"
            | "keyword_parameter"
            | "splat_parameter"
            | "hash_splat_parameter"
            | "block_parameter" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let name = self.text(name).to_owned();
                    self.declare(&name);
                }
                if let Some(value) = node.child_by_field_name("value") {
                    self.visit_default(value);
                }
            }
            _ => {}
        }
    }

    /// A parameter's default value, and the parameters the grammar folded into it. tree-sitter
    /// reads `def m(a = nil, b = nil)` as one parameter whose default swallowed `b`, so the names
    /// it swallowed have to be declared as parameters of their own.
    fn visit_default(&mut self, value: Node<'_>) {
        let Some(list) = folded_parameter_list(value) else {
            self.visit(value);
            return;
        };
        let items = named_children(list);
        let Some((first, swallowed)) = items.split_first() else {
            self.visit(value);
            return;
        };
        self.visit(*first);
        for parameter in swallowed {
            self.declare_parameter(*parameter);
        }
        if let Some(right) = value.child_by_field_name("right") {
            self.visit_default(right);
        }
    }

    fn visit_exception_variable(&mut self, node: Node<'_>) {
        let Some(target) = node.named_child(0) else {
            return;
        };
        if target.kind() == "identifier" {
            let name = self.text(target).to_owned();
            self.declare(&name);
        } else {
            self.visit(target);
        }
    }

    /// `/(?<name>…)/ =~ text` declares one local per named capture. Only a regexp the parser can
    /// compile does so; one holding an interpolation stays an ordinary `=~` call.
    fn visit_binary(&mut self, node: Node<'_>) {
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            self.visit_children(node);
            return;
        };
        if operator(node) != Some("=~")
            || left.kind() != "regex"
            || named_children(left)
                .iter()
                .any(|part| part.kind() == "interpolation")
        {
            self.visit_children(node);
            return;
        }
        for name in named_captures(self.text(left)) {
            self.declare(&name);
        }
        self.visit(left);
        self.visit(right);
    }

    /// The names a pattern binds. They only ever make a later read resolve to a variable.
    fn declare_pattern(&mut self, node: Node<'_>) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node).to_owned();
                self.declare(&name);
            }
            "array_pattern"
            | "find_pattern"
            | "hash_pattern"
            | "alternative_pattern"
            | "as_pattern"
            | "parenthesized_pattern"
            | "splat_parameter" => {
                for child in named_children(node) {
                    self.declare_pattern(child);
                }
            }
            "keyword_pattern" => match node.child_by_field_name("value") {
                Some(value) => self.declare_pattern(value),
                None => {
                    if let Some(key) = node.child_by_field_name("key") {
                        let name = self.text(key).trim_end_matches(':').to_owned();
                        self.declare(&name);
                    }
                }
            },
            // `in ^name` reads a local rather than binding one.
            "variable_reference_pattern" => self.visit_children(node),
            _ => self.visit(node),
        }
    }
}

/// Whether a `block` node is only the braces of a lambda literal.
fn inline_block(node: Node<'_>) -> bool {
    node.kind() == "block"
        && node
            .parent()
            .is_some_and(|parent| parent.kind() == "lambda")
}

/// Whether the grammar split a `=~` into the `=` of an assignment and a unary `~` opening its
/// value. Ruby's own lexer never separates the two, so an `=` glued to a `~` was always one token.
pub(super) fn split_match_operator(source: &SourceFile, node: Node<'_>, right: Node<'_>) -> bool {
    if !source.node_text(right).starts_with('~') {
        return false;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named() && child.kind() == "=")
        .is_some_and(|equals| equals.end_byte() == right.start_byte())
}

/// The three keywords that wear the shape of a name here but are literals upstream: `__FILE__` is
/// a `str`, `__LINE__` an `int` and `__ENCODING__` a `const`. None of them is a call.
pub(super) fn is_keyword_literal(name: &str) -> bool {
    matches!(name, "__FILE__" | "__LINE__" | "__ENCODING__")
}

/// Node kinds that hold a comma-separated list of expressions. tree-sitter reads `foo(a, b = 1)`
/// as a multiple assignment that swallowed `a`, which Ruby does not: only `b` is assigned.
const COMMA_SEPARATED_LISTS: &[&str] = &[
    "argument_list",
    "array",
    "splat_argument",
    "optional_parameter",
    "keyword_parameter",
    "right_assignment_list",
];

/// Whether an assignment's target list is really the comma-separated list written around it.
pub(super) fn spurious_assignment_list(list: Node<'_>) -> bool {
    // A swallowed list runs on into the value, so `foo(a = 1, b = 2, c = 3)` nests one invented
    // assignment inside the next and only the outermost one stands in the list itself.
    let mut current = list.parent();
    while let Some(node) = current {
        let Some(parent) = node.parent() else {
            return false;
        };
        if COMMA_SEPARATED_LISTS.contains(&parent.kind()) {
            return true;
        }
        let continues = parent.kind() == "assignment"
            && parent
                .child_by_field_name("right")
                .is_some_and(|right| right.id() == node.id());
        current = continues.then_some(parent);
    }
    false
}

pub(super) fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

pub(super) fn field_name(child: Node<'_>, parent: Node<'_>) -> Option<&'static str> {
    let mut cursor = parent.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        if cursor.node().id() == child.id() {
            return cursor.field_name();
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

/// The operator token of a node whose operands tree-sitter names but whose operator it does not.
pub(super) fn operator(node: Node<'_>) -> Option<&'static str> {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        let child = cursor.node();
        if !child.is_named() {
            return Some(child.kind());
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

/// The swallowed parameter list of a folded default value, when there is one.
pub(super) fn folded_parameter_list<'tree>(value: Node<'tree>) -> Option<Node<'tree>> {
    value
        .child_by_field_name("left")
        .filter(|_| value.kind() == "assignment")
        .filter(|left| left.kind() == "left_assignment_list")
}

/// Whether an identifier stands where the parser upstream would have built an `lvar`, rather than
/// a name being declared, written or called.
fn is_variable_read(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    match parent.kind() {
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

/// The names a regexp literal captures, in the order they are written.
pub(super) fn named_captures(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut names = Vec::new();
    let mut index = 0;
    while index + 3 < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == b'('
            && bytes[index + 1] == b'?'
            && matches!(bytes[index + 2], b'<' | b'\'')
        {
            let close = if bytes[index + 2] == b'<' {
                b'>'
            } else {
                b'\''
            };
            // `(?<=` and `(?<!` are look-behind, not a capture.
            if close == b'>' && matches!(bytes[index + 3], b'=' | b'!') {
                index += 4;
                continue;
            }
            if let Some(end) = source[index + 3..].find(close as char) {
                names.push(source[index + 3..index + 3 + end].to_owned());
                index += 3 + end + 1;
                continue;
            }
        }
        index += 1;
    }
    names
}
