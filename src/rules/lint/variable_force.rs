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
//! order is what decides whether a read comes before or after an assignment.
//!
//! Where the syntax tree here disagrees with the one upstream reasons about, the difference is
//! bridged rather than worked around, and each such place says which parse Ruby's own would have
//! produced: `->(x) { }` holds its parameters one node above its body, a heredoc's body hangs off
//! the statement rather than the expression that opened it, and the grammar reads several
//! constructs as literals that Ruby reads as code -- `foo(a = 1, b = 2)`, `"%d"%[x]`, `/\c#{x}/`.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::rules::node_ext::NodeExt;
use crate::rules::support::{scope_kind, spurious_assignment_list};
use crate::source::SourceFile;

/// How a variable came into being, which decides what may be reported about it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::rules) enum Declaration {
    /// `arg`, `optarg`, `restarg`, `kwarg`, `kwoptarg`, `kwrestarg` or `blockarg`.
    Argument(Argument),
    /// `shadowarg`: the block local variable of `each { |item; buffer| }`.
    BlockLocal,
    /// An `lvasgn` and the two node types that stand in for one.
    Variable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::rules) enum Argument {
    Positional,
    Optional,
    Rest,
    Keyword,
    Block,
}

/// What an assignment writes, which decides the range it is reported at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::rules) enum AssignmentKind {
    /// An ordinary `lvasgn`, whatever syntax produced it.
    Plain,
    /// `match_with_lvasgn`: the locals `/(?<year>\d+)/ =~ text` creates.
    RegexpNamedCapture,
}

/// One read of a variable, as `VariableForce::Reference` records it.
pub(in crate::rules) struct Reference<'tree> {
    pub node: Node<'tree>,
    /// `Reference#explicit?`: false for the two reads nobody wrote, a zero-arity `super` and a
    /// `binding` call.
    pub explicit: bool,
}

pub(in crate::rules) struct Assignment<'tree> {
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
    /// The reads this write answered, which is how `Lint/ShadowedArgument` tells a read of the
    /// argument apart from a read of what overwrote it.
    pub references: Vec<Node<'tree>>,
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

pub(in crate::rules) struct Variable<'tree> {
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
    pub references: Vec<Reference<'tree>>,
    pub referenced: bool,
    /// Whether any of the references was written out. `Reference#explicit?` is false for the two
    /// that stand for a read nobody wrote: a zero-arity `super` and a `binding` call.
    pub referenced_explicitly: bool,
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

pub(in crate::rules) struct Scope<'tree> {
    /// The `def`, `class`, `block` or root node the scope belongs to.
    pub node: Node<'tree>,
    /// Whether this is the file's top level, which is not a scope node of any kind.
    pub top_level: bool,
    /// Indices into [`Analysis::variables`], in declaration order.
    pub variables: Vec<usize>,
}

pub(in crate::rules) struct Analysis<'tree> {
    /// The file's node index, which answers a parent with one hash lookup. `Node::parent` walks
    /// down from the root, and the force asks for a parent on nearly every node it visits.
    index: &'tree super::super::AstIndex<'tree>,
    /// Every scope, in the order they were left, which is the order the cops report in.
    pub scopes: Vec<Scope<'tree>>,
    pub variables: Vec<Variable<'tree>>,
    /// The identifiers that resolved to a local variable. tree-sitter cannot tell a read of a
    /// local from a receiverless call, and only the analysis knows which one the parser upstream
    /// would have built.
    lvars: crate::rules::IdSet,
    /// The references Naming handlers see. This includes implicit numbered block parameters,
    /// which are local-variable reads but are not declarations VariableForce reports.
    naming_references: crate::rules::IdSet,
    /// The `foo()` calls whose name a local variable already holds. The parentheses make these
    /// calls whatever the name resolves to, so nothing here is an `lvar` -- but writing the same
    /// name without them would be one, which is the question a cop that drops parentheses asks.
    shadowed_calls: crate::rules::IdSet,
    /// Receiverless call-name nodes whose spelling is already held by a local variable. Unlike
    /// `shadowed_calls`, this also records calls with arguments for syntax recovery such as
    /// `collection [0]`.
    local_method_names: crate::rules::IdSet,
    /// The identifier nodes that RuboCop dispatches as variable definitions. Pattern bindings and
    /// explicit block-local variables deliberately stay out: upstream names them `match_var` and
    /// `shadowarg`, neither of which the Naming cops handle.
    naming_definitions: crate::rules::IdSet,
}

impl<'tree> Analysis<'tree> {
    /// Whether the parser upstream would have built an `lvar` here rather than a receiverless call.
    pub(in crate::rules) fn is_variable_reference(&self, node: Node<'_>) -> bool {
        self.lvars.contains(&node.id())
    }

    /// Whether this `foo()` would stop being a call if its parentheses went away, because a local
    /// variable of that name is in scope. Only the walk knows what is in scope at a given node.
    pub(in crate::rules) fn shadows_a_local(&self, node: Node<'_>) -> bool {
        self.shadowed_calls.contains(&node.id())
    }

