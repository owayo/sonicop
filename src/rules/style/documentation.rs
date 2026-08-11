use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::comments::{AnnotationKeywords, PrecedingComments, is_annotation, is_rubocop_directive};

/// `nodoc?` without `require_all`: the comment that exempts one class or module.
static NODOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#\s*:nodoc:").unwrap());

/// `nodoc?` with `require_all`: the comment that exempts everything nested inside.
static NODOC_ALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#\s*:nodoc:\s+all\s*$").unwrap());

/// `interpreter_directive_comment?`: a comment the interpreter reads, which documents nothing.
static INTERPRETER_DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#\s*(frozen_string_literal|encoding):").unwrap());

/// The methods that make a body a bare namespace declaration rather than code.
const CONSTANT_VISIBILITY: &[&str] = &["public_constant", "private_constant"];

/// The methods `include_statement?` accepts.
const INCLUSION_METHODS: &[&str] = &["include", "extend", "prepend"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedConstants").unwrap_or_default();
    let annotation_keywords = AnnotationKeywords::new(context);
    let preceding = PrecedingComments::new(context);

    for node in context.nodes_of_any(&["class", "module"]) {
        let body = body_statements(node);
        // A class with no body documents nothing; a module with no body still has to be documented.
        if node.kind() == "class" && body.is_empty() {
            continue;
        }
        let Some(name) = node.child_by_field_name("name") else {
            continue;
        };
        if namespace(context, &body)
            || documentation_comment(context, &preceding, node, &annotation_keywords)
            || allowed
                .iter()
                .any(|entry| entry == short_name(context, name))
            || nodoc_self_or_outer_module(context, node, name)
            || include_statement_only(context, &body)
        {
            continue;
        }

        offenses.push(context.offense(
            format!(
                "Missing top-level documentation comment for `{} {}`.",
                node.kind(),
                identifier(context, node, name)
            ),
            node.start_byte()..name.end_byte(),
        ));
    }
}

/// The statements of the body, as RuboCop's `node.body` holds them: an empty list where upstream
/// has `nil`, one entry for a single expression, several where upstream builds a `begin` node.
fn body_statements<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };
    super::nodes::children(body)
}

/// `namespace?`: a body that only declares constants, which needs no documentation of its own.
fn namespace(context: &RuleContext<'_>, body: &[Node<'_>]) -> bool {
    match body {
        [] => false,
        [only] => constant_definition(*only),
        several => several
            .iter()
            .all(|child| constant_definition(*child) || constant_visibility(context, *child)),
    }
}

/// `constant_definition?`: `{class module casgn}`.
fn constant_definition(node: Node<'_>) -> bool {
    match node.kind() {
        "class" | "module" => true,
        "assignment" => node
            .child_by_field_name("left")
            .is_some_and(|left| matches!(left.kind(), "constant" | "scope_resolution")),
        _ => false,
    }
}

/// `constant_visibility_declaration?`: `(send nil? {:public_constant :private_constant} ({sym str} _))`.
fn constant_visibility(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some((name, arguments)) = receiverless_call(context, node) else {
        return false;
    };
    CONSTANT_VISIBILITY.contains(&name)
        && matches!(arguments.as_slice(), [only] if matches!(
            only.kind(),
            "simple_symbol" | "delimited_symbol" | "string"
        ))
}

/// `include_statement_only?`: a body that does nothing but mix other modules in.
///
/// Upstream recurses over the raw children of the node, so a node whose children are not AST nodes
/// fails and one with no children at all passes vacuously -- which is why a body of `true` or `[]`
/// exempts the class.
fn include_statement_only(context: &RuleContext<'_>, body: &[Node<'_>]) -> bool {
    match body {
        [] => false,
        [only] => include_statement_only_node(context, *only),
        several => several
            .iter()
            .all(|child| include_statement_only_node(context, *child)),
    }
}

fn include_statement_only_node(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    if include_statement(context, node) {
        return true;
    }
    match node.kind() {
        // Upstream builds these with no children at all, so `children.all?` is vacuously true.
        "true" | "false" | "nil" | "self" | "redo" | "retry" => true,
        "array" | "hash" | "begin" => super::nodes::children(node)
            .into_iter()
            .all(|child| include_statement_only_node(context, child)),
        _ => false,
    }
}

/// `include_statement?`: `(send nil? {:include :extend :prepend} const)`.
fn include_statement(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some((name, arguments)) = receiverless_call(context, node) else {
        return false;
    };
    INCLUSION_METHODS.contains(&name)
        && matches!(arguments.as_slice(), [only] if matches!(
            only.kind(),
            "constant" | "scope_resolution"
        ))
}

