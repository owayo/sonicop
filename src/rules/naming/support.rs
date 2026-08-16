//! Name spelling and variable resolution shared by the cops that enforce `EnforcedStyle`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::spurious_assignment_list;
use crate::source::SourceFile;

// `ConfigurableNaming::FORMATS` writes these with POSIX character classes on purpose, and Ruby's
// POSIX classes are Unicode-aware, so an accented lower-case letter is still lower case. Rust's
// `[[:lower:]]` is ASCII-only, so the Unicode properties are what reproduce the upstream regexes.
// The `@{0,2}` prefix is why one pattern can judge `foo`, `@foo` and `@@foo` alike.
static SNAKE_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@{0,2}[0-9\p{Lowercase}_]+[!?=]?$").unwrap());
static CAMEL_CASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@{0,2}(?:_|_?\p{Lowercase}[0-9\p{Lowercase}\p{Uppercase}]*)[!?=]?$").unwrap()
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
pub(in crate::rules) struct Variables {
    roles: HashMap<usize, Role>,
    /// The method names of receiverless calls that also name a local variable in scope.
    ///
    /// The grammar reads `collection [0]` as a call handed an array while Ruby reads it as an index
    /// on the local variable, and only the scope replay can tell the two apart. Nothing upstream
    /// corresponds to this position, so it is kept apart from the roles a variable handler sees.
    local_calls: HashSet<usize>,
}