    pub(in crate::rules) fn names_a_local(&self, node: Node<'_>) -> bool {
        self.local_method_names.contains(&node.id())
    }

    /// Whether the Naming cops see this node as a variable read or definition.
    pub(in crate::rules) fn is_naming_variable(&self, node: Node<'_>) -> bool {
        self.is_naming_definition(node) || self.naming_references.contains(&node.id())
    }

    pub(in crate::rules) fn is_variable(&self, node: Node<'_>) -> bool {
        self.is_naming_variable(node)
    }

    pub(in crate::rules) fn is_reference(&self, node: Node<'_>) -> bool {
        self.naming_references.contains(&node.id())
    }

    /// Whether this node is the target of the `lvasgn` shape upstream exposes. Parameter
    /// declarations are naming definitions too, but an AST walk restricted to `lvasgn` does not
    /// visit them.
    pub(in crate::rules) fn is_local_assignment(&self, node: Node<'_>) -> bool {
        self.is_naming_definition(node) && structural_definition(node, self.index)
    }

    /// Whether the parser would dispatch this node to a Naming variable-definition handler.
    pub(in crate::rules) fn is_naming_definition(&self, node: Node<'_>) -> bool {
        if node.kind_str() == "identifier" {
            return self.naming_definitions.contains(&node.id());
        }
        matches!(
            node.kind_str(),
            "instance_variable" | "class_variable" | "global_variable"
        ) && structural_definition(node, self.index)
    }

    pub(in crate::rules) fn is_definition(&self, node: Node<'_>) -> bool {
        self.is_naming_definition(node)
    }

    pub(in crate::rules) fn run(
        index: &'tree super::super::AstIndex<'tree>,
        source: &SourceFile,
    ) -> Self {
        let root = index.root_node();
        let mut force = Force {
            index,
            source,
            scopes: Vec::new(),
            variables: Vec::new(),
            stack: Vec::new(),
            branches: Vec::new(),
            branch_index: HashMap::new(),
            heredocs: heredoc_bodies(index),
            scanned: crate::rules::IdSet::default(),
            lvars: crate::rules::IdSet::default(),
            naming_references: crate::rules::IdSet::default(),
            shadowed_calls: crate::rules::IdSet::default(),
            local_method_names: crate::rules::IdSet::default(),
            naming_definitions: crate::rules::IdSet::default(),
        };
        force.push_scope(root, true);
        force.process_children(root);
        force.pop_scope();
        Analysis {
            index,
            scopes: force.scopes,
            variables: force.variables,
            lvars: force.lvars,
            naming_references: force.naming_references,
            shadowed_calls: force.shadowed_calls,
            local_method_names: force.local_method_names,
            naming_definitions: force.naming_definitions,
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
    index: &'tree super::super::AstIndex<'tree>,
    source: &'a SourceFile,
    scopes: Vec<Scope<'tree>>,
    variables: Vec<Variable<'tree>>,
    stack: Vec<Frame<'tree>>,
    branches: Vec<Branch>,
    branch_index: HashMap<(usize, usize), usize>,
    /// Each heredoc's body, found from the `<<~X` that opened it. tree-sitter hangs the body off
    /// the enclosing statement, so a heredoc written inside a block would otherwise have its
    /// interpolations resolved in the wrong scope.
    heredocs: crate::rules::IdKeyed<Node<'tree>>,
    /// Nodes already walked in an outer scope, which the scope they sit in must not walk again.
    scanned: crate::rules::IdSet,
    lvars: crate::rules::IdSet,
    naming_references: crate::rules::IdSet,
    shadowed_calls: crate::rules::IdSet,
    local_method_names: crate::rules::IdSet,
    naming_definitions: crate::rules::IdSet,
}

// ---------------------------------------------------------------------------
// Node classification
// ---------------------------------------------------------------------------

/// Whether the `=` the grammar found is really the left half of a `=~`.
fn mislexed_match_operator(node: Node<'_>, source: &SourceFile) -> bool {
    let Some(right) = node.field("right") else {
        return false;
    };
    let mut cursor = node.walk();
    let Some(operator) = node
        .children(&mut cursor)
        .find(|child| !child.is_named() && source.node_text(*child) == "=")
    else {
        return false;
    };
    operator.end_byte() == right.start_byte() && source.node_text(right).starts_with('~')
}

/// Whether a non-local variable node is written in a position RuboCop dispatches as a variable
/// definition. Local identifiers are recorded during the force walk, but instance, class and
/// global variables do not participate in local-variable resolution and are cheaper to classify
/// directly from their parent.
fn structural_definition(node: Node<'_>, index: &super::super::AstIndex<'_>) -> bool {
    let Some(parent) = index.parent(node) else {
        return false;
    };
    match parent.kind_str() {
        "assignment" | "operator_assignment" => parent
            .field("left")
            .is_some_and(|left| left.id() == node.id()),
        "left_assignment_list" if spurious_assignment_list(parent) => {
            let mut cursor = parent.walk();
            parent
                .named_children(&mut cursor)
                .last()
                .is_some_and(|last| last.id() == node.id())
        }
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => true,
        "for" => parent
            .field("pattern")
            .is_some_and(|pattern| pattern.id() == node.id()),
        "exception_variable" => true,
        _ => false,
    }
}

/// What an assignment really stores. When the grammar swallowed the neighbouring items of a
/// comma-separated list, the node it made the right-hand side spans all of them, and only its
/// first element belongs to this write.
pub(super) fn assigned_value<'tree>(right: Node<'tree>) -> Node<'tree> {
    let Some(list) = right
        .field("left")
        .filter(|_| right.kind_str() == "assignment")
        .filter(|left| left.kind_str() == "left_assignment_list")
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
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        if !child.is_named() {
            if !cursor.goto_next_sibling() {
                break;
            }
            continue;
        }
        if !owned_by_scope(child, node, scope_node) {
            if !cursor.goto_next_sibling() {
                break;
            }
            continue;
        }
        nodes.push(child);
        scan_scope(child, scope_node, nodes);
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn owned_by_scope(child: Node<'_>, parent: Node<'_>, scope_node: Node<'_>) -> bool {
    let Some((_, outer_fields)) = scope_kind(parent.kind_str()) else {
        return true;
    };
    let outer = outer_fields
        .iter()
        .any(|field| parent.field(field).is_some_and(|f| f.id() == child.id()));
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
    let body = match scope.node.kind_str() {
        // A lambda literal keeps its statements one level down, inside the braces node.
        "lambda" => scope.node.field("body")?.field("body"),
        _ => scope.node.field("body"),
    }?;
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .any(|child| child.kind_str() != "empty_statement")
        .then_some(body)
}

/// The call a block was written on, which names the method the block was passed to.
pub(super) fn block_call<'tree>(
    scope_node: Node<'tree>,
    index: &super::super::AstIndex<'tree>,
) -> Option<Node<'tree>> {
    match scope_node.kind_str() {
        "block" | "do_block" => index
            .parent_in_tree(scope_node)
            .filter(|node| node.kind_str() == "call"),
        _ => None,
    }
}

