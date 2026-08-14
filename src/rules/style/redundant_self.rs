//! `self.` is only needed where dropping it would name something else.
//!
//! What it disambiguates is a local variable: inside a method that took a `bar` parameter or
//! assigned a `bar`, the bare `bar` reads that variable and `self.bar` calls the method. RuboCop
//! answers this with a list of names per method body that every node under it shares, filled in as
//! the walk reaches each parameter and each assignment -- so a name registered *after* a call has
//! been looked at does not save it. The walk here fires the same handlers in the same order.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Redundant `self` detected.";

/// `KEYWORDS`: names a bare call could never spell, so `self.` is what makes them a call at all.
const KEYWORDS: &[&str] = &[
    "alias",
    "and",
    "begin",
    "break",
    "case",
    "class",
    "def",
    "defined?",
    "do",
    "else",
    "elsif",
    "end",
    "ensure",
    "false",
    "for",
    "if",
    "in",
    "module",
    "next",
    "nil",
    "not",
    "or",
    "redo",
    "rescue",
    "retry",
    "return",
    "self",
    "super",
    "then",
    "true",
    "undef",
    "unless",
    "until",
    "when",
    "while",
    "yield",
    "__FILE__",
    "__LINE__",
    "__ENCODING__",
];

/// `OPERATOR_METHODS`, which read badly without a receiver and are left alone.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "[]", "[]=", "!", "!=", "!~",
];

/// `KERNEL_METHODS`: `Kernel.methods(false)` as RuboCop sees it, with `pathname` required. A private
/// `Kernel` method reached through `self.` is a different call from the bare one, so the `self.`
/// stays.
const KERNEL_METHODS: &[&str] = &[
    "Array",
    "Complex",
    "Float",
    "Hash",
    "Integer",
    "Pathname",
    "Rational",
    "String",
    "__callee__",
    "__dir__",
    "__method__",
    "`",
    "abort",
    "at_exit",
    "autoload",
    "autoload?",
    "binding",
    "block_given?",
    "caller",
    "caller_locations",
    "catch",
    "eval",
    "exec",
    "exit",
    "exit!",
    "fail",
    "fork",
    "format",
    "gets",
    "global_variables",
    "iterator?",
    "lambda",
    "load",
    "local_variables",
    "loop",
    "open",
    "p",
    "print",
    "printf",
    "proc",
    "putc",
    "puts",
    "raise",
    "rand",
    "readline",
    "readlines",
    "require",
    "require_relative",
    "select",
    "set_trace_func",
    "sleep",
    "spawn",
    "sprintf",
    "srand",
    "syscall",
    "system",
    "test",
    "throw",
    "trace_var",
    "trap",
    "untrace_var",
    "warn",
];

/// The parameter lists `on_args` is given.
const PARAMETER_LISTS: &[&str] = &["method_parameters", "block_parameters", "lambda_parameters"];

/// The conditionals whose branches may assign a name the condition already reads.
const CONDITIONALS: &[&str] = &[
    "if",
    "unless",
    "elsif",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
];

/// The names in force somewhere, shared between every node of the scope that holds them.
type Scope = Rc<RefCell<Vec<String>>>;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut cop = Cop {
        context,
        allowed: HashSet::new(),
        scopes: HashMap::new(),
        offenses,
    };
    for node in context.nodes() {
        cop.visit(node);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    /// `@allowed_send_nodes`: the calls an assignment already accounted for.
    allowed: HashSet<usize>,
    /// `@local_variables_scopes`, keyed by node.
    scopes: HashMap<usize, Scope>,
    offenses: &'a mut Vec<Offense>,
}

