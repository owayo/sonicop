//! Name spelling and variable resolution shared by the cops that enforce `EnforcedStyle`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::source::SourceFile;

// `ConfigurableNaming::FORMATS` writes these with POSIX character classes on purpose, and Ruby's
// POSIX classes are Unicode-aware, so an accented lower-case letter is still lower case. Rust's
// `[[:lower:]]` is ASCII-only, so the Unicode properties are what reproduce the upstream regexes.
// The `@{0,2}` prefix is why one pattern can judge `foo`, `@foo` and `@@foo` alike.
static SNAKE_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@{0,2}[\d\p{Lowercase}_]+[!?=]?$").unwrap());
static CAMEL_CASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@{0,2}(?:_|_?\p{Lowercase}[\d\p{Lowercase}\p{Uppercase}]*)[!?=]?$").unwrap()
});

pub(super) fn valid_name(name: &str, style: &str) -> bool {
    if style == "camelCase" {
        CAMEL_CASE.is_match(name)
    } else {
        SNAKE_CASE.is_match(name)
    }
}

/// Which identifiers in one file are variables, and in what role.
///
/// Ruby tells a local variable read apart from a receiverless method call by whether the parser
/// has already seen the name bound in the enclosing scope, and two cops here turn on that
/// distinction: `Naming/VariableName` reports every `lvar` through `on_lvar`, and
/// `Naming/ConstantName` excuses `CONST = some_method` while reporting `CONST = some_local`.
/// tree-sitter spells both as `identifier`, so the scopes have to be replayed to separate them.
pub(super) struct Variables {
    roles: HashMap<usize, Role>,
}

impl Variables {
    pub(super) fn resolve<'a>(root: Node<'_>, source: &'a SourceFile) -> Self {
        let mut roles = HashMap::new();
        let mut scopes: Vec<Scope<'a>> = vec![Scope::new(true)];
        let mut steps = vec![Step::Visit(root)];
        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(isolated) => scopes.push(Scope::new(isolated)),
                Step::Leave => {
                    scopes.pop();
                }
                Step::Visit(node) => {
                    record(node, source, &mut scopes, &mut roles);
                    push_children(node, &mut steps);
                }
            }
        }
        Self { roles }
    }

    /// Whether RuboCop's variable handlers see this node at all: an assignment target, a
    /// parameter, or a read that resolved to a local.
    pub(super) fn is_variable(&self, node: Node<'_>) -> bool {
        self.roles.contains_key(&node.start_byte())
    }

    /// Whether the parser would build an `lvar` here rather than a receiverless `send`.
    pub(super) fn is_reference(&self, node: Node<'_>) -> bool {
        self.roles.get(&node.start_byte()) == Some(&Role::Reference)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// A name being bound: what `on_lvasgn`, `on_ivasgn`, `on_arg` and the rest of the aliases see.
    Definition,
    /// A read of a name bound earlier, which `on_lvar` reports as well.
    Reference,
}

struct Scope<'a> {
    /// Whether the scope hides the names around it, as a `def` or a class body does. Blocks do
    /// not: they close over the locals of the scope they were written in.
    isolated: bool,
    names: HashSet<&'a str>,
}

impl Scope<'_> {
    fn new(isolated: bool) -> Self {
        Self {
            isolated,
            names: HashSet::new(),
        }
    }
}

enum Step<'tree> {
    Visit(Node<'tree>),
    Enter(bool),
    Leave,
}

fn record<'a>(
    node: Node<'_>,
    source: &'a SourceFile,
    scopes: &mut Vec<Scope<'a>>,
    roles: &mut HashMap<usize, Role>,
) {
    match node.kind() {
        "identifier" => {
            let name = source.node_text(node);
            match position(node) {
                Position::Binding => {
                    define(scopes, name);
                    roles.insert(node.start_byte(), Role::Definition);
                }
                Position::Shadow => define(scopes, name),
                Position::MethodName => {}
                Position::Value => {
                    if resolves(scopes, name) {
                        roles.insert(node.start_byte(), Role::Reference);
                    }
                }
            }
        }
        // Only assignment reaches these; a read of `@foo` has no handler in `Naming/VariableName`.
        "instance_variable" | "class_variable" | "global_variable" => {
            if matches!(position(node), Position::Binding) {
                roles.insert(node.start_byte(), Role::Definition);
            }
        }
        // `in {key:}` binds `key` without giving it an identifier node of its own.
        "keyword_pattern" if node.child_by_field_name("value").is_none() => {
            if let Some(key) = node.child_by_field_name("key") {
                define(scopes, source.node_text(key).trim_end_matches(':'));
            }
        }
        _ => {}
    }
}

fn define<'a>(scopes: &mut [Scope<'a>], name: &'a str) {
    if let Some(scope) = scopes.last_mut() {
        scope.names.insert(name);
    }
}