/// `BlockNode#lambda?`: both `->() {}` and `lambda {}` reach RuboCop as a block on `lambda`.
pub(super) fn is_lambda(
    scope_node: Node<'_>,
    source: &SourceFile,
    index: &super::super::AstIndex<'_>,
) -> bool {
    scope_node.kind_str() == "lambda" || block_method(scope_node, source, index) == Some("lambda")
}

pub(super) fn block_method<'a>(
    scope_node: Node<'_>,
    source: &'a SourceFile,
    index: &super::super::AstIndex<'_>,
) -> Option<&'a str> {
    let call = block_call(scope_node, index)?;
    Some(source.node_text(call.field("method")?))
}

// ---------------------------------------------------------------------------
// Traversal
// ---------------------------------------------------------------------------

impl<'tree> Force<'tree, '_> {
    fn text(&self, node: Node<'_>) -> &str {
        self.source.node_text(node)
    }

    fn push_scope(&mut self, node: Node<'tree>, top_level: bool) {
        let block = matches!(node.kind_str(), "block" | "do_block" | "lambda");
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

    /// A node's named children, taken from the file's index. The list borrows the index rather
    /// than this walk, so a `&mut self` call inside the loop is still allowed.
    fn children(&self, node: Node<'tree>) -> std::borrow::Cow<'tree, [Node<'tree>]> {
        super::super::send_node::named_children_in(node, self.index)
    }

    fn process_children(&mut self, node: Node<'tree>) {
        let children = self.children(node);
        for child in children.iter().copied() {
            if !self.scanned.contains(&child.id()) {
                self.process_node(child);
            }
        }
    }

    fn process_node(&mut self, node: Node<'tree>) {
        if scope_kind(node.kind_str()).is_some() && !self.inline_block(node) {
            self.process_scope(node);
            return;
        }
        match node.kind_str() {
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
                // The parser registers a local the moment it *reads* the assignment, so the `paths`
                // in `paths = [paths] unless paths.is_a?(Array)` is an `lvar` on both sides of the
                // keyword -- which is the whole of what `Style/ArrayCoercion` matches on. Declaring
                // the names the body writes, with no assignment behind them yet, gets that spelling
                // right while the walk below still reaches them in execution order.
                if let Some(body) = node.field("body") {
                    self.declare_lexically(body);
                }
                if let Some(condition) = node.field("condition") {
                    self.process_node(condition);
                }
                if let Some(body) = node.field("body") {
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
            "interpolation" if swallowed_by_escape(node, self.source) => {}
            "chained_string" => self.process_chained_string(node),
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
        node.kind_str() == "block"
            && self
                .index
                .parent(node)
                .is_some_and(|parent| parent.kind_str() == "lambda")
    }

    fn process_scope(&mut self, node: Node<'tree>) {
        let (_, outer_fields) = scope_kind(node.kind_str()).expect("checked by the caller");
        for field in outer_fields {
            if let Some(child) = node.field(field) {
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
        if matches!(kind, Declaration::Argument(_)) {
            self.naming_definitions.insert(name_node.id());
        }
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
            references: Vec::new(),
            referenced: false,
            referenced_explicitly: false,
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

    /// Introduces the names an assignment inside `node` would introduce, without recording the
    /// assignment itself -- the lexical half of what the parser does when it reaches one.
    ///
    /// Anything opening a scope of its own is left alone: a name written inside a block or a `def`
    /// does not survive it, so the parser does not spell it as a variable outside either.
    fn declare_lexically(&mut self, node: Node<'tree>) {
        if opens_a_scope(node) {
            return;
        }
        // Only the plain `foo = ...` target is introduced here. A multiple assignment reaches the
        // same names through `process_assignment`, whose handling of `left_assignment_list` is not
        // worth duplicating for a shape the spelling question has never turned on.
        if matches!(node.kind_str(), "assignment" | "operator_assignment")
            && let Some(left) = node.field("left")
            && left.kind_str() == "identifier"
        {
            let name = self.text(left).to_owned();
            self.declare_unless_known(&name, node, left);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor).collect::<Vec<_>>() {
            self.declare_lexically(child);
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
        if kind == AssignmentKind::Plain {
            self.naming_definitions.insert(name.id());
        }
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
            references: Vec::new(),
            reassigned: false,
            branch,
        });
    }

    fn reference(&mut self, variable: usize, node: Node<'tree>) {
        self.capture_if_needed(variable);
        self.variables[variable].referenced_explicitly = true;
        self.record_reference(variable, node, true);
        self.mark_assignments_read(variable, node);
    }

    /// `Variable#reference!` on its own. `process_send` and `process_zero_arity_super` reach past
    /// the variable table and so never mark the variable as captured by a block, which is what
    /// keeps `binding = proc { binding }` reportable.
    fn reference_without_capture(&mut self, variable: usize, node: Node<'tree>) {
        self.record_reference(variable, node, false);
        self.mark_assignments_read(variable, node);
    }

    /// The rest of `Variable#reference!`: which of the writes so far the read consumed.
    fn mark_assignments_read(&mut self, variable: usize, node: Node<'tree>) {
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
                self.variables[variable].assignments[index]
                    .references
                    .push(node);
            }
            let assignment_node = self.variables[variable].assignments[index].node;
            if in_modifier_conditional(assignment_node, node, self.index) {
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

    /// `Variable#reference!` pushing the read itself, which `reference` does before it walks the
    /// assignments and the implicit readers do on their own.
    fn record_reference(&mut self, variable: usize, node: Node<'tree>, explicit: bool) {
        self.variables[variable]
            .references
            .push(Reference { node, explicit });
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
        // `f(nil, r =~ x)` is no assignment: Ruby's lexer reads `=~` as one token wherever the two
        // characters touch, while the grammar here splits them and invents a multiple assignment
        // out of the argument list it was written in.
        if mislexed_match_operator(node, self.source) {
            self.process_children(node);
            return;
        }
        let Some(left) = node.field("left") else {
            self.process_children(node);
            return;
        };
        let right = node.field("right");
        match left.kind_str() {
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
                if last.kind_str() == "identifier" {
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
        match node.kind_str() {
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
        let (Some(left), right) = (node.field("left"), node.field("right")) else {
            self.process_children(node);
            return;
        };
        if left.kind_str() != "identifier" {
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
        if self.index.parent(node).is_some_and(|parent| {
            matches!(parent.kind_str(), "alias" | "undef" | "setter")
                || (matches!(parent.kind_str(), "method" | "singleton_method")
                    && field_name(node, parent) == Some("name"))
        }) {
            return;
        }
        let name = self.text(node);
        if let Some(variable) = self.find_variable(name) {
            self.lvars.insert(node.id());
            self.naming_references.insert(node.id());
            self.reference(variable, node);
        } else if implicit_numbered_parameter(name)
            && self.stack.last().is_some_and(|frame| frame.block)
        {
            self.naming_references.insert(node.id());
        } else if name == "binding" {
            self.reference_everything(node);
        }
    }

    fn process_call(&mut self, node: Node<'tree>) {
        if node.field("receiver").is_none()
            && let Some(method) = node.field("method")
            && method.kind_str() == "identifier"
            && self.find_variable(self.text(method)).is_some()
        {
            self.local_method_names.insert(method.id());
        }
        if self.binary_operator_on_a_local(node) {
            return;
        }
        if self.name_is_taken_by_a_local(node) {
            self.shadowed_calls.insert(node.id());
        }
        if let Some(method) = node.field("method")
            && self.text(method) == "binding"
            && opaque_binding_argument(node)
        {
            self.reference_everything(node);
        }
        // `super do ... end` hands the method's own arguments on exactly as a bare `super` does:
        // upstream reads it as a block whose call is a `zsuper`, and only an argument list --
        // `super()`'s empty one included -- makes it an ordinary `super` that passes nothing.
        if let Some(method) = node.field("method")
            && method.kind_str() == "super"
            && node.field("arguments").is_none()
        {
            self.process_zero_arity_super(method);
        }
        for child in named_children(node) {
            if node.field("method").is_some_and(|m| m.id() == child.id()) {
                continue;
            }
            if !self.scanned.contains(&child.id()) {
                self.process_node(child);
            }
        }
    }

    /// Whether `foo()` is written where a local variable already holds the name `foo`.
    ///
    /// The empty parentheses make it a call either way, so no `lvar` is built here and the name is
    /// not a read of the variable. Dropping them would leave a bare name, and that one _is_ the
    /// variable -- which is why a cop that offers to drop them has to leave this call alone.
    fn name_is_taken_by_a_local(&self, node: Node<'tree>) -> bool {
        if node.field("receiver").is_some() {
            return false;
        }
        let (Some(method), Some(arguments)) = (node.field("method"), node.field("arguments"))
        else {
            return false;
        };
        method.kind_str() == "identifier"
            && arguments.named_child_count() == 0
            && self.find_variable(self.text(method)).is_some()
    }

    /// Whether the call is really `local & expr` or `local * expr`. Ruby resolves the ambiguity by
    /// what it has already seen: once the name is a local variable, the `&` is an operator and not
    /// the start of a block-pass argument. tree-sitter has no scope to consult and always reads the
    /// argument form, so the two operands would otherwise look like a call to a method of that
    /// name with nothing reading the variable.
    fn binary_operator_on_a_local(&mut self, node: Node<'tree>) -> bool {
        if node.field("receiver").is_some() {
            return false;
        }
        let (Some(method), Some(arguments)) = (node.field("method"), node.field("arguments"))
        else {
            return false;
        };
        if method.kind_str() != "identifier"
            || self.text(arguments).starts_with('(')
            || arguments.named_child_count() != 1
        {
            return false;
        }
        let Some(argument) = arguments
            .named_child(0)
            .filter(|child| matches!(child.kind_str(), "block_argument" | "splat_argument"))
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
            self.reference_without_capture(variable, node);
        }
    }

    /// Bare `super` passes the method's own arguments on, which reads every one of them.
    fn process_zero_arity_super(&mut self, node: Node<'tree>) {
        for variable in self.accessible_variables() {
            let method_argument = self.variables[variable].is_argument()
                && matches!(
                    self.variables[variable].scope_node.kind_str(),
                    "method" | "singleton_method"
                );
            if method_argument {
                self.reference_without_capture(variable, node);
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
        match node.kind_str() {
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
                let argument = match node.kind_str() {
                    "optional_parameter" => Argument::Optional,
                    "keyword_parameter" => Argument::Keyword,
                    "splat_parameter" | "hash_splat_parameter" => Argument::Rest,
                    "block_parameter" => Argument::Block,
                    _ => return,
                };
                // `def m(*)` and `def m(**)` name nothing, so they declare nothing.
                let Some(name_node) = node.field("name") else {
                    if let Some(value) = node.field("value") {
                        self.process_node(value);
                    }
                    return;
                };
                let name = self.text(name_node).to_owned();
                self.declare(&name, node, name_node, Declaration::Argument(argument));
                if let Some(value) = node.field("value") {
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
            .field("left")
            .filter(|left| {
                value.kind_str() == "assignment" && left.kind_str() == "left_assignment_list"
            })
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
        if last.kind_str() == "identifier" {
            let name = self.text(*last).to_owned();
            self.declare(&name, *last, *last, Declaration::Argument(argument));
        }
        if let Some(right) = value.field("right") {
            self.process_default(right, argument);
        }
    }

    /// `{ name: }` is Ruby's shorthand for `{ name: name }`, so the key reads the variable.
    fn process_pair(&mut self, node: Node<'tree>) {
        let Some(key) = node.field("key") else {
            self.process_children(node);
            return;
        };
        if node.field("value").is_some() {
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
            if child.kind_str() == "comment" {
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

    /// `"%3d %s"%[a, b]` applies the modulo operator to an array, but the grammar reads the `%` as
    /// the start of one more string literal and folds it into the concatenation. Ruby only begins a
    /// percent literal where an expression may begin, and a finished string is not such a place, so
    /// a component starting with `%` always holds code rather than text.
    fn process_chained_string(&mut self, node: Node<'tree>) {
        for (position, child) in named_children(node).into_iter().enumerate() {
            if position > 0 && self.text(child).starts_with('%') {
                for name in identifier_words(self.text(child)) {
                    self.reference_by_name(&name, child);
                }
            } else {
                self.process_node(child);
            }
        }
    }

    fn process_exception_variable(&mut self, node: Node<'tree>) {
        let Some(target) = node.named_child(0) else {
            return;
        };
        if target.kind_str() != "identifier" {
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
        let (Some(left), Some(right)) = (node.field("left"), node.field("right")) else {
            self.process_children(node);
            return;
        };
        // Only a regexp the parser can compile becomes a `match_with_lvasgn`; one holding an
        // interpolation stays an ordinary `=~` call and creates no local at all.
        if operator(node) != Some("=~")
            || left.kind_str() != "regex"
            || named_children(left)
                .iter()
                .any(|part| part.kind_str() == "interpolation")
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
        match node.kind_str() {
            "in_clause" | "match_pattern" => {
                if let Some(value) = node.field("value") {
                    self.process_node(value);
                }
                if let Some(pattern) = node.field("pattern") {
                    self.declare_pattern(pattern);
                }
                if let Some(body) = node.field("body") {
                    self.process_node(body);
                }
                if let Some(guard) = node.field("guard") {
                    self.process_node(guard);
                }
            }
            _ => self.declare_pattern(node),
        }
    }

    /// The names a pattern binds. Upstream calls them `match_var`, declares them and never
    /// assigns to them, so they only ever make a later read resolve to a local.
    fn declare_pattern(&mut self, node: Node<'tree>) {
        match node.kind_str() {
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
            "keyword_pattern" => match node.field("value") {
                Some(value) => self.declare_pattern(value),
                None => {
                    if let Some(key) = node.field("key") {
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
        match node.kind_str() {
            "for" => {
                // `for item in items` evaluates the collection first.
                if let Some(value) = node.field("value") {
                    self.process_node(value);
                }
                if let Some(pattern) = node.field("pattern") {
                    self.process_multiple_assignment_target(pattern);
                }
                if let Some(body) = node.field("body") {
                    self.process_node(body);
                }
            }
            _ if post_condition_loop(node) => {
                // `begin … end while cond` runs its body before the condition is ever read.
                if let Some(body) = node.field("body") {
                    self.process_node(body);
                }
                if let Some(condition) = node.field("condition") {
                    self.process_node(condition);
                }
            }
            _ => {
                if let Some(condition) = node.field("condition") {
                    self.process_node(condition);
                }
                if let Some(body) = node.field("body") {
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
            .filter(|child| child.kind_str() == "rescue")
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
        collect_loop_references(node, self.source, self.index, &mut names, &mut assignments);
        for name in names {
            let Some(variable) = self.find_variable(&name) else {
                continue;
            };
            // `assignment_nodes_in_loop.include?` compares parser nodes, and those compare by
            // structure rather than identity, so an assignment written the same way anywhere in
            // the scope matches one the loop holds.
            let indices: Vec<usize> = (0..self.variables[variable].assignments.len())
                .filter(|&index| {
                    let assignment = &self.variables[variable].assignments[index];
                    assignments.contains(&assignment_shape(assignment, self.source))
                })
                .collect();
            let Some(&last) = indices.last() else {
                continue;
            };
            for &index in &indices {
                let assignment = &self.variables[variable].assignments[index];
                if has_branch_ancestor(assignment.node, self.index) {
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
            let parent = self.index.parent_in_tree(current)?;
            if let Some(role) = branch_role(current, parent, self.index)
                && role.branched
            {
                return Some(self.intern_branch(role, parent, scope_node, top_level));
            }
            current = parent;
        }
    }

    fn intern_branch(
        &mut self,
        role: BranchRole<'tree>,
        control: Node<'tree>,
        scope_node: Node<'tree>,
        top_level: bool,
    ) -> usize {
        let key = (control.id(), role.child.id());
        if let Some(&index) = self.branch_index.get(&key) {
            return index;
        }
        let index = self.branches.len();
        self.branches.push(Branch {
            control: control.id(),
            child: role.child.id(),
            parent: None,
            incomplete: role.incomplete,
            jumps: role.jumps,
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

fn implicit_numbered_parameter(name: &str) -> bool {
    matches!(name.as_bytes(), [b'_', b'1'..=b'9'])
}

// ---------------------------------------------------------------------------
// Node helpers
// ---------------------------------------------------------------------------

/// Whether the child stands in a branch of the control structure above it, and how that branch
/// behaves: `(may_run_incompletely, may_jump_to_other_branch, branched)`.
fn branch_role<'tree>(
    child: Node<'tree>,
    parent: Node<'tree>,
    index: &'tree super::super::AstIndex<'tree>,
) -> Option<BranchRole<'tree>> {
    let field = field_name(child, parent);
    let always_run = BranchRole::always_run(child);
    let branched = BranchRole::branched(child);
    match parent.kind_str() {
        "if" | "elsif" | "unless" | "conditional" | "if_modifier" | "unless_modifier" | "while"
        | "until" | "while_modifier" | "until_modifier" => Some(match field {
            Some("condition") => always_run,
            _ => branched,
        }),
        "for" => Some(match field {
            Some("body") => branched,
            _ => always_run,
        }),
        "case" | "case_match" => Some(match field {
            Some("value") => always_run,
            _ => branched,
        }),
        "binary" => match operator(parent) {
            Some("&&" | "||" | "and" | "or") if field != Some("left") => Some(branched),
            Some("&&" | "||" | "and" | "or") => Some(always_run),
            _ => None,
        },
        "operator_assignment" if field != Some("left") => Some(branched),
        "operator_assignment" => Some(always_run),
        "rescue_modifier" => Some(match field {
            Some("body") => branched.escaping(),
            _ => branched,
        }),
        // `begin … rescue … ensure … end` has no node of its own here: the clauses stand beside
        // the statements they guard. Upstream wraps those statements in one node, and the whole
        // main body is a single branch, so they all have to point at the same child.
        _ if is_rescue_container(parent, index) => Some(match child.kind_str() {
            "rescue" | "else" => branched,
            "ensure" => always_run,
            _ => BranchRole::branched(main_body_anchor(parent, index)?).escaping(),
        }),
        _ => None,
    }
}

/// One arm of a control structure: the node upstream would have hung the branch off, and how the
/// arm behaves when something raises inside it.
#[derive(Clone, Copy)]
struct BranchRole<'tree> {
    child: Node<'tree>,
    incomplete: bool,
    jumps: bool,
    branched: bool,
}

impl<'tree> BranchRole<'tree> {
    fn branched(child: Node<'tree>) -> Self {
        Self {
            child,
            incomplete: false,
            jumps: false,
            branched: true,
        }
    }

    fn always_run(child: Node<'tree>) -> Self {
        Self {
            branched: false,
            ..Self::branched(child)
        }
    }

    /// `ExceptionHandler`: the guarded body may stop part-way through and continue in a rescue
    /// clause, so nothing it assigns can be ruled out by where a later read stands.
    fn escaping(self) -> Self {
        Self {
            incomplete: true,
            jumps: true,
            ..self
        }
    }
}

/// The first statement of a `begin`'s main body, which stands for the whole of it.
fn main_body_anchor<'tree>(
    container: Node<'tree>,
    index: &'tree super::super::AstIndex<'tree>,
) -> Option<Node<'tree>> {
    super::super::send_node::named_children_in(container, index)
        .iter()
        .copied()
        .find(|child| !matches!(child.kind_str(), "rescue" | "else" | "ensure"))
}

fn is_rescue_container<'tree>(
    node: Node<'tree>,
    index: &'tree super::super::AstIndex<'tree>,
) -> bool {
    has_child_kind(node, "rescue", index) || has_child_kind(node, "ensure", index)
}

fn has_child_kind<'tree>(
    node: Node<'tree>,
    kind: &str,
    index: &'tree super::super::AstIndex<'tree>,
) -> bool {
    super::super::send_node::named_children_in(node, index)
        .iter()
        .any(|child| child.kind_str() == kind)
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
    matches!(node.kind_str(), "while_modifier" | "until_modifier")
        && node
            .field("body")
            .is_some_and(|body| body.kind_str() == "begin")
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
            return Some(child.kind_str());
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

/// Whether a `binding` call passes nothing the parser would give children of its own, which is the
/// shape upstream treats as the argument-less call.
fn opaque_binding_argument(node: Node<'_>) -> bool {
    let Some(arguments) = node.field("arguments") else {
        return true;
    };
    match arguments.named_child(0) {
        None => true,
        Some(first) => matches!(first.kind_str(), "nil" | "true" | "false" | "self"),
    }
}

/// The node types that hold their own local variables, which a name written inside does not leave.
fn opens_a_scope(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "method"
            | "singleton_method"
            | "class"
            | "singleton_class"
            | "module"
            | "block"
            | "do_block"
            | "lambda"
    )
}

fn contains_kind(node: Node<'_>, kind: &str) -> bool {
    if node.kind_str() == kind {
        return true;
    }
    named_children(node)
        .into_iter()
        .any(|child| contains_kind(child, kind))
}

/// The branch node types `reference_assignments` looks for: an assignment under one of them may
/// be skipped on some iterations, so every one of them counts as read by the loop.
fn has_branch_ancestor<'tree>(
    node: Node<'tree>,
    index: &'tree super::super::AstIndex<'tree>,
) -> bool {
    let mut current = node;
    while let Some(parent) = index.parent(current) {
        let branching = matches!(
            parent.kind_str(),
            "if" | "elsif"
                | "unless"
                | "if_modifier"
                | "unless_modifier"
                | "conditional"
                | "case"
                | "case_match"
                | "rescue"
                | "rescue_modifier"
        ) || (parent.kind_str() != "program"
            && has_child_kind(parent, "rescue", index));
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
    index: &super::super::AstIndex<'_>,
    names: &mut Vec<String>,
    assignments: &mut HashSet<String>,
) {
    for child in named_children(node) {
        match child.kind_str() {
            "assignment" => {
                if let Some(left) = child.field("left") {
                    let value = child.field("right").map(assigned_value);
                    match left.kind_str() {
                        "identifier" => {
                            assignments.insert(shape(left, value, source));
                        }
                        "left_assignment_list" if spurious_assignment_list(left) => {
                            if let Some(target) = named_children(left).last() {
                                assignments.insert(shape(*target, value, source));
                            }
                        }
                        // Each target of a multiple assignment is an `lvasgn` of its own, and
                        // none of them carries a value.
                        "left_assignment_list" => {
                            for target in named_children(left) {
                                assignments.insert(shape(target, None, source));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "operator_assignment" => {
                if let Some(left) = child.field("left")
                    && left.kind_str() == "identifier"
                {
                    // `foo += 1` both reads `foo` and writes it, and the write is the `lvasgn`
                    // the operator assignment wraps, which carries no value of its own.
                    names.push(source.node_text(left).to_owned());
                    assignments.insert(shape(left, None, source));
                }
            }
            // Only a read is an `lvar` upstream: a method name, a parameter and an assignment
            // target all wear the same node type here. A `for` variable and a rescue clause's
            // variable are `lvasgn` nodes that carry no value.
            "identifier" => {
                if is_variable_read(child, index) {
                    names.push(source.node_text(child).to_owned());
                } else if bare_assignment_target(child, index) {
                    assignments.insert(shape(child, None, source));
                }
            }
            _ => {}
        }
        collect_loop_references(child, source, index, names, assignments);
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
        names.extend(identifier_words(expression));
        rest = after;
    }
    names
}

/// The words in `text` that could name a local variable. Nothing is lost by offering a name the
/// table has no entry for, while missing one would turn a variable that is read into an offense.
fn identifier_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| {
            word.chars()
                .next()
                .is_some_and(|first| first.is_lowercase() || first == '_')
        })
        .map(ToOwned::to_owned)
        .collect()
}

/// Whether an identifier stands where the parser upstream would have built an `lvar`, rather than
/// a name being declared, written or called.
pub(super) fn is_variable_read(node: Node<'_>, index: &super::super::AstIndex<'_>) -> bool {
    let Some(parent) = index.parent(node) else {
        return true;
    };
    match parent.kind_str() {
        // The receiver of `foo.bar` and of `def obj.baz` is read; the selector is not.
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

/// Whether the `#` that seems to open this interpolation was really the argument of the escape
/// before it. `/\c#{str}/` is the control character `\c#` followed by the literal text `{str}`,
/// but the grammar stops the escape at `\c` and reads the rest as an interpolation.
fn swallowed_by_escape(node: Node<'_>, source: &SourceFile) -> bool {
    node.prev_named_sibling().is_some_and(|previous| {
        previous.kind_str() == "escape_sequence"
            && previous.end_byte() == node.start_byte()
            && matches!(source.node_text(previous), "\\c" | "\\C-" | "\\M-")
    })
}

/// Whether an identifier is a write that stores no value of its own.
fn bare_assignment_target(node: Node<'_>, index: &super::super::AstIndex<'_>) -> bool {
    let Some(parent) = index.parent(node) else {
        return false;
    };
    match parent.kind_str() {
        "for" => field_name(node, parent) == Some("pattern"),
        "exception_variable" => true,
        _ => false,
    }
}

/// How an `lvasgn` node compares to another one. The parser's nodes compare by structure, so two
/// writes match when they name the same variable and store the same expression -- and a write
/// that stores nothing, such as a `for` variable, matches every other one of its name.
fn shape(name: Node<'_>, value: Option<Node<'_>>, source: &SourceFile) -> String {
    let name = source.node_text(name);
    match value {
        Some(value) => format!("{name}={}", collapse(source.node_text(value))),
        None => name.to_owned(),
    }
}

/// The shape of one recorded assignment, matching what [`shape`] builds from the syntax tree.
fn assignment_shape(assignment: &Assignment<'_>, source: &SourceFile) -> String {
    match assignment.kind {
        // A named capture is a `match_with_lvasgn`, which the loop scan never collects.
        AssignmentKind::RegexpNamedCapture => String::new(),
        AssignmentKind::Plain => shape(assignment.name, assignment.value, source),
    }
}

/// Layout is not part of a node's structure, so two expressions written with different spacing
/// still compare equal.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
fn in_modifier_conditional(
    assignment: Node<'_>,
    reference: Node<'_>,
    index: &super::super::AstIndex<'_>,
) -> bool {
    let mut current = assignment;
    while let Some(parent) = index.parent(current) {
        if matches!(
            parent.kind_str(),
            "if_modifier" | "unless_modifier" | "while_modifier" | "until_modifier"
        ) && let Some(condition) = parent.field("condition")
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
fn heredoc_bodies<'tree>(
    index: &super::super::AstIndex<'tree>,
) -> crate::rules::IdKeyed<Node<'tree>> {
    // The index already groups the file's nodes by kind, so the recursive walk this used to make
    // -- which allocated a vector of children at every node of the tree -- answers nothing new.
    let mut beginnings: Vec<Node<'tree>> = index.nodes_of_kind("heredoc_beginning").collect();
    let mut bodies: Vec<Node<'tree>> = index.nodes_of_kind("heredoc_body").collect();
    beginnings.sort_by_key(|node: &Node<'_>| node.start_byte());
    bodies.sort_by_key(|node: &Node<'_>| node.start_byte());
    beginnings
        .into_iter()
        .zip(bodies)
        .map(|(beginning, body)| (beginning.id(), body))
        .collect()
}
