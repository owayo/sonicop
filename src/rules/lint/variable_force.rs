//! A port of RuboCop's `VariableForce`: where every local variable is declared, which of its
//! assignments are read afterwards, and which branch each of those sits in.
//!
//! Two cops share it -- `Lint/UselessAssignment` reports assignments nothing reads, and
//! `Lint/UnusedBlockArgument` reports parameters nothing reads -- and both depend on details that
//! no shortcut reproduces: a name shadowed by an inner block, a variable a `binding` call hands to
//! the caller, an assignment in one arm of an `if` that another arm's read cannot excuse.
//!
//! The traversal follows the upstream force node for node, including the places where it walks
//! children out of order (a post-condition loop scans its body before its condition), because the
//! order is what decides whether a read comes before or after an assignment. Two tree-sitter
//! shapes need bridging: a heredoc's body hangs off the statement rather than the expression that
//! opened it, and `->(x) { }` keeps its parameters one node above its body.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::source::SourceFile;

/// How a variable came into being, which decides what may be reported about it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Declaration {
    /// `arg`, `optarg`, `restarg`, `kwarg`, `kwoptarg`, `kwrestarg` or `blockarg`.
    Argument(Argument),
    /// `shadowarg`: the block local variable of `each { |item; buffer| }`.
    BlockLocal,
    /// An `lvasgn` and the two node types that stand in for one.
    Variable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Argument {
    Positional,
    Optional,
    Rest,
    Keyword,
    Block,
}

/// What an assignment writes, which decides the range it is reported at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AssignmentKind {
    /// An ordinary `lvasgn`, whatever syntax produced it.
    Plain,
    /// `match_with_lvasgn`: the locals `/(?<year>\d+)/ =~ text` creates.
    RegexpNamedCapture,
}

pub(super) struct Assignment<'tree> {
    /// The name being written, or the regexp of a named capture: what `loc.name` covers.
    pub name: Node<'tree>,
    /// The expression the write belongs to, which decides how it can be corrected.
    pub node: Node<'tree>,
    /// What the write stores, when there is such a node. An `lvasgn` upstream carries its value,
    /// but the one standing for a `masgn` target, a `for` variable or a rescue clause does not --
    /// and neither does one the grammar built by swallowing its neighbours, whose own node spans
    /// far more than the write it stands for.
    pub value: Option<Node<'tree>>,
    pub kind: AssignmentKind,
    pub referenced: bool,
    reassigned: bool,
    branch: Option<usize>,
}

impl Assignment<'_> {
    /// A variable a block captured may be read at any later time, so an assignment that is not
    /// overwritten before then still counts as used.
    fn used(&self, captured_by_block: bool) -> bool {
        (!self.reassigned && captured_by_block) || self.referenced
    }
}

pub(super) struct Variable<'tree> {
    pub name: String,
    /// The node that declared it: the parameter, or the first assignment to it.
    pub declaration: Node<'tree>,
    /// The part of the declaration that `loc.name` covers.
    pub name_node: Node<'tree>,
    pub kind: Declaration,
    /// Index into [`Analysis::scopes`], filled in when the scope is left.
    pub scope: usize,
    /// The scope the variable belongs to, known while it is still being walked.
    scope_node: Node<'tree>,
    pub assignments: Vec<Assignment<'tree>>,
    pub referenced: bool,
    pub captured_by_block: bool,
}

impl Variable<'_> {
    /// `should_be_unused?`: a name the author already marked as deliberately unused.
    pub(super) fn should_be_unused(&self) -> bool {
        self.name.starts_with('_')
    }

    pub(super) fn is_argument(&self) -> bool {
        matches!(
            self.kind,
            Declaration::Argument(_) | Declaration::BlockLocal
        )
    }

    pub(super) fn assignment_used(&self, index: usize) -> bool {
        self.assignments[index].used(self.captured_by_block)
    }
}

pub(super) struct Scope<'tree> {
    /// The `def`, `class`, `block` or root node the scope belongs to.
    pub node: Node<'tree>,
    /// Whether this is the file's top level, which is not a scope node of any kind.
    pub top_level: bool,
    /// Indices into [`Analysis::variables`], in declaration order.
    pub variables: Vec<usize>,
}

pub(super) struct Analysis<'tree> {
    /// Every scope, in the order they were left, which is the order the cops report in.
    pub scopes: Vec<Scope<'tree>>,
    pub variables: Vec<Variable<'tree>>,
}

impl<'tree> Analysis<'tree> {
    pub(super) fn run(root: Node<'tree>, source: &SourceFile) -> Self {
        let mut force = Force {
            source,
            scopes: Vec::new(),
            variables: Vec::new(),
            stack: Vec::new(),
            branches: Vec::new(),
            branch_index: HashMap::new(),
            heredocs: heredoc_bodies(root),
            scanned: HashSet::new(),
        };
        force.push_scope(root, true);
        force.process_children(root);
        force.pop_scope();
        Analysis {
            scopes: force.scopes,
            variables: force.variables,
        }
    }
}

/// One branch of one control structure, as `VariableForce::Branch` models it.
struct Branch {
    control: usize,
    child: usize,
    parent: Option<usize>,
    /// `may_run_incompletely?`: the body of a `begin` that a raise can leave early.
    incomplete: bool,
    /// `may_jump_to_other_branch?`: the same body, which a raise moves to a rescue clause.
    jumps: bool,
}

struct Frame<'tree> {
    node: Node<'tree>,
    top_level: bool,
    /// Whether the scope can see the variables around it, as only a block can.
    block: bool,
    names: HashMap<String, usize>,
    order: Vec<usize>,
}