impl Variables {
    pub(in crate::rules) fn resolve<'a>(root: Node<'_>, source: &'a SourceFile) -> Self {
        let mut roles = HashMap::new();
        let mut local_calls = HashSet::new();
        let mut scopes: Vec<Scope<'a>> = vec![Scope::new(true)];
        let mut steps = vec![Step::Visit(root)];
        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(isolated) => scopes.push(Scope::new(isolated)),
                Step::Leave => {
                    scopes.pop();
                }
                Step::Visit(node) => {
                    record(node, source, &mut scopes, &mut roles, &mut local_calls);
                    push_children(node, &mut steps);
                }
            }
        }
        Self { roles, local_calls }
    }

    /// Whether a receiverless call's name is a local variable in scope, which is what makes Ruby
    /// read `collection [0]` as an index rather than as a call handed an array.
    pub(in crate::rules) fn names_a_local(&self, node: Node<'_>) -> bool {
        self.local_calls.contains(&node.start_byte())
    }

    /// Whether RuboCop's variable handlers see this node at all: an assignment target, a
    /// parameter, or a read that resolved to a local.
    pub(in crate::rules) fn is_variable(&self, node: Node<'_>) -> bool {
        self.roles.contains_key(&node.start_byte())
    }

    /// Whether the parser would build an `lvar` here rather than a receiverless `send`.
    pub(in crate::rules) fn is_reference(&self, node: Node<'_>) -> bool {
        self.roles.get(&node.start_byte()) == Some(&Role::Reference)
    }

    /// Whether the name is being bound: an assignment target or a parameter, which is what
    /// `on_lvasgn` and `on_arg` between them see.
    pub(in crate::rules) fn is_definition(&self, node: Node<'_>) -> bool {
        self.roles.get(&node.start_byte()) == Some(&Role::Definition)
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
    local_calls: &mut HashSet<usize>,
) {
    match node.kind_str() {
        "identifier" => {
            let name = source.node_text(node);
            match position(node) {
                Position::Binding => {
                    define(scopes, name);
                    roles.insert(node.start_byte(), Role::Definition);
                }
                Position::Shadow => define(scopes, name),
                Position::MethodName => {
                    if resolves(scopes, name) {
                        local_calls.insert(node.start_byte());
                    }
                }
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
        "keyword_pattern" if node.field("value").is_none() => {
            if let Some(key) = node.field("key") {
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
    let Some((isolated, outer_fields)) = opened_scope(node.kind_str()) else {
        let start = steps.len();
        let mut cursor = node.walk();
        steps.extend(node.named_children(&mut cursor).map(Step::Visit));
        steps[start..].reverse();
        return;
    };
    let outer: Vec<Node<'tree>> = outer_fields
        .iter()
        .filter_map(|field| node.field(field))
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
    match parent.kind_str() {
        "assignment" | "operator_assignment" => bound_when(node, "left"),
        "left_assignment_list" if spurious_assignment_list(parent) => {
            // Only the name the real parser would have assigned to is bound; the items the
            // grammar swallowed ahead of it are ordinary expressions.
            if last_named_child(parent).is_some_and(|last| last.id() == node.id()) {
                Position::Binding
            } else {
                Position::Value
            }
        }
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

pub(super) fn last_named_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).last()
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

/// One heredoc, in the shape the `Heredoc` mixin hands its cops.
pub(super) struct Heredoc {
    /// `node.source_range`: the `<<~SQL` opening, which is all the parser gives the string node
    /// itself. The body and the terminator live in separate locations.
    pub opening: Range<usize>,
    /// `node.loc.heredoc_end`, which starts at the beginning of the terminator's line and so
    /// covers the indentation a `<<-` or `<<~` heredoc is allowed to close with.
    pub heredoc_end: Range<usize>,
    /// Whether the parser builds a node with no children, which is what an entirely empty body
    /// produces and what `Naming/HeredocDelimiterNaming` reports the opening for.
    pub empty: bool,
}

impl Heredoc {
    /// `Heredoc#delimiter_string`: the delimiter with any quoting stripped.
    pub(super) fn delimiter<'a>(&self, source: &'a SourceFile) -> &'a str {
        static OPENING_DELIMITER: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("(<<[~-]?)['\"`]?([^'\"`]+)['\"`]?").unwrap());
        OPENING_DELIMITER
            .captures(source.slice(self.opening.clone()))
            .and_then(|captures| captures.get(2))
            .map_or("", |group| group.as_str())
    }
}

/// The file's heredocs, in the order the `<<` openings appear.
///
/// tree-sitter splits a heredoc in two: the `heredoc_beginning` sits where the string was written
/// and the `heredoc_body` follows the statement that holds it. The parser upstream keeps them in
/// one node, so the two lists have to be paired back up, which their shared source order does --
/// bodies close in the order their openings were written.
pub(super) fn heredocs(context: &RuleContext<'_>) -> Vec<Heredoc> {
    let bodies: Vec<Node<'_>> = context.nodes_of("heredoc_body").collect();
    context
        .nodes_of("heredoc_beginning")
        .zip(bodies)
        .filter_map(|(opening, body)| {
            let mut cursor = body.walk();
            let terminator = body
                .named_children(&mut cursor)
                .find(|child| child.kind_str() == "heredoc_end")?;
            let (body_line, _) = context.source.line_column(body.start_byte());
            let (end_line, _) = context.source.line_column(terminator.start_byte());
            Some(Heredoc {
                opening: opening.byte_range(),
                heredoc_end: context.source.line_start(end_line)..terminator.end_byte(),
                // The body opens with the newline that ended the `<<` line, so a terminator on the
                // very next line means the heredoc holds nothing at all.
                empty: end_line == body_line + 1,
            })
        })
        .collect()
}

/// One entry of `node.arguments`, in the shape the parser builds it.
pub(super) struct Parameter<'tree> {
    /// The parameter node itself, whose start is what `arg_range` measures from.
    pub node: Node<'tree>,
    /// The identifier the parameter binds, absent for an anonymous `*`, `**` or `&`, for `...`
    /// and for `**nil`.
    pub name: Option<Node<'tree>>,
    /// The parser's node type, which decides both how the name is read and how far the range a
    /// cop reports reaches past it.
    pub kind: ParameterKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ParameterKind {
    Arg,
    Optarg,
    Restarg,
    Kwarg,
    Kwoptarg,
    Kwrestarg,
    Blockarg,
    Shadowarg,
    /// A destructured parameter, which holds nodes rather than a name.
    Mlhs,
    /// `...`, `**nil` and the anonymous forms, none of which name anything.
    Nameless,
}

/// The parameters of one `def`, block or lambda, in source order.
///
/// The list is what `node.arguments` yields upstream, so a destructured parameter stays one entry
/// rather than being flattened into the names inside it.
pub(super) fn parameters<'tree>(list: Node<'tree>) -> Vec<Parameter<'tree>> {
    let mut out = Vec::new();
    let mut cursor = list.walk();
    for child in list.named_children(&mut cursor) {
        let kind = match child.kind_str() {
            "identifier" if field(child) == Some("locals") => ParameterKind::Shadowarg,
            "identifier" => ParameterKind::Arg,
            "optional_parameter" => ParameterKind::Optarg,
            "splat_parameter" => ParameterKind::Restarg,
            "hash_splat_parameter" => ParameterKind::Kwrestarg,
            "keyword_parameter" => {
                if child.field("value").is_some() {
                    ParameterKind::Kwoptarg
                } else {
                    ParameterKind::Kwarg
                }
            }
            "block_parameter" => ParameterKind::Blockarg,
            "destructured_parameter" => ParameterKind::Mlhs,
            _ => ParameterKind::Nameless,
        };
        match kind {
            ParameterKind::Arg | ParameterKind::Shadowarg => out.push(Parameter {
                node: child,
                name: Some(child),
                kind,
            }),
            ParameterKind::Mlhs | ParameterKind::Nameless => out.push(Parameter {
                node: child,
                name: None,
                kind,
            }),
            ParameterKind::Optarg => expand_optional(child, &mut out),
            _ => out.push(Parameter {
                node: child,
                name: child.field("name"),
                kind,
            }),
        }
    }
    out
}

/// Pushes the optional parameters an `optional_parameter` node stands for.
///
/// tree-sitter reads `def m(x = A, y = 2)` as one parameter whose default value is a multiple
/// assignment that swallowed the parameter written after it. Ruby closes the default at the comma,
/// so the swallowed names are parameters of their own and have to be handed back one at a time.
fn expand_optional<'tree>(node: Node<'tree>, out: &mut Vec<Parameter<'tree>>) {
    out.push(Parameter {
        node,
        name: node.field("name"),
        kind: ParameterKind::Optarg,
    });
    let mut value = node.field("value");
    while let Some(assignment) = value.filter(|node| node.kind_str() == "assignment") {
        let Some(list) = assignment.field("left").filter(|left| {
            left.kind_str() == "left_assignment_list" && spurious_assignment_list(*left)
        }) else {
            return;
        };
        let Some(name) = last_named_child(list).filter(|name| name.kind_str() == "identifier")
        else {
            return;
        };
        out.push(Parameter {
            node: name,
            name: Some(name),
            kind: ParameterKind::Optarg,
        });
        value = assignment.field("right");
    }
}