/// A call with no receiver, as its method name and argument nodes.
fn receiverless_call<'a, 'tree>(
    context: &'a RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(&'a str, Vec<Node<'tree>>)> {
    if node.kind() != "call" || node.child_by_field_name("receiver").is_some() {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    if method.kind() != "identifier" {
        return None;
    }
    let arguments = node
        .child_by_field_name("arguments")
        .map_or_else(Vec::new, super::nodes::children);
    Some((context.source.node_text(method), arguments))
}

/// `documentation_comment?`: a real comment sits directly above the definition.
fn documentation_comment(
    context: &RuleContext<'_>,
    preceding: &PrecedingComments,
    node: Node<'_>,
    keywords: &AnnotationKeywords,
) -> bool {
    if shadowed_by_enclosing_node(node) {
        return false;
    }
    let lines = preceding.above(context, node.start_byte());
    let Some(last) = lines.last() else {
        return false;
    };
    let text = context.source.text();
    let (node_line, _) = context.source.line_column(node.start_byte());
    let (last_line, _) = context.source.line_column(last.start);
    // `precede?` wants the comment on the line directly above, and `comment_line?` rejects the
    // `=begin` block, whose source does not open with a `#`.
    if node_line != last_line + 1 || !text[last.clone()].starts_with('#') {
        return false;
    }
    lines.iter().any(|range| {
        let comment = &text[range.clone()];
        !is_annotation(comment, keywords)
            && !INTERPRETER_DIRECTIVE.is_match(comment)
            && !is_rubocop_directive(comment)
    })
}

/// Node kinds that stand for a sequence of statements rather than an expression. Upstream's parser
/// either builds a `begin` node for them, which takes no leading comments, or no node at all.
const STATEMENT_SEQUENCE_KINDS: &[&str] = &[
    "program",
    "body_statement",
    "then",
    "else",
    "do",
    "block_body",
    "ensure",
];

/// Whether an enclosing node opens at the same offset, and so is handed the leading comments first.
///
/// The associator walks parents before children, so `module Foo end if bar` gives every comment
/// above it to the `if`, and the module is left undocumented however much prose precedes it.
fn shadowed_by_enclosing_node(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.start_byte() != node.start_byte() {
            return false;
        }
        if !STATEMENT_SEQUENCE_KINDS.contains(&parent.kind()) {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// `nodoc_self_or_outer_module?`: the definition, or the namespace it is written into, is tagged.
fn nodoc_self_or_outer_module(context: &RuleContext<'_>, node: Node<'_>, name: Node<'_>) -> bool {
    if nodoc_comment(context, node, false) {
        return true;
    }
    // A compact name attaches its comment to the innermost constant, which the direct check cannot
    // reach; upstream goes back for it through the two-level constant inside the name.
    context.source.node_text(name).contains("::")
        && outer_module(name).is_some_and(|outer| tagged_on_line(context, outer, false))
}

/// `nodoc_comment?`: the definition carries `:nodoc:`, or an enclosing one carries `:nodoc: all`.
fn nodoc_comment(context: &RuleContext<'_>, node: Node<'_>, require_all: bool) -> bool {
    let mut current = Some(node);
    let mut require_all = require_all;
    while let Some(definition) = current {
        let name = definition.child_by_field_name("name");
        // Only a plain constant puts the trailing comment where upstream looks for it: a compact
        // name hands it to the constant nested inside, leaving the definition itself untagged.
        if name.is_some_and(|name| name.kind() == "constant")
            && tagged_on_line(context, definition, require_all)
        {
            return true;
        }
        current = enclosing_definition(definition);
        require_all = true;
    }
    false
}

/// Whether a `:nodoc:` comment closes the node's first line.
fn tagged_on_line(context: &RuleContext<'_>, node: Node<'_>, require_all: bool) -> bool {
    let (line, _) = context.source.line_column(node.start_byte());
    let range = context.source.line_range(line);
    let Some(comment) = context
        .comment_ranges()
        .iter()
        .find(|comment| comment.start >= range.start && comment.start < range.end)
    else {
        return false;
    };
    if comment.start < node.start_byte() {
        return false;
    }
    let text = &context.source.text()[comment.clone()];
    match require_all {
        true => NODOC_ALL.is_match(text),
        false => NODOC.is_match(text),
    }
}

/// The two-level constant inside a compact name, which is where `outer_module` lands.
fn outer_module<'tree>(name: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = name;
    loop {
        if current.kind() != "scope_resolution" {
            return None;
        }
        let scope = current.child_by_field_name("scope")?;
        if scope.kind() == "constant" {
            return Some(current);
        }
        current = scope;
    }
}

fn enclosing_definition<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        if matches!(candidate.kind(), "class" | "module") {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// `identifier`: the definition's name qualified by every class and module it is nested in.
fn identifier(context: &RuleContext<'_>, node: Node<'_>, name: Node<'_>) -> String {
    let mut parts = vec![qualify(context, name)];
    let mut current = enclosing_definition(node);
    while let Some(definition) = current {
        if let Some(outer) = definition.child_by_field_name("name") {
            parts.push(qualify(context, outer));
        }
        current = enclosing_definition(definition);
    }
    parts.reverse();
    // `::Foo` contributes a `::` of its own, which the join then doubles; upstream folds the first
    // such run back down to one separator.
    parts.join("::").replacen("::::", "::", 1)
}

fn qualify(context: &RuleContext<'_>, node: Node<'_>) -> String {
    if node.kind() != "scope_resolution" {
        return context.source.node_text(node).to_owned();
    }
    let name = node
        .child_by_field_name("name")
        .map_or_else(String::new, |part| {
            context.source.node_text(part).to_owned()
        });
    match node.child_by_field_name("scope") {
        Some(scope) => format!("{}::{name}", qualify(context, scope)),
        // A leading `::` is a part of its own upstream, so the join puts a separator on both sides.
        None => format!("::::{name}"),
    }
}

/// `short_name`: the last constant of the name, which `AllowedConstants` lists.
fn short_name<'a>(context: &'a RuleContext<'_>, name: Node<'_>) -> &'a str {
    let part = match name.kind() {
        "scope_resolution" => name.child_by_field_name("name").unwrap_or(name),
        _ => name,
    };
    context.source.node_text(part)
}