/// Whether `name` is bound anywhere in the scope chain, stopping at the first scope that hides
/// its surroundings.
fn resolves(scopes: &[Scope<'_>], name: &str) -> bool {
    for scope in scopes.iter().rev() {
        if scope.names.contains(name) {
            return true;
        }
        if scope.isolated {
            break;
        }
    }
    false
}

fn push_children<'tree>(node: Node<'tree>, steps: &mut Vec<Step<'tree>>) {
    let Some((isolated, outer_fields)) = opened_scope(node.kind()) else {
        let start = steps.len();
        let mut cursor = node.walk();
        steps.extend(node.named_children(&mut cursor).map(Step::Visit));
        steps[start..].reverse();
        return;
    };
    let outer: Vec<Node<'tree>> = outer_fields
        .iter()
        .filter_map(|field| node.child_by_field_name(field))
        .collect();
    let mut cursor = node.walk();
    let inner: Vec<Node<'tree>> = node
        .named_children(&mut cursor)
        .filter(|child| !outer.iter().any(|node| node.id() == child.id()))
        .collect();
    steps.push(Step::Leave);
    steps.extend(inner.into_iter().rev().map(Step::Visit));
    steps.push(Step::Enter(isolated));
    steps.extend(outer.into_iter().rev().map(Step::Visit));
}

/// The scope a node opens, plus the fields that are still evaluated outside it. A class body
/// cannot see the locals around it, but the superclass expression written next to it can, and the
/// same holds for the receiver of `def obj.method`.
fn opened_scope(kind: &str) -> Option<(bool, &'static [&'static str])> {
    match kind {
        "method" => Some((true, &[])),
        "singleton_method" => Some((true, &["object"])),
        "class" | "module" => Some((true, &["name", "superclass"])),
        "singleton_class" => Some((true, &["value"])),
        "block" | "do_block" | "lambda" => Some((false, &[])),
        _ => None,
    }
}

enum Position {
    /// A name the assignment or parameter list binds, and that RuboCop reports on.
    Binding,
    /// A name bound without RuboCop looking at it. Block-locals arrive as `shadowarg` and pattern
    /// matches as `match_var`, and `Naming/VariableName` has a handler for neither -- but both
    /// still make later reads of the name resolve to a local.
    Shadow,
    MethodName,
    Value,
}

fn position(node: Node<'_>) -> Position {
    let Some(parent) = node.parent() else {
        return Position::Value;
    };
    match parent.kind() {
        "assignment" | "operator_assignment" => bound_when(node, "left"),
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
            Position::Binding
        }
        "method_parameters"
        | "block_parameters"
        | "lambda_parameters"
        | "destructured_parameter" => {
            if field(node) == Some("locals") {
                Position::Shadow
            } else {
                Position::Binding
            }
        }
        "optional_parameter"
        | "keyword_parameter"
        | "splat_parameter"
        | "hash_splat_parameter"
        | "block_parameter" => bound_when(node, "name"),
        "for" => bound_when(node, "pattern"),
        "exception_variable" => Position::Binding,
        "call" => {
            if field(node) == Some("method") {
                Position::MethodName
            } else {
                Position::Value
            }
        }
        "method" | "singleton_method" => {
            if field(node) == Some("name") {
                Position::MethodName
            } else {
                Position::Value
            }
        }
        "setter" | "alias" | "undef" => Position::MethodName,
        // A pin (`in ^name`) reads an existing local instead of binding a new one.
        "variable_reference_pattern" => Position::Value,
        "as_pattern" => shadowed_when(node, "name"),
        "in_clause" => shadowed_when(node, "pattern"),
        "array_pattern"
        | "find_pattern"
        | "hash_pattern"
        | "keyword_pattern"
        | "alternative_pattern" => Position::Shadow,
        _ => Position::Value,
    }
}

fn bound_when(node: Node<'_>, field_name: &str) -> Position {
    if field(node) == Some(field_name) {
        Position::Binding
    } else {
        Position::Value
    }
}

fn shadowed_when(node: Node<'_>, field_name: &str) -> Position {
    if field(node) == Some(field_name) {
        Position::Shadow
    } else {
        Position::Value
    }
}

/// The field name `node` occupies in its parent. `child_by_field_name` cannot answer this: a
/// parent may hold several children under the same field, and only the first is reachable that
/// way.
fn field(node: Node<'_>) -> Option<&'static str> {
    let parent = node.parent()?;
    let mut cursor = parent.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        if cursor.node().id() == node.id() {
            return cursor.field_name();
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn snake_case_accepts_the_prefixes_and_suffixes_ruby_allows() {
        for name in [
            "foo_bar", "foo?", "foo!", "foo=", "_", "@foo", "@@foo", "foo1",
        ] {
            assert!(
                valid_name(name, "snake_case"),
                "{name} should be snake_case"
            );
        }
        for name in ["fooBar", "FooBar", "FOO", "@fooBar", "foo_Bar"] {
            assert!(!valid_name(name, "snake_case"), "{name} is not snake_case");
        }
    }

    #[test]
    fn camel_case_rejects_underscores_after_the_first_character() {
        for name in ["fooBar", "foo", "_foo", "_", "@fooBar", "fooBar?"] {
            assert!(valid_name(name, "camelCase"), "{name} should be camelCase");
        }
        for name in ["foo_bar", "FooBar", "__foo"] {
            assert!(!valid_name(name, "camelCase"), "{name} is not camelCase");
        }
    }

    /// RuboCop spells the formats with POSIX classes so that accented letters count as letters;
    /// an ASCII-only translation would report every non-English identifier.
    #[test]
    fn accented_letters_count_as_lower_case() {
        assert!(valid_name("é", "snake_case"));
        assert!(valid_name("é", "camelCase"));
    }
}