struct Force<'tree, 'a> {
    source: &'a SourceFile,
    scopes: Vec<Scope<'tree>>,
    variables: Vec<Variable<'tree>>,
    stack: Vec<Frame<'tree>>,
    branches: Vec<Branch>,
    branch_index: HashMap<(usize, usize), usize>,
    /// Each heredoc's body, found from the `<<~X` that opened it. tree-sitter hangs the body off
    /// the enclosing statement, so a heredoc written inside a block would otherwise have its
    /// interpolations resolved in the wrong scope.
    heredocs: HashMap<usize, Node<'tree>>,
    /// Nodes already walked in an outer scope, which the scope they sit in must not walk again.
    scanned: HashSet<usize>,
}

// ---------------------------------------------------------------------------
// Node classification
// ---------------------------------------------------------------------------

/// The scope a node opens, and the fields that still belong to the scope around it. RuboCop calls
/// these "twisted" nodes: `class Foo < bar` evaluates `bar` outside the class body it precedes.
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

/// Node kinds that hold a comma-separated list of expressions. tree-sitter parses `foo(a, b = 1)`
/// as a multiple assignment that swallowed `a`, which Ruby does not: only `b` is assigned.
const COMMA_SEPARATED_LISTS: &[&str] = &[
    "argument_list",
    "array",
    "splat_argument",
    "optional_parameter",
    "keyword_parameter",
    "right_assignment_list",
];

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

/// What an assignment really stores. When the grammar swallowed the neighbouring items of a
/// comma-separated list, the node it made the right-hand side spans all of them, and only its
/// first element belongs to this write.
pub(super) fn assigned_value<'tree>(right: Node<'tree>) -> Node<'tree> {
    let Some(list) = right
        .child_by_field_name("left")
        .filter(|_| right.kind() == "assignment")
        .filter(|left| left.kind() == "left_assignment_list")
        .filter(|left| spurious_assignment_list(*left))
    else {
        return right;
    };
    list.named_child(0).unwrap_or(right)
}

pub(super) fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// `Scope#each_node`: every node the scope owns. The body of a nested `def` or block belongs to
/// that scope instead, but the parts written beside it -- a superclass, a block's own receiver --
/// are still evaluated here.
pub(super) fn scope_nodes<'tree>(scope: &Scope<'tree>) -> Vec<Node<'tree>> {
    let mut nodes = Vec::new();
    if scope.top_level {
        nodes.push(scope.node);
    }
    scan_scope(scope.node, scope.node, &mut nodes);
    nodes
}

fn scan_scope<'tree>(node: Node<'tree>, scope_node: Node<'tree>, nodes: &mut Vec<Node<'tree>>) {
    for child in named_children(node) {
        if !owned_by_scope(child, node, scope_node) {
            continue;
        }
        nodes.push(child);
        scan_scope(child, scope_node, nodes);
    }
}

fn owned_by_scope(child: Node<'_>, parent: Node<'_>, scope_node: Node<'_>) -> bool {
    let Some((_, outer_fields)) = scope_kind(parent.kind()) else {
        return true;
    };
    let outer = outer_fields.iter().any(|field| {
        parent
            .child_by_field_name(field)
            .is_some_and(|f| f.id() == child.id())
    });
    if parent.id() == scope_node.id() {
        // The scope's own outer parts were evaluated before it was entered.
        !outer
    } else {
        outer
    }
}

/// The statements a scope's body holds, which is `nil` upstream when the body is empty. A block
/// written `{ |x| ; }` has a body node here but no statement in it.
pub(super) fn body_node<'tree>(scope: &Scope<'tree>) -> Option<Node<'tree>> {
    if scope.top_level {
        return Some(scope.node);
    }
    let body = match scope.node.kind() {
        // A lambda literal keeps its statements one level down, inside the braces node.
        "lambda" => scope
            .node
            .child_by_field_name("body")?
            .child_by_field_name("body"),
        _ => scope.node.child_by_field_name("body"),
    }?;
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .any(|child| child.kind() != "empty_statement")
        .then_some(body)
}

/// The call a block was written on, which names the method the block was passed to.
pub(super) fn block_call<'tree>(scope_node: Node<'tree>) -> Option<Node<'tree>> {
    match scope_node.kind() {
        "block" | "do_block" => scope_node.parent().filter(|node| node.kind() == "call"),
        _ => None,
    }
}

/// `BlockNode#lambda?`: both `->() {}` and `lambda {}` reach RuboCop as a block on `lambda`.
pub(super) fn is_lambda(scope_node: Node<'_>, source: &SourceFile) -> bool {
    scope_node.kind() == "lambda" || block_method(scope_node, source) == Some("lambda")
}

pub(super) fn block_method<'a>(scope_node: Node<'_>, source: &'a SourceFile) -> Option<&'a str> {
    let call = block_call(scope_node)?;
    Some(source.node_text(call.child_by_field_name("method")?))
}

// ---------------------------------------------------------------------------
// Traversal
// ---------------------------------------------------------------------------