/// Every identifier a parameter list binds, with the parser node type it would carry.
///
/// Unlike [`parameters`] this reaches inside a destructured parameter, because the cops that
/// dispatch on the node type see the `arg` nodes in there rather than the `mlhs` around them.
pub(super) fn bound_parameters<'tree>(list: Node<'tree>) -> Vec<(Node<'tree>, ParameterKind)> {
    let mut out = Vec::new();
    for parameter in parameters(list) {
        if parameter.kind == ParameterKind::Mlhs {
            destructured_names(parameter.node, &mut out);
        } else if let Some(name) = parameter.name {
            out.push((name, parameter.kind));
        }
    }
    out
}

fn destructured_names<'tree>(node: Node<'tree>, out: &mut Vec<(Node<'tree>, ParameterKind)>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind_str() {
            "identifier" => out.push((child, ParameterKind::Arg)),
            "splat_parameter" => {
                if let Some(name) = child.field("name") {
                    out.push((name, ParameterKind::Restarg));
                }
            }
            "destructured_parameter" => destructured_names(child, out),
            _ => {}
        }
    }
}

/// The node kinds that hold a parameter list.
pub(super) const PARAMETER_LISTS: &[&str] =
    &["method_parameters", "block_parameters", "lambda_parameters"];

/// `arg.children.first.to_s`: the name for a parameter that binds one, and the S-expression of the
/// first element for a destructured parameter.
///
/// The second case is upstream's own accident -- `UncommunicativeName` reads the first child
/// without checking that it is a symbol -- but it decides both the message and the length of the
/// range reported, so it has to be reproduced exactly.
pub(super) fn parameter_full_name(
    parameter: &Parameter<'_>,
    source: &SourceFile,
) -> Option<String> {
    if parameter.kind == ParameterKind::Mlhs {
        let mut cursor = parameter.node.walk();
        let first = parameter.node.named_children(&mut cursor).next()?;
        return Some(sexp(first, source, 0));
    }
    Some(source.node_text(parameter.name?).to_owned())
}

/// `Parser::AST::Node#to_sexp`, for the nodes a destructured parameter is built from. Children
/// that are nodes go on their own indented line, while a name stays on the head's line.
fn sexp(node: Node<'_>, source: &SourceFile, indent: usize) -> String {
    let padding = "  ".repeat(indent);
    match node.kind_str() {
        "destructured_parameter" => {
            let mut out = format!("{padding}(mlhs");
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                out.push('\n');
                out.push_str(&sexp(child, source, indent + 1));
            }
            out.push(')');
            out
        }
        "splat_parameter" => match node.field("name") {
            Some(name) => format!("{padding}(restarg :{})", source.node_text(name)),
            None => format!("{padding}(restarg)"),
        },
        _ => format!("{padding}(arg :{})", source.node_text(node)),
    }
}