impl<'tree> Cop<'_, 'tree> {
    fn visit(&mut self, node: Node<'tree>) {
        match node.kind_str() {
            "method" | "singleton_method" => self.add_scope(node, Scope::default()),
            kind if PARAMETER_LISTS.contains(&kind) => self.on_args(node),
            "assignment" => self.on_assignment(node),
            "operator_assignment" => self.on_operator_assignment(node),
            // `resbody`: the exception the clause binds is a local variable in its body.
            "rescue" => self.on_resbody(node),
            "in_clause" => self.on_in_pattern(node),
            "call" => {
                // A block is a node of its own upstream, wrapped around the call rather than held
                // by it, and its scope covers everything the call was written with.
                if node.field("block").is_some() {
                    self.on_block(node);
                }
                self.on_send(node);
            }
            // `-> { }` is a `block` upstream too, hanging off a `lambda` call nothing else names.
            "lambda" => self.on_block(node),
            kind if CONDITIONALS.contains(&kind) => self.on_conditional(node),
            _ => {}
        }
    }

    /// `add_scope`: every node under this one answers with the same list.
    fn add_scope(&mut self, node: Node<'tree>, scope: Scope) {
        let mut stack = send_node::named_children(node);
        while let Some(current) = stack.pop() {
            self.scopes.insert(current.id(), Scope::clone(&scope));
            stack.extend(send_node::named_children(current));
        }
    }

    fn on_block(&mut self, node: Node<'tree>) {
        let scope = self.scope(node.id());
        self.add_scope(node, scope);
    }

    /// The list in force at a node, created empty the way the default-block hash creates it.
    fn scope(&mut self, id: usize) -> Scope {
        Scope::clone(self.scopes.entry(id).or_default())
    }

    fn names(&self, id: usize) -> Option<&Scope> {
        self.scopes.get(&id)
    }

    /// `on_args`: each parameter's name joins the scope it stands in.
    fn on_args(&mut self, node: Node<'tree>) {
        for argument in send_node::named_children(node) {
            self.on_argument(argument);
        }
    }

    fn on_argument(&mut self, node: Node<'tree>) {
        // A destructured parameter is an `mlhs` holding parameters of its own.
        if node.kind_str() == "destructured_parameter" {
            self.on_args(node);
            return;
        }
        let Some(name) = parameter_name(node, self.context) else {
            return;
        };
        self.scope(node.id()).borrow_mut().push(name);
    }

    fn on_assignment(&mut self, node: Node<'tree>) {
        let (Some(left), Some(right)) = (
            node.field("left"),
            node.field("right"),
        ) else {
            return;
        };
        match left.kind_str() {
            // `on_masgn`: every name the left-hand side binds.
            "left_assignment_list" => {
                for target in send_node::named_children(left) {
                    let name = self.context.source.node_text(target).to_owned();
                    self.add_lhs_to_scopes(right, name);
                }
            }
            "identifier" => {
                let name = self.context.source.node_text(left).to_owned();
                self.add_lhs_to_scopes(right, name);
            }
            _ => {}
        }
    }

    fn on_operator_assignment(&mut self, node: Node<'tree>) {
        let Some(left) = node.field("left") else {
            return;
        };
        self.allow_self(left);
        let operator = node
            .field("operator")
            .map(|operator| self.context.source.node_text(operator));
        // Only `||=` and `&&=` carry a name forward; every other operator reads what is already
        // there rather than introducing it.
        if !matches!(operator, Some("||=") | Some("&&=")) {
            return;
        }
        let Some(right) = node.field("right") else {
            return;
        };
        let name = self.context.source.node_text(left).to_owned();
        self.add_lhs_to_scopes(right, name);
    }

    fn on_resbody(&mut self, node: Node<'tree>) {
        let Some(variable) = send_node::named_children(node)
            .into_iter()
            .find(|child| child.kind_str() == "exception_variable")
        else {
            return;
        };
        let Some(name) = send_node::named_children(variable)
            .first()
            .filter(|bound| bound.kind_str() == "identifier")
            .map(|bound| self.context.source.node_text(*bound).to_owned())
        else {
            return;
        };
        self.scope(node.id()).borrow_mut().push(name);
    }

    /// `add_match_var_scopes`: `in [Integer => n]` binds `n` for the branch.
    fn on_in_pattern(&mut self, node: Node<'tree>) {
        let mut names = Vec::new();
        let mut stack = send_node::named_children(node);
        while let Some(current) = stack.pop() {
            if is_match_var(current) {
                names.push(self.context.source.node_text(current).to_owned());
            }
            stack.extend(send_node::named_children(current));
        }
        self.scope(node.id()).borrow_mut().extend(names);
    }

    /// `on_if` / `on_while` / `on_until`: a name the body assigns is in scope for the condition
    /// too, because the condition is read again after the first pass through the body.
    fn on_conditional(&mut self, node: Node<'tree>) {
        let Some(condition) = node.field("condition") else {
            return;
        };
        let mut names = Vec::new();
        let mut stack = send_node::named_children(node);
        while let Some(current) = stack.pop() {
            if current.kind_str() == "assignment"
                && let Some(left) = current.field("left")
            {
                match left.kind_str() {
                    "identifier" => names.push(self.context.source.node_text(left).to_owned()),
                    "left_assignment_list" => names.extend(
                        send_node::named_children(left)
                            .into_iter()
                            .map(|target| self.context.source.node_text(target).to_owned()),
                    ),
                    _ => {}
                }
            }
            stack.extend(send_node::named_children(current));
        }
        for name in names {
            self.add_lhs_to_scopes(condition, name);
        }
    }

    /// `add_lhs_to_local_variables_scopes`: the name reaches the expression that was assigned, or
    /// that expression's arguments when it is a call taking any -- a call with arguments cannot be
    /// the variable itself, but what it was given can name it.
    fn add_lhs_to_scopes(&mut self, right: Node<'tree>, name: String) {
        let arguments = match right.kind_str() {
            "call" => send_node::arguments(right),
            _ => Vec::new(),
        };
        if arguments.is_empty() {
            self.scope(right.id()).borrow_mut().push(name);
            return;
        }
        for argument in arguments {
            self.scope(argument.first().id())
                .borrow_mut()
                .push(name.clone());
        }
    }

    /// `allow_self`: the call an assignment is written against keeps its `self`, which is what
    /// stops `self.foo = 1` from being read as binding a local.
    fn allow_self(&mut self, node: Node<'tree>) {
        if node.kind_str() == "call" && self.self_receiver(node).is_some() {
            self.allowed.insert(node.id());
        }
    }

    fn on_send(&mut self, node: Node<'tree>) {
        let Some(receiver) = self.self_receiver(node) else {
            return;
        };
        if !self.regular_method_call(node) {
            return;
        }
        if node.parent_of(self.context).is_some_and(|parent| {
            matches!(
                parent.kind_str(),
                "left_assignment_list" | "destructured_left_assignment"
            )
        }) {
            return;
        }
        if self.allowed_send_node(node) || self.it_method_in_block(node) {
            return;
        }
        let Some(dot) = node.field("operator") else {
            return;
        };
        self.offenses.push(
            self.context
                .offense(MSG, receiver.byte_range())
                .corrected_by_all([
                    Edit {
                        start: receiver.start_byte(),
                        end: receiver.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: dot.start_byte(),
                        end: dot.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                ]),
        );
    }

    /// `self_receiver?`, and the `self` itself so the offense can point at it.
    fn self_receiver(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        if !send_node::is_plain_send(node, self.context) {
            return None;
        }
        node.field("receiver")
            .filter(|receiver| receiver.kind_str() == "self")
    }

    fn regular_method_call(&self, node: Node<'tree>) -> bool {
        let Some(method) = node.field("method") else {
            // `self.()` is an implicit `call`, which has no name to fall back on.
            return false;
        };
        let name = self.context.source.node_text(method);
        if OPERATOR_METHODS.contains(&name)
            || KEYWORDS.contains(&name)
            || name.starts_with(|first: char| first.is_ascii_uppercase())
        {
            return false;
        }
        // `setter_method?`: an assignment writes the `=` into the call's own location.
        !node.parent_of(self.context).is_some_and(|parent| {
            matches!(parent.kind_str(), "assignment" | "operator_assignment")
                && parent
                    .field("left")
                    .is_some_and(|left| left.id() == node.id())
        })
    }

    fn allowed_send_node(&self, node: Node<'tree>) -> bool {
        if self.allowed.contains(&node.id()) {
            return true;
        }
        let Some(method) = node.field("method") else {
            return false;
        };
        let name = self.context.source.node_text(method);
        if KERNEL_METHODS.contains(&name) {
            return true;
        }
        let mut current = Some(node);
        while let Some(visited) = current {
            if self
                .names(visited.id())
                .is_some_and(|scope| scope.borrow().iter().any(|held| held == name))
            {
                return true;
            }
            current = visited.parent_of(self.context);
        }
        false
    }

    /// `it_method_in_block?`: inside a block without parameters, a bare `it` will mean the first
    /// block parameter from Ruby 3.4, so `self.it` is how the method is reached.
    fn it_method_in_block(&self, node: Node<'tree>) -> bool {
        if node
            .field("method")
            .is_none_or(|method| self.context.source.node_text(method) != "it")
        {
            return false;
        }
        if !send_node::arguments(node).is_empty() || node.field("block").is_some() {
            return false;
        }
        let mut current = node.parent_of(self.context);
        while let Some(visited) = current {
            if matches!(visited.kind_str(), "do_block" | "block" | "lambda") {
                return send_node::named_children(visited)
                    .iter()
                    .all(|child| child.kind_str() != "block_parameters");
            }
            current = visited.parent_of(self.context);
        }
        false
    }
}

/// The name a parameter binds, or `None` for one that binds nothing.
fn parameter_name(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    match node.kind_str() {
        "identifier" => Some(context.source.node_text(node).to_owned()),
        "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter" => node
            .field("name")
            .map(|name| context.source.node_text(name).to_owned()),
        _ => None,
    }
}

/// `match_var`: a name a pattern binds, either on its own or after a `=>`.
fn is_match_var(node: Node<'_>) -> bool {
    node.kind_str() == "identifier"
        && node.parent().is_some_and(|parent| {
            matches!(
                parent.kind_str(),
                "match_pattern" | "test_pattern" | "as_pattern"
            ) || parent.kind_str() == "array_pattern"
                || parent.kind_str() == "find_pattern"
                || parent.kind_str() == "hash_pattern"
                || parent.kind_str() == "alternative_pattern"
        })
}