impl<'tree> Force<'tree, '_> {
    fn text(&self, node: Node<'_>) -> &str {
        self.source.node_text(node)
    }

    fn push_scope(&mut self, node: Node<'tree>, top_level: bool) {
        let block = matches!(node.kind(), "block" | "do_block" | "lambda");
        self.stack.push(Frame {
            node,
            top_level,
            block,
            names: HashMap::new(),
            order: Vec::new(),
        });
    }

    fn pop_scope(&mut self) {
        let frame = self.stack.pop().expect("scope stack is never empty");
        let index = self.scopes.len();
        for &variable in &frame.order {
            self.variables[variable].scope = index;
        }
        self.scopes.push(Scope {
            node: frame.node,
            top_level: frame.top_level,
            variables: frame.order,
        });
    }

    fn process_children(&mut self, node: Node<'tree>) {
        for child in named_children(node) {
            if !self.scanned.contains(&child.id()) {
                self.process_node(child);
            }
        }
    }

    fn process_node(&mut self, node: Node<'tree>) {
        if scope_kind(node.kind()).is_some() && !self.inline_block(node) {
            self.process_scope(node);
            return;
        }
        match node.kind() {
            "assignment" => self.process_assignment(node),
            "operator_assignment" => self.process_operator_assignment(node),
            "identifier" => self.process_identifier(node),
            "call" => self.process_call(node),
            "super" => self.process_zero_arity_super(node),
            "while" | "until" | "while_modifier" | "until_modifier" | "for" => {
                self.process_loop(node);
            }
            // `foo = 1 if bar` runs its condition first, but tree-sitter writes the body first.
            "if_modifier" | "unless_modifier" => {
                if let Some(condition) = node.child_by_field_name("condition") {
                    self.process_node(condition);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.process_node(body);
                }
            }
            "binary" => self.process_binary(node),
            "exception_variable" => self.process_exception_variable(node),
            "method_parameters" | "block_parameters" | "lambda_parameters" => {
                self.process_parameters(node);
            }
            "pair" => self.process_pair(node),
            "heredoc_beginning" => self.process_heredoc(node),
            // Walked at the `<<~X` that opened it, in whichever scope that was written in.
            "heredoc_body" => {}
            "array_pattern"
            | "find_pattern"
            | "hash_pattern"
            | "alternative_pattern"
            | "as_pattern"
            | "in_clause"
            | "match_pattern" => self.process_pattern(node),
            _ => {
                if self.retry_loop(node) {
                    self.process_loop_body(node);
                } else {
                    self.process_children(node);
                }
            }
        }
    }

    /// Whether a `block` node is only the braces of a lambda literal. Upstream `->(x) { }` is one
    /// block node holding both the parameters and the body, so the inner node is not a scope.
    fn inline_block(&self, node: Node<'tree>) -> bool {
        node.kind() == "block"
            && node
                .parent()
                .is_some_and(|parent| parent.kind() == "lambda")
    }

    fn process_scope(&mut self, node: Node<'tree>) {
        let (_, outer_fields) = scope_kind(node.kind()).expect("checked by the caller");
        for field in outer_fields {
            if let Some(child) = node.child_by_field_name(field) {
                self.process_node(child);
                self.scanned.insert(child.id());
            }
        }
        self.push_scope(node, false);
        self.process_children(node);
        self.pop_scope();
    }

    // -- variable table ----------------------------------------------------

    fn find_variable(&self, name: &str) -> Option<usize> {
        for frame in self.stack.iter().rev() {
            if let Some(&index) = frame.names.get(name) {
                return Some(index);
            }
            if !frame.block {
                return None;
            }
        }
        None
    }

    fn declare(
        &mut self,
        name: &str,
        declaration: Node<'tree>,
        name_node: Node<'tree>,
        kind: Declaration,
    ) {
        let index = self.variables.len();
        let frame = self.stack.last_mut().expect("scope stack is never empty");
        self.variables.push(Variable {
            name: name.to_owned(),
            declaration,
            name_node,
            kind,
            scope: 0,
            scope_node: frame.node,
            assignments: Vec::new(),
            referenced: false,
            captured_by_block: false,
        });
        if let Some(replaced) = frame.names.insert(name.to_owned(), index) {
            frame.order.retain(|&variable| variable != replaced);
        }
        frame.order.push(index);
    }

    fn declare_unless_known(&mut self, name: &str, node: Node<'tree>, name_node: Node<'tree>) {
        if self.find_variable(name).is_none() {
            self.declare(name, node, name_node, Declaration::Variable);
        }
    }

    /// Whether the scope now being walked is a block other than the one the variable belongs to,
    /// which is what makes the variable outlive the reads this walk can see.
    fn capture_if_needed(&mut self, variable: usize) {
        let Some(frame) = self.stack.last() else {
            return;
        };
        if frame.block && self.variables[variable].scope_node.id() != frame.node.id() {
            self.variables[variable].captured_by_block = true;
        }
    }

    fn assign(
        &mut self,
        variable: usize,
        name: Node<'tree>,
        node: Node<'tree>,
        value: Option<Node<'tree>>,
        kind: AssignmentKind,
    ) {
        self.capture_if_needed(variable);
        let branch = self.branch_of(node);
        let captured = self.variables[variable].captured_by_block;
        let previous = self.variables[variable]
            .assignments
            .last()
            .map(|last| last.branch);
        if !captured && previous == Some(branch) {
            if let Some(last) = self.variables[variable].assignments.last_mut() {
                if !last.referenced {
                    last.reassigned = true;
                }
            }
        }
        self.variables[variable].assignments.push(Assignment {
            name,
            node,
            value,
            kind,
            referenced: false,
            reassigned: false,
            branch,
        });
    }

    fn reference(&mut self, variable: usize, node: Node<'tree>) {
        self.capture_if_needed(variable);
        self.reference_without_capture(variable, node);
    }

    /// `Variable#reference!` on its own. `process_send` and `process_zero_arity_super` reach past
    /// the variable table and so never mark the variable as captured by a block, which is what
    /// keeps `binding = proc { binding }` reportable.
    fn reference_without_capture(&mut self, variable: usize, node: Node<'tree>) {
        self.variables[variable].referenced = true;
        let reference_branch = self.branch_of(node);
        let mut consumed: Vec<usize> = Vec::new();
        for index in (0..self.variables[variable].assignments.len()).rev() {
            let branch = self.variables[variable].assignments[index].branch;
            if branch.is_some_and(|branch| consumed.contains(&branch)) {
                continue;
            }
            if !self.exclusive(branch, reference_branch) {
                self.variables[variable].assignments[index].referenced = true;
            }
            let assignment_node = self.variables[variable].assignments[index].node;
            if in_modifier_conditional(assignment_node, node) {
                continue;
            }
            let Some(branch) = branch else { break };
            if Some(branch) == reference_branch {
                break;
            }
            if !self.branches[branch].incomplete {
                consumed.push(branch);
            }
        }
    }

    fn reference_by_name(&mut self, name: &str, node: Node<'tree>) {
        if let Some(variable) = self.find_variable(name) {
            self.reference(variable, node);
        }
    }

    /// Every variable the current point can still name, innermost scope first.
    fn accessible_variables(&self) -> Vec<usize> {
        let mut accessible = Vec::new();
        for frame in self.stack.iter().rev() {
            accessible.extend(frame.order.iter().copied());
            if !frame.block {
                break;
            }
        }
        accessible
    }

    // -- handlers ----------------------------------------------------------

    fn process_assignment(&mut self, node: Node<'tree>) {
        let Some(left) = node.child_by_field_name("left") else {
            self.process_children(node);
            return;
        };
        let right = node.child_by_field_name("right");
        match left.kind() {
            "identifier" => {
                let name = self.text(left).to_owned();
                self.declare_unless_known(&name, node, left);
                if let Some(right) = right {
                    self.process_node(right);
                }
                if let Some(variable) = self.find_variable(&name) {
                    self.assign(
                        variable,
                        left,
                        node,
                        right.map(assigned_value),
                        AssignmentKind::Plain,
                    );
                }
            }
            "left_assignment_list" if spurious_assignment_list(left) => {
                // Everything the grammar swallowed before the real target is an ordinary
                // expression standing next to it in a comma-separated list.
                let targets = named_children(left);
                let Some((&last, leading)) = targets.split_last() else {
                    return;
                };
                for target in leading {
                    self.process_node(*target);
                }
                if let Some(right) = right {
                    self.process_node(right);
                }
                if last.kind() == "identifier" {
                    let name = self.text(last).to_owned();
                    self.declare_unless_known(&name, last, last);
                    if let Some(variable) = self.find_variable(&name) {
                        self.assign(
                            variable,
                            last,
                            last,
                            right.map(assigned_value),
                            AssignmentKind::Plain,
                        );
                    }
                } else {
                    self.process_multiple_assignment_target(last);
                }
            }
            "left_assignment_list" => {
                // `masgn` evaluates its right-hand side first.
                if let Some(right) = right {
                    self.process_node(right);
                }
                self.process_multiple_assignment_target(left);
            }
            _ => self.process_children(node),
        }
    }

    fn process_multiple_assignment_target(&mut self, node: Node<'tree>) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node).to_owned();
                self.declare_unless_known(&name, node, node);
                if let Some(variable) = self.find_variable(&name) {
                    self.assign(variable, node, node, None, AssignmentKind::Plain);
                }
            }
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                for child in named_children(node) {
                    self.process_multiple_assignment_target(child);
                }
            }
            _ => self.process_node(node),
        }
    }

    fn process_operator_assignment(&mut self, node: Node<'tree>) {
        let (Some(left), right) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            self.process_children(node);
            return;
        };
        if left.kind() != "identifier" {
            self.process_children(node);
            return;
        }
        // `foo += foo = 2` reads `foo` before the right-hand side runs, so the reference has to be
        // recorded first.
        let name = self.text(left).to_owned();
        self.declare_unless_known(&name, left, left);
        self.reference_by_name(&name, node);
        if let Some(right) = right {
            self.process_node(right);
        }
        if let Some(variable) = self.find_variable(&name) {
            self.assign(variable, left, left, None, AssignmentKind::Plain);
        }
    }

    fn process_identifier(&mut self, node: Node<'tree>) {
        // Only tree-sitter spells a method name with the same node type as a variable read; the
        // parser upstream turns `def foo` and `alias foo bar` into symbols. The receiver of
        // `def obj.foo` is a genuine read and stays.
        if node.parent().is_some_and(|parent| {
            matches!(parent.kind(), "alias" | "undef" | "setter")
                || (matches!(parent.kind(), "method" | "singleton_method")
                    && field_name(node, parent) == Some("name"))
        }) {
            return;
        }
        let name = self.text(node);
        if let Some(variable) = self.find_variable(name) {
            self.reference(variable, node);
        } else if name == "binding" {
            self.reference_everything(node);
        }
    }

    fn process_call(&mut self, node: Node<'tree>) {
        if self.binary_operator_on_a_local(node) {
            return;
        }
        if let Some(method) = node.child_by_field_name("method")
            && self.text(method) == "binding"
            && opaque_binding_argument(node)
        {
            self.reference_everything(node);
        }
        for child in named_children(node) {
            if node
                .child_by_field_name("method")
                .is_some_and(|m| m.id() == child.id())
            {
                continue;
            }
            if !self.scanned.contains(&child.id()) {
                self.process_node(child);
            }
        }
    }

    /// Whether the call is really `local & expr` or `local * expr`. Ruby resolves the ambiguity by
    /// what it has already seen: once the name is a local variable, the `&` is an operator and not
    /// the start of a block-pass argument. tree-sitter has no scope to consult and always reads the
    /// argument form, so the two operands would otherwise look like a call to a method of that
    /// name with nothing reading the variable.
    fn binary_operator_on_a_local(&mut self, node: Node<'tree>) -> bool {
        if node.child_by_field_name("receiver").is_some() {
            return false;
        }
        let (Some(method), Some(arguments)) = (
            node.child_by_field_name("method"),
            node.child_by_field_name("arguments"),
        ) else {
            return false;
        };
        if method.kind() != "identifier"
            || self.text(arguments).starts_with('(')
            || arguments.named_child_count() != 1
        {
            return false;
        }
        let Some(argument) = arguments
            .named_child(0)
            .filter(|child| matches!(child.kind(), "block_argument" | "splat_argument"))
        else {
            return false;
        };
        let name = self.text(method).to_owned();
        let Some(variable) = self.find_variable(&name) else {
            return false;
        };
        self.reference(variable, method);
        self.process_children(argument);
        true
    }

    /// A `binding` call hands the whole scope to its caller, so nothing in reach can be called
    /// unused any more.
    fn reference_everything(&mut self, node: Node<'tree>) {
        for variable in self.accessible_variables() {
            self.reference(variable, node);
        }
    }

    /// Bare `super` passes the method's own arguments on, which reads every one of them.
    fn process_zero_arity_super(&mut self, node: Node<'tree>) {
        for variable in self.accessible_variables() {
            let method_argument = self.variables[variable].is_argument()
                && matches!(
                    self.variables[variable].scope_node.kind(),
                    "method" | "singleton_method"
                );
            if method_argument {
                self.reference(variable, node);
            }
        }
    }

    fn process_parameters(&mut self, node: Node<'tree>) {
        let mut cursor = node.walk();
        if !cursor.goto_first_child() {
            return;
        }
        loop {
            let child = cursor.node();
            if child.is_named() {
                let local = cursor.field_name() == Some("locals");
                self.declare_parameter(child, local);
            }
            if !cursor.goto_next_sibling() {
                return;
            }
        }
    }

    fn declare_parameter(&mut self, node: Node<'tree>, local: bool) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node).to_owned();
                let kind = if local {
                    Declaration::BlockLocal
                } else {
                    Declaration::Argument(Argument::Positional)
                };
                self.declare(&name, node, node, kind);
            }
            // `|(a, b)|` declares each element on its own.
            "destructured_parameter" => {
                for child in named_children(node) {
                    self.declare_parameter(child, local);
                }
            }
            _ => {
                let argument = match node.kind() {
                    "optional_parameter" => Argument::Optional,
                    "keyword_parameter" => Argument::Keyword,
                    "splat_parameter" | "hash_splat_parameter" => Argument::Rest,
                    "block_parameter" => Argument::Block,
                    _ => return,
                };
                // `def m(*)` and `def m(**)` name nothing, so they declare nothing.
                let Some(name_node) = node.child_by_field_name("name") else {
                    if let Some(value) = node.child_by_field_name("value") {
                        self.process_node(value);
                    }
                    return;
                };
                let name = self.text(name_node).to_owned();
                self.declare(&name, node, name_node, Declaration::Argument(argument));
                if let Some(value) = node.child_by_field_name("value") {
                    self.process_default(value, argument);
                }
            }
        }
    }

    /// A parameter's default value, and the parameters the grammar folded into it. tree-sitter
    /// reads `def m(a = nil, b = nil)` as one parameter whose default is a multiple assignment
    /// that swallowed `b`, so the names it swallowed have to be declared as parameters too.
    fn process_default(&mut self, value: Node<'tree>, argument: Argument) {
        let Some(left) = value
            .child_by_field_name("left")
            .filter(|left| value.kind() == "assignment" && left.kind() == "left_assignment_list")
            .filter(|left| spurious_assignment_list(*left))
        else {
            self.process_node(value);
            return;
        };
        let items = named_children(left);
        let Some((first, rest)) = items.split_first() else {
            self.process_node(value);
            return;
        };
        self.process_node(*first);
        let Some((last, positional)) = rest.split_last() else {
            return;
        };
        for parameter in positional {
            self.declare_parameter(*parameter, false);
        }
        if last.kind() == "identifier" {
            let name = self.text(*last).to_owned();
            self.declare(&name, *last, *last, Declaration::Argument(argument));
        }
        if let Some(right) = value.child_by_field_name("right") {
            self.process_default(right, argument);
        }
    }

    /// `{ name: }` is Ruby's shorthand for `{ name: name }`, so the key reads the variable.
    fn process_pair(&mut self, node: Node<'tree>) {
        let Some(key) = node.child_by_field_name("key") else {
            self.process_children(node);
            return;
        };
        if node.child_by_field_name("value").is_some() {
            self.process_children(node);
            return;
        }
        let name = self.text(key).trim_end_matches(':').to_owned();
        self.reference_by_name(&name, node);
    }

    fn process_heredoc(&mut self, node: Node<'tree>) {
        let Some(&body) = self.heredocs.get(&node.id()) else {
            return;
        };
        self.scanned.insert(body.id());
        for child in named_children(body) {
            // `#` never opens a comment inside a heredoc, but the grammar lexes one anyway when a
            // literal `#` precedes an interpolation, swallowing the rest of the line. The reads
            // written in it are real, so the names they mention are recovered from the text.
            if child.kind() == "comment" {
                self.reference_names_in_interpolations(child);
            } else {
                self.process_node(child);
            }
        }
    }

    fn reference_names_in_interpolations(&mut self, node: Node<'tree>) {
        for name in interpolated_names(self.text(node)) {
            self.reference_by_name(&name, node);
        }
    }

    fn process_exception_variable(&mut self, node: Node<'tree>) {
        let Some(target) = node.named_child(0) else {
            return;
        };
        if target.kind() != "identifier" {
            self.process_node(target);
            return;
        }
        let name = self.text(target).to_owned();
        self.declare_unless_known(&name, target, target);
        if let Some(variable) = self.find_variable(&name) {
            self.assign(variable, target, target, None, AssignmentKind::Plain);
        }
    }

    /// `/(?<name>…)/ =~ text` declares one local per named capture. Any other binary operator is
    /// just an expression.
    fn process_binary(&mut self, node: Node<'tree>) {
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            self.process_children(node);
            return;
        };
        // Only a regexp the parser can compile becomes a `match_with_lvasgn`; one holding an
        // interpolation stays an ordinary `=~` call and creates no local at all.
        if operator(node) != Some("=~")
            || left.kind() != "regex"
            || named_children(left)
                .iter()
                .any(|part| part.kind() == "interpolation")
        {
            self.process_children(node);
            return;
        }
        let names = named_captures(self.text(left));
        if names.is_empty() {
            self.process_children(node);
            return;
        }
        for name in &names {
            self.declare_unless_known(name, node, left);
        }
        self.process_node(right);
        self.process_node(left);
        for name in &names {
            if let Some(variable) = self.find_variable(name) {
                self.assign(
                    variable,
                    left,
                    node,
                    None,
                    AssignmentKind::RegexpNamedCapture,
                );
            }
        }
    }

    fn process_pattern(&mut self, node: Node<'tree>) {
        match node.kind() {
            "in_clause" | "match_pattern" => {
                if let Some(value) = node.child_by_field_name("value") {
                    self.process_node(value);
                }
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.declare_pattern(pattern);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.process_node(body);
                }
                if let Some(guard) = node.child_by_field_name("guard") {
                    self.process_node(guard);
                }
            }
            _ => self.declare_pattern(node),
        }
    }

    /// The names a pattern binds. Upstream calls them `match_var`, declares them and never
    /// assigns to them, so they only ever make a later read resolve to a local.
    fn declare_pattern(&mut self, node: Node<'tree>) {
        match node.kind() {
            "identifier" => {
                let name = self.text(node).to_owned();
                if self.find_variable(&name).is_none() {
                    self.declare(&name, node, node, Declaration::Variable);
                }
            }
            "array_pattern"
            | "find_pattern"
            | "hash_pattern"
            | "alternative_pattern"
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
                        if self.find_variable(&name).is_none() {
                            self.declare(&name, key, key, Declaration::Variable);
                        }
                    }
                }
            },
            "as_pattern" => {
                for child in named_children(node) {
                    self.declare_pattern(child);
                }
            }
            // `in ^name` reads a local rather than binding one.
            "variable_reference_pattern" => self.process_children(node),
            _ => self.process_node(node),
        }
    }

    // -- loops -------------------------------------------------------------

    fn process_loop(&mut self, node: Node<'tree>) {
        match node.kind() {
            "for" => {
                // `for item in items` evaluates the collection first.
                if let Some(value) = node.child_by_field_name("value") {
                    self.process_node(value);
                }
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.process_multiple_assignment_target(pattern);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.process_node(body);
                }
            }
            _ if post_condition_loop(node) => {
                // `begin … end while cond` runs its body before the condition is ever read.
                if let Some(body) = node.child_by_field_name("body") {
                    self.process_node(body);
                }
                if let Some(condition) = node.child_by_field_name("condition") {
                    self.process_node(condition);
                }
            }
            _ => {
                if let Some(condition) = node.child_by_field_name("condition") {
                    self.process_node(condition);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.process_node(body);
                }
            }
        }
        self.mark_assignments_referenced_in_loop(node);
    }

    /// A `begin`/`rescue` that can `retry` runs its body more than once, so it is a loop too.
    fn retry_loop(&self, node: Node<'tree>) -> bool {
        named_children(node)
            .iter()
            .filter(|child| child.kind() == "rescue")
            .any(|child| contains_kind(*child, "retry"))
    }

    fn process_loop_body(&mut self, node: Node<'tree>) {
        self.process_children(node);
        self.mark_assignments_referenced_in_loop(node);
    }

    /// An assignment a later iteration reads is not useless, even though the read stands before it
    /// in the source.
    fn mark_assignments_referenced_in_loop(&mut self, node: Node<'tree>) {
        let mut names = Vec::new();
        let mut assignments = HashSet::new();
        collect_loop_references(node, self.source, &mut names, &mut assignments);
        for name in names {
            let Some(variable) = self.find_variable(&name) else {
                continue;
            };
            let indices: Vec<usize> = (0..self.variables[variable].assignments.len())
                .filter(|&index| {
                    assignments.contains(&self.variables[variable].assignments[index].node.id())
                })
                .collect();
            let Some(&last) = indices.last() else {
                continue;
            };
            for &index in &indices {
                let assignment = &self.variables[variable].assignments[index];
                if has_branch_ancestor(assignment.node) {
                    self.variables[variable].assignments[index].referenced = true;
                }
            }
            self.variables[variable].assignments[last].referenced = true;
            self.variables[variable].referenced = true;
        }
    }

    // -- branches ----------------------------------------------------------

    /// `Branch.of`: the innermost conditional arm the node sits in, within its own scope.
    fn branch_of(&mut self, node: Node<'tree>) -> Option<usize> {
        let scope_node = self.stack.last()?.node;
        let top_level = self.stack.last()?.top_level;
        self.branch_within(node, scope_node, top_level)
    }

    fn branch_within(
        &mut self,
        node: Node<'tree>,
        scope_node: Node<'tree>,
        top_level: bool,
    ) -> Option<usize> {
        let mut current = node;
        loop {
            if !top_level && current.id() == scope_node.id() {
                return None;
            }
            let parent = current.parent()?;
            if let Some((incomplete, jumps, branched)) = branch_role(current, parent)
                && branched
            {
                return Some(
                    self.intern_branch(current, parent, scope_node, top_level, incomplete, jumps),
                );
            }
            current = parent;
        }
    }

    fn intern_branch(
        &mut self,
        child: Node<'tree>,
        control: Node<'tree>,
        scope_node: Node<'tree>,
        top_level: bool,
        incomplete: bool,
        jumps: bool,
    ) -> usize {
        let key = (control.id(), child.id());
        if let Some(&index) = self.branch_index.get(&key) {
            return index;
        }
        let index = self.branches.len();
        self.branches.push(Branch {
            control: control.id(),
            child: child.id(),
            parent: None,
            incomplete,
            jumps,
        });
        self.branch_index.insert(key, index);
        let parent = self.branch_within(control, scope_node, top_level);
        self.branches[index].parent = parent;
        index
    }

    /// `Branchable#run_exclusively_with?`: whether two branches of the same structure can never
    /// both run, which is what keeps a read in one arm from excusing an assignment in another.
    fn exclusive(&self, assignment: Option<usize>, reference: Option<usize>) -> bool {
        let (Some(assignment), Some(reference)) = (assignment, reference) else {
            return false;
        };
        self.exclusive_branches(assignment, reference)
    }

    fn exclusive_branches(&self, branch: usize, other: usize) -> bool {
        if self.branches[branch].jumps {
            return false;
        }
        let mut candidate = Some(other);
        while let Some(index) = candidate {
            if self.branches[index].control == self.branches[branch].control {
                return self.branches[index].child != self.branches[branch].child;
            }
            candidate = self.branches[index].parent;
        }
        match self.branches[branch].parent {
            Some(parent) => self.exclusive_branches(parent, other),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Node helpers
// ---------------------------------------------------------------------------

/// Whether the child stands in a branch of the control structure above it, and how that branch
/// behaves: `(may_run_incompletely, may_jump_to_other_branch, branched)`.
fn branch_role(child: Node<'_>, parent: Node<'_>) -> Option<(bool, bool, bool)> {
    let field = field_name(child, parent);
    match parent.kind() {
        "if" | "elsif" | "unless" | "conditional" => Some(match field {
            Some("condition") => (false, false, false),
            _ => (false, false, true),
        }),
        "if_modifier" | "unless_modifier" => Some(match field {
            Some("condition") => (false, false, false),
            _ => (false, false, true),
        }),
        "while" | "until" | "while_modifier" | "until_modifier" => Some(match field {
            Some("condition") => (false, false, false),
            _ => (false, false, true),
        }),
        "for" => Some(match field {
            Some("body") => (false, false, true),
            _ => (false, false, false),
        }),
        "case" | "case_match" => Some(match field {
            Some("value") => (false, false, false),
            _ => (false, false, true),
        }),
        "binary" => match operator(parent) {
            Some("&&" | "||" | "and" | "or") => Some((false, false, field != Some("left"))),
            _ => None,
        },
        "operator_assignment" => Some((false, false, field != Some("left"))),
        "rescue_modifier" => Some(match field {
            Some("body") => (true, true, true),
            _ => (false, false, true),
        }),
        _ if is_rescue_container(parent) => Some(match child.kind() {
            "rescue" => (false, false, true),
            "else" => (false, false, true),
            "ensure" => (false, false, false),
            // The body a raise can leave part-way through.
            _ if has_child_kind(parent, "rescue") => (true, true, true),
            _ => (true, true, true),
        }),
        _ => None,
    }
}

fn is_rescue_container(node: Node<'_>) -> bool {
    has_child_kind(node, "rescue") || has_child_kind(node, "ensure")
}

fn has_child_kind(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(|c| c.kind() == kind)
}

fn field_name(child: Node<'_>, parent: Node<'_>) -> Option<&'static str> {
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

/// `begin … end while cond` runs its body once before the condition is tested, which the parser
/// records as a distinct node type upstream.
fn post_condition_loop(node: Node<'_>) -> bool {
    matches!(node.kind(), "while_modifier" | "until_modifier")
        && node
            .child_by_field_name("body")
            .is_some_and(|body| body.kind() == "begin")
}

/// The operator token of a node whose operands tree-sitter names but whose operator it does not.
fn operator(node: Node<'_>) -> Option<&'static str> {
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

/// Whether a `binding` call passes nothing the parser would give children of its own, which is the
/// shape upstream treats as the argument-less call.
fn opaque_binding_argument(node: Node<'_>) -> bool {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return true;
    };
    match arguments.named_child(0) {
        None => true,
        Some(first) => matches!(first.kind(), "nil" | "true" | "false" | "self"),
    }
}

fn contains_kind(node: Node<'_>, kind: &str) -> bool {
    if node.kind() == kind {
        return true;
    }
    named_children(node)
        .into_iter()
        .any(|child| contains_kind(child, kind))
}

/// The branch node types `reference_assignments` looks for: an assignment under one of them may
/// be skipped on some iterations, so every one of them counts as read by the loop.
fn has_branch_ancestor(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        let branching = matches!(
            parent.kind(),
            "if" | "elsif"
                | "unless"
                | "if_modifier"
                | "unless_modifier"
                | "conditional"
                | "case"
                | "case_match"
                | "rescue"
                | "rescue_modifier"
        ) || (parent.kind() != "program" && has_child_kind(parent, "rescue"));
        if branching {
            return true;
        }
        current = parent;
    }
    false
}

/// The variable reads and the assignments a loop body holds, which decide what a later iteration
/// would see. Inner scopes are deliberately included, as upstream's `each_descendant` is.
fn collect_loop_references(
    node: Node<'_>,
    source: &SourceFile,
    names: &mut Vec<String>,
    assignments: &mut HashSet<usize>,
) {
    for child in named_children(node) {
        match child.kind() {
            "assignment" => {
                if let Some(left) = child.child_by_field_name("left") {
                    match left.kind() {
                        "identifier" => {
                            assignments.insert(child.id());
                        }
                        // Each target of a multiple assignment is an `lvasgn` of its own.
                        "left_assignment_list" => {
                            for target in named_children(left) {
                                assignments.insert(target.id());
                            }
                        }
                        _ => {}
                    }
                }
            }
            "operator_assignment" => {
                if let Some(left) = child.child_by_field_name("left")
                    && left.kind() == "identifier"
                {
                    // `foo += 1` both reads `foo` and writes it, and the write is the `lvasgn`
                    // the operator assignment wraps.
                    names.push(source.node_text(left).to_owned());
                    assignments.insert(left.id());
                }
            }
            "exception_variable" | "for" => {
                let target = if child.kind() == "for" {
                    child.child_by_field_name("pattern")
                } else {
                    child.named_child(0)
                };
                if let Some(target) = target.filter(|node| node.kind() == "identifier") {
                    assignments.insert(target.id());
                }
            }
            // Only a read is an `lvar` upstream: a method name, a parameter and an assignment
            // target all wear the same node type here.
            "identifier" if is_variable_read(child) => {
                names.push(source.node_text(child).to_owned());
            }
            _ => {}
        }
        collect_loop_references(child, source, names, assignments);
    }
}

/// Every identifier-shaped word written inside a `#{…}` of `text`. Reading a name that only looks
/// like a variable costs nothing -- the table has no entry for it -- while missing one would turn
/// a used variable into a reported offense.
fn interpolated_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("#{") {
        rest = &rest[start + 2..];
        let end = rest.find('}').unwrap_or(rest.len());
        let (expression, after) = rest.split_at(end);
        for word in expression.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if word
                .chars()
                .next()
                .is_some_and(|first| first.is_lowercase() || first == '_')
            {
                names.push(word.to_owned());
            }
        }
        rest = after;
    }
    names
}