/// `class_emitter_method?`: a singleton method may be named after a class defined beside it, as
/// `def self.Foo` is next to `class Foo`. RuboCop lets that through -- the method emits the class.
pub(super) fn class_emitter_method(node: Node<'_>, name: &str, source: &SourceFile) -> bool {
    if node.kind_str() != "singleton_method" {
        return false;
    }
    let mut current = node;
    while let Some(parent) = current
        .parent()
        .filter(|p| p.kind_str() == "singleton_method")
    {
        current = parent;
    }
    let Some(parent) = current.parent() else {
        return false;
    };
    let mut cursor = parent.walk();
    parent.named_children(&mut cursor).any(|child| {
        child.kind_str() == "class"
            && child
                .field("name")
                .is_some_and(|class_name| source.node_text(class_name) == name)
    })
}

/// Appends the character one Ruby escape sequence stands for. The numeric forms are the ones that
/// matter: `:"a\000"` names a method whose name holds a NUL byte, which no naming style accepts,
/// and reading the escape verbatim would have called it `a000`.
pub(super) fn unescape(escape: &str, out: &mut String) {
    let body = &escape[1..];
    let mut characters = body.chars();
    let Some(first) = characters.next() else {
        return;
    };
    match first {
        'n' => out.push('\n'),
        't' => out.push('\t'),
        'r' => out.push('\r'),
        's' => out.push(' '),
        'a' => out.push('\u{7}'),
        'b' => out.push('\u{8}'),
        'e' => out.push('\u{1b}'),
        'f' => out.push('\u{c}'),
        'v' => out.push('\u{b}'),
        '\n' => {}
        '0'..='7' => push_code_point(u32::from_str_radix(body, 8).ok(), out),
        'x' => push_code_point(u32::from_str_radix(characters.as_str(), 16).ok(), out),
        'u' => push_unicode(characters.as_str(), out),
        // `\cX`, `\C-X` and `\M-X` name control and meta characters; none of them can appear in a
        // name written in an enforced style, so the exact byte does not matter.
        'c' | 'C' | 'M' => out.push('\u{1}'),
        _ => out.push(first),
    }
}

/// `\uXXXX` names one code point and `\u{...}` names a space-separated list of them.
fn push_unicode(body: &str, out: &mut String) {
    let Some(list) = body
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        push_code_point(u32::from_str_radix(body, 16).ok(), out);
        return;
    };
    for point in list.split_whitespace() {
        push_code_point(u32::from_str_radix(point, 16).ok(), out);
    }
}

fn push_code_point(value: Option<u32>, out: &mut String) {
    // A code point Rust cannot hold is a surrogate or a raw byte; either way it is not a character
    // any naming style allows, so a placeholder that is equally unacceptable stands in for it.
    out.push(value.and_then(char::from_u32).unwrap_or('\u{1}'));
}

/// The value RuboCop reads off a `str`/`sym` node: the literal's text with its escapes resolved.
/// A literal holding an interpolation is a `dstr`/`dsym` upstream, which names nothing, and is the
/// one shape rejected here.
pub(super) fn quoted_content(node: Node<'_>, source: &SourceFile) -> Option<String> {
    let mut value = String::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind_str() {
            "string_content" => value.push_str(source.node_text(child)),
            "escape_sequence" => unescape(source.node_text(child), &mut value),
            _ => return None,
        }
    }
    Some(value)
}

/// A Ruby pattern from the configuration, compiled for the `regex` crate.
///
/// A configuration value carrying the `!ruby/regexp` tag reaches RuboCop as a `Regexp` and keeps
/// the flags written in the literal; a plain string is compiled without any. Ruby anchors `^` and
/// `$` to lines whatever the flags say, and its `\w`, `\d` and `\s` stay ASCII while the POSIX
/// classes do not, so the pattern is rewritten rather than handed over as written.
pub(crate) fn ruby_regex(value: &serde_yaml_ng::Value) -> Option<&'static Regex> {
    let (body, flags) = match value {
        serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "!ruby/regexp" => {
            let literal = tagged.value.as_str()?;
            split_regexp_literal(literal).unwrap_or((literal, ""))
        }
        other => (other.as_str()?, ""),
    };
    let mut pattern = String::from("(?m");
    if flags.contains('i') {
        pattern.push('i');
    }
    // Ruby's `/m` is what makes `.` match a newline, which the `regex` crate spells `s`.
    if flags.contains('m') {
        pattern.push('s');
    }
    if flags.contains('x') {
        pattern.push('x');
    }
    pattern.push(')');
    pattern.push_str(&translate_ruby_pattern(body));
    crate::rules::regex_cache::compiled(&pattern)
}