/// Whether an identifier stands where the parser upstream would have built an `lvar`, rather than
/// a name being declared, written or called.
fn is_variable_read(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    match parent.kind() {
        "call" | "method" | "singleton_method" => field_name(node, parent) != Some("name"),
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
        // A parameter's default value is an ordinary expression; only its name is a declaration.
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
fn named_captures(source: &str) -> Vec<String> {
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

/// `in_modifier_conditional?`: an assignment made in `foo = 1 if bar` is not in scope to the left
/// of the keyword, so a read there cannot be the one that uses it.
fn in_modifier_conditional(assignment: Node<'_>, reference: Node<'_>) -> bool {
    let mut current = assignment;
    while let Some(parent) = current.parent() {
        if matches!(
            parent.kind(),
            "if_modifier" | "unless_modifier" | "while_modifier" | "until_modifier"
        ) && let Some(condition) = parent.child_by_field_name("condition")
            && covers(condition, assignment)
        {
            return covers(parent, reference) && !covers(condition, reference);
        }
        current = parent;
    }
    false
}

fn covers(container: Node<'_>, node: Node<'_>) -> bool {
    container.id() == node.id()
        || (container.start_byte() <= node.start_byte() && node.end_byte() <= container.end_byte())
}

/// Each heredoc's body, paired with the `<<~X` that opened it. Both appear in the same order in
/// the file, which is what makes matching them by position sound.
fn heredoc_bodies(root: Node<'_>) -> HashMap<usize, Node<'_>> {
    let mut beginnings = Vec::new();
    let mut bodies = Vec::new();
    collect_heredocs(root, &mut beginnings, &mut bodies);
    beginnings.sort_by_key(|node: &Node<'_>| node.start_byte());
    bodies.sort_by_key(|node: &Node<'_>| node.start_byte());
    beginnings
        .into_iter()
        .zip(bodies)
        .map(|(beginning, body)| (beginning.id(), body))
        .collect()
}

fn collect_heredocs<'tree>(
    node: Node<'tree>,
    beginnings: &mut Vec<Node<'tree>>,
    bodies: &mut Vec<Node<'tree>>,
) {
    match node.kind() {
        "heredoc_beginning" => beginnings.push(node),
        "heredoc_body" => bodies.push(node),
        _ => {}
    }
    for child in named_children(node) {
        collect_heredocs(child, beginnings, bodies);
    }
}