/// `Regexp#to_s`, which is what a pattern looks like once a message has interpolated it: the
/// enabled flags, the disabled ones, and the source, all in `m`, `i`, `x` order.
pub(super) fn ruby_regex_to_s(value: &serde_yaml_ng::Value) -> Option<String> {
    let (body, flags) = match value {
        serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "!ruby/regexp" => {
            let literal = tagged.value.as_str()?;
            split_regexp_literal(literal).unwrap_or((literal, ""))
        }
        other => (other.as_str()?, ""),
    };
    let enabled: String = "mix".chars().filter(|flag| flags.contains(*flag)).collect();
    let disabled: String = "mix"
        .chars()
        .filter(|flag| !flags.contains(*flag))
        .collect();
    let disabled = if disabled.is_empty() {
        String::new()
    } else {
        format!("-{disabled}")
    };
    Some(format!("(?{enabled}{disabled}:{body})"))
}

/// Splits `/body/flags` into its two halves. A `Regexp` dumped to YAML is written that way, so the
/// slashes and the trailing flags are not part of the pattern.
fn split_regexp_literal(literal: &str) -> Option<(&str, &str)> {
    let rest = literal.strip_prefix('/')?;
    let close = rest.rfind('/')?;
    let flags = &rest[close + 1..];
    flags
        .chars()
        .all(|flag| matches!(flag, 'i' | 'm' | 'x' | 'o' | 'n' | 'u' | 's' | 'e'))
        .then(|| (&rest[..close], flags))
}

/// Rewrites the escapes whose meaning differs between Ruby's regex engine and the `regex` crate.
///
/// Ruby's `\w`, `\d` and `\s` match ASCII only, while the crate reads them as Unicode properties,
/// so a Japanese identifier would match a pattern that rejects it upstream. The POSIX classes run
/// the other way: Ruby's are Unicode-aware and the crate's are not.
fn translate_ruby_pattern(pattern: &str) -> String {
    const POSIX_CLASSES: &[(&str, &str)] = &[
        ("[:alpha:]", r"\p{Alphabetic}"),
        ("[:alnum:]", r"\p{Alphabetic}\p{Nd}"),
        ("[:upper:]", r"\p{Uppercase}"),
        ("[:lower:]", r"\p{Lowercase}"),
        ("[:digit:]", r"\p{Nd}"),
        ("[:space:]", r"\p{White_Space}"),
        ("[:word:]", r"\w"),
        ("[:punct:]", r"\p{P}\p{S}"),
    ];
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(character) = rest.chars().next() {
        if character == '\\' {
            let mut escaped = rest[1..].chars();
            let Some(escape) = escaped.next() else {
                out.push('\\');
                break;
            };
            match escape {
                'w' => out.push_str("[0-9A-Za-z_]"),
                'W' => out.push_str("[^0-9A-Za-z_]"),
                'd' => out.push_str("[0-9]"),
                'D' => out.push_str("[^0-9]"),
                's' => out.push_str("[ \\t\\r\\n\\x0B\\f]"),
                'S' => out.push_str("[^ \\t\\r\\n\\x0B\\f]"),
                'h' => out.push_str("[0-9a-fA-F]"),
                'H' => out.push_str("[^0-9a-fA-F]"),
                // `\Z` matches at the end of the text or just before a final newline, which the
                // crate has no escape for.
                'Z' => out.push_str("(?:\\n?\\z)"),
                _ => {
                    out.push('\\');
                    out.push(escape);
                }
            }
            rest = &rest[1 + escape.len_utf8()..];
            continue;
        }
        if let Some((name, replacement)) = POSIX_CLASSES
            .iter()
            .find(|(name, _)| rest.starts_with(*name))
        {
            out.push_str(replacement);
            rest = &rest[name.len()..];
            continue;
        }
        out.push(character);
        rest = &rest[character.len_utf8()..];
    }
    out
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
