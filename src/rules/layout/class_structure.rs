//! `Layout/ClassStructure`: the order the pieces of a class body are written in.

use std::collections::BTreeMap;
use std::ops::Range;

use tree_sitter::Node;

use super::support::body_statements;
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::literals::recursive_basic_literal;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, heredoc_body, string_text, symbol_name};

/// `VisibilityHelp::VISIBILITY_SCOPES`, which `module_function` is not one of.
const VISIBILITY_SCOPES: &[&str] = &["private", "protected", "public"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(order) = context.setting::<Vec<String>>("ExpectedOrder") else {
        return;
    };
    let cop = Cop {
        context,
        order,
        // `Categories` maps a name to the macros that stand for it. Upstream searches it in the
        // order it was written; a name listed under two categories is the one place that shows.
        categories: context
            .setting::<BTreeMap<String, Vec<String>>>("Categories")
            .unwrap_or_default(),
    };
    for holder in context.nodes_of_any(&["class", "singleton_class"]) {
        cop.walk_over_nested_class_definition(holder, offenses);
    }
}

struct Cop<'ctx, 'src> {
    context: &'ctx RuleContext<'src>,
    order: Vec<String>,
    categories: BTreeMap<String, Vec<String>>,
}

impl Cop<'_, '_> {
    /// `on_class`, with the walk it drives.
    fn walk_over_nested_class_definition(&self, holder: Node<'_>, offenses: &mut Vec<Offense>) {
        let mut previous: Option<usize> = None;
        for node in class_elements(holder) {
            let classification = self.classify(node);
            if self.ignore(node, &classification) {
                continue;
            }
            let Some(index) = self.index_of(&classification) else {
                continue;
            };
            if let Some(before) = previous.filter(|before| index < *before) {
                let message = format!(
                    "`{classification}` is supposed to appear before `{}`.",
                    self.order[before]
                );
                offenses.push(self.report(node, message));
            }
            previous = Some(index);
        }
    }

    fn report(&self, node: Node<'_>, message: String) -> Offense {
        let offense = self.context.offense(message, node.byte_range());
        let Some(previous) = self.previous_for_autocorrect(node) else {
            return offense;
        };
        let current = self.source_range_with_comment(node);
        let previous = self.source_range_with_comment(previous);
        // `SafeAutoCorrect: false`: moving a definition can change what the class does, so `-a`
        // leaves it alone and only `-A` applies it.
        offense
            .corrections_anchored_at(previous.clone())
            .corrected_by_all([
                Edit {
                    start: previous.start,
                    end: previous.start,
                    replacement: self.context.source.slice(current.clone()).to_owned(),
                    safe: false,
                },
                Edit {
                    start: current.start,
                    end: current.end,
                    replacement: String::new(),
                    safe: false,
                },
            ])
    }

    /// `autocorrect`: the nearest thing written above that this one belongs in front of.
    fn previous_for_autocorrect<'tree>(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        let classification = self.classify(node);
        let dynamic = self.dynamic_constant(node);
        left_siblings(node)
            .into_iter()
            .rev()
            .find(|sibling| !self.ignore_for_autocorrect(&classification, dynamic, *sibling))
    }

    /// `ignore_for_autocorrect?`.
    fn ignore_for_autocorrect(
        &self,
        classification: &str,
        dynamic: bool,
        sibling: Node<'_>,
    ) -> bool {
        let sibling_class = self.classify(sibling);
        self.ignore(sibling, &sibling_class) || classification == sibling_class || dynamic
    }

    /// `classify`.
    fn classify(&self, node: Node<'_>) -> String {
        match node.kind_str() {
            // `block` is classified by its `send_node`; the grammar keeps the block inside the
            // call, so the call already is the node upstream would have looked at. A bare
            // `private` is an identifier here and a receiverless `send` there.
            "call" | "identifier" => self.find_send_node_category(node),
            _ => {
                let name = self.humanize_node(node);
                self.find_category(&name).unwrap_or(name)
            }
        }
    }

    /// `find_send_node_category`.
    fn find_send_node_category(&self, node: Node<'_>) -> String {
        let name = self.call_name(node);
        let category = self.find_category(&name);
        let key = category.unwrap_or_else(|| name.clone());
        let visibility_key = match def_modifier(node).is_some() {
            true => match name.ends_with("_class_method") {
                true => format!("{name}s"),
                false => format!("{name}_methods"),
            },
            false => format!("{}_{key}", self.node_visibility(node)),
        };
        match self.order.contains(&visibility_key) {
            true => visibility_key,
            false => key,
        }
    }

    /// `find_category`.
    fn find_category(&self, name: &str) -> Option<String> {
        self.categories
            .iter()
            .find(|(_, names)| names.iter().any(|listed| listed == name))
            .map(|(category, _)| category.clone())
    }

    /// `humanize_node` together with `HUMANIZED_NODE_TYPE`.
    ///
    /// Anything else falls back to the node's own kind, which never names a category and so is
    /// dropped by `ignore?` -- upstream's `node.type` does the same.
    fn humanize_node(&self, node: Node<'_>) -> String {
        match node.kind_str() {
            "method" => match self.definition_name(node).as_deref() {
                Some("initialize") => "initializer".to_owned(),
                _ => format!("{}_methods", self.node_visibility(node)),
            },
            "singleton_method" => "public_class_methods".to_owned(),
            "singleton_class" => "class_singleton".to_owned(),
            _ if constant_target(node, self.context).is_some() => "constants".to_owned(),
            _ => node.kind_str().to_owned(),
        }
    }

    /// `ignore?`.
    fn ignore(&self, node: Node<'_>, classification: &str) -> bool {
        classification.ends_with('=')
            || self.index_of(classification).is_none()
            || self.private_constant(node)
    }

    fn index_of(&self, classification: &str) -> Option<usize> {
        self.order
            .iter()
            .position(|listed| listed == classification)
    }

    /// `VisibilityHelp#node_visibility`.
    fn node_visibility(&self, node: Node<'_>) -> String {
        self.visibility_inline(node)
            .or_else(|| self.visibility_block(node))
            .unwrap_or_else(|| "public".to_owned())
    }

    /// `node_visibility_from_visibility_inline`, which only a `def` can be marked by.
    fn visibility_inline(&self, node: Node<'_>) -> Option<String> {
        if node.kind_str() != "method" {
            return None;
        }
        // `visibility_inline_on_def?`: `private def foo; end`.
        if let Some(name) = upstream_parent(node)
            .filter(|parent| def_modifier(*parent).is_some_and(|target| target.id() == node.id()))
            .map(|parent| self.call_name(parent))
            .filter(|name| VISIBILITY_SCOPES.contains(&name.as_str()))
        {
            return Some(name);
        }
        // `visibility_inline_on_method_name?`: `private :foo` written after the definition.
        let method = self.definition_name(node)?;
        right_siblings(node)
            .into_iter()
            .rev()
            .find(|sibling| self.marks_method_name(*sibling, &method))
            .map(|sibling| self.call_name(sibling))
    }

    /// `(send nil? VISIBILITY_SCOPES (sym %method_name))`.
    fn marks_method_name(&self, node: Node<'_>, method: &str) -> bool {
        if node.kind_str() != "call" || node.field("receiver").is_some() {
            return false;
        }
        if !VISIBILITY_SCOPES.contains(&self.call_name(node).as_str()) {
            return false;
        }
        match arguments(node).as_slice() {
            [only] => match only.parts() {
                [single] => symbol_name(*single, self.context) == Some(method),
                _ => false,
            },
            _ => false,
        }
    }

    /// `node_visibility_from_visibility_block`: the last bare `private` written above.
    fn visibility_block(&self, node: Node<'_>) -> Option<String> {
        left_siblings(node)
            .into_iter()
            .rev()
            .find(|sibling| self.is_visibility_block(*sibling))
            .map(|sibling| self.call_name(sibling))
    }

    /// `(send nil? VISIBILITY_SCOPES)`, which is a bare identifier in this grammar.
    fn is_visibility_block(&self, node: Node<'_>) -> bool {
        node.kind_str() == "identifier"
            && VISIBILITY_SCOPES.contains(&self.context.source.node_text(node))
    }

    /// `private_constant?`.
    fn private_constant(&self, node: Node<'_>) -> bool {
        let Some(name) = plain_constant_name(node, self.context) else {
            return false;
        };
        let Some(parent) = node.parent() else {
            return false;
        };
        let mut cursor = parent.walk();
        parent
            .named_children(&mut cursor)
            .any(|child| self.marked_as_private_constant(child, name))
    }

    /// `marked_as_private_constant?`.
    fn marked_as_private_constant(&self, node: Node<'_>, name: &str) -> bool {
        if node.kind_str() != "call" || self.call_name(node) != "private_constant" {
            return false;
        }
        arguments(node)
            .iter()
            .any(|argument| match argument.parts() {
                [single] => {
                    symbol_name(*single, self.context) == Some(name)
                        || (single.kind_str() == "string"
                            && string_text(*single, self.context) == name)
                }
                _ => false,
            })
    }

    /// `dynamic_constant?`: a constant whose value is computed rather than written out.
    fn dynamic_constant(&self, node: Node<'_>) -> bool {
        if plain_constant_name(node, self.context).is_none() {
            return false;
        }
        let Some(expression) = node.field("right") else {
            return false;
        };
        if !self.is_send(expression) {
            return false;
        }
        let frozen_literal = self.call_name(expression) == "freeze"
            && expression
                .field("receiver")
                .is_some_and(|receiver| recursive_basic_literal(receiver, self.context));
        !frozen_literal
    }

    /// `node.send_type?`: what upstream's parser builds a `send` for, which is more than the
    /// grammar's `call` -- an operator, an index, and a bare name that is not a local variable are
    /// all method calls there. `&.` is a `csend` and a call written with a block is a `block`
    /// wrapped around the send, so neither is one of them.
    fn is_send(&self, node: Node<'_>) -> bool {
        match node.kind_str() {
            "call" => {
                node.field("block").is_none()
                    && node
                        .field("operator")
                        .is_none_or(|operator| self.context.source.node_text(operator) != "&.")
            }
            "unary" | "element_reference" => true,
            "binary" => node.field("operator").is_some_and(|operator| {
                !matches!(
                    self.context.source.node_text(operator),
                    "&&" | "||" | "and" | "or"
                )
            }),
            // `__FILE__` / `__LINE__` / `__ENCODING__` read as bare names here and are resolved
            // into a literal by upstream's parser, so none of them is a call.
            "identifier" => {
                !matches!(
                    self.context.source.node_text(node),
                    "__FILE__" | "__LINE__" | "__ENCODING__"
                ) && !LocalVariables::new(self.context).is_lvar(node)
            }
            _ => false,
        }
    }

    /// `CommentsHelp#source_range_with_comment`, as this cop overrides both ends of it.
    fn source_range_with_comment(&self, node: Node<'_>) -> Range<usize> {
        self.begin_pos_with_comment(node)..self.end_position_for(node)
    }

    /// `begin_pos_with_comment`: the run of comments written straight above goes with the node.
    fn begin_pos_with_comment(&self, node: Node<'_>) -> usize {
        let first_line = node.start_position().row + 1;
        let mut first_comment = None;
        let mut line = first_line;
        while line > 1 {
            line -= 1;
            if !self.has_comment_at_line(line) {
                break;
            }
            // Only a comment on a line of its own is taken; a trailing one merely keeps the walk
            // going, exactly as upstream's `break unless comment` does.
            if self.context.source.line(line).trim_start().starts_with('#') {
                first_comment = Some(line);
            }
        }
        // `start_line_position`: the line break that ends the line before.
        self.context
            .source
            .line_start(first_comment.unwrap_or(first_line))
            .saturating_sub(1)
    }

    /// `processed_source.comment_at_line`.
    fn has_comment_at_line(&self, line: usize) -> bool {
        self.context
            .comment_ranges()
            .iter()
            .any(|comment| self.context.source.line_column(comment.start).0 == line)
    }

    /// `end_position_for`: the end of the last line the node is written on, or of the heredoc a
    /// constant's value opens.
    fn end_position_for(&self, node: Node<'_>) -> usize {
        if constant_target(node, self.context).is_some()
            && let Some(terminator) = self.heredoc_terminator(node)
        {
            return terminator + 1;
        }
        let last_line = self.context.source.line_column(node.end_byte()).0;
        let range = self.context.source.line_range(last_line);
        match self.context.source.slice(range.clone()).ends_with('\n') {
            true => range.end - 1,
            false => range.end,
        }
    }

    /// `find_heredoc`: where the heredoc opened anywhere under the node is terminated.
    fn heredoc_terminator(&self, node: Node<'_>) -> Option<usize> {
        let mut stack = vec![node];
        while let Some(current) = stack.pop() {
            if current.kind_str() == "heredoc_beginning" {
                let body = heredoc_body(current, self.context)?;
                let mut cursor = body.walk();
                return body
                    .named_children(&mut cursor)
                    .find(|child| child.kind_str() == "heredoc_end")
                    .map(|child| child.end_byte());
            }
            crate::rules::push_named_children(current, &mut stack);
        }
        None
    }

    /// The name a call was written with, which is the identifier itself for a bare one.
    fn call_name(&self, node: Node<'_>) -> String {
        let name = match node.kind_str() {
            "identifier" => node,
            _ => match node.field("method") {
                Some(method) => method,
                None => return String::new(),
            },
        };
        self.context.source.node_text(name).to_owned()
    }

    /// `def_node.method_name`.
    fn definition_name(&self, node: Node<'_>) -> Option<String> {
        node.field("name")
            .map(|name| self.context.source.node_text(name).to_owned())
    }
}

/// `class_elements` together with `flatten_class_elements`.
fn class_elements<'tree>(holder: Node<'tree>) -> Vec<Node<'tree>> {
    match holder.field("body") {
        Some(body) => flatten(body),
        None => Vec::new(),
    }
}

fn flatten<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    // A body written with `rescue` or `ensure` is one `rescue` node upstream, which holds the
    // statements rather than standing beside them -- and that node names no category, so nothing
    // written inside it is looked at.
    let mut cursor = node.walk();
    if node
        .named_children(&mut cursor)
        .any(|child| matches!(child.kind_str(), "rescue" | "ensure" | "else"))
    {
        return Vec::new();
    }
    body_statements(node)
        .into_iter()
        .flat_map(|child| match child.kind_str() {
            "begin" => flatten(child),
            _ => vec![child],
        })
        .collect()
}

/// `node.left_siblings`, taken from the statement list the node was written in.
fn left_siblings<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let (left, _) = split_siblings(node);
    left
}

/// `node.right_siblings`.
fn right_siblings<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let (_, right) = split_siblings(node);
    right
}

fn split_siblings<'tree>(node: Node<'tree>) -> (Vec<Node<'tree>>, Vec<Node<'tree>>) {
    let Some(parent) = node.parent() else {
        return (Vec::new(), Vec::new());
    };
    let statements = body_statements(parent);
    let Some(position) = statements
        .iter()
        .position(|statement| statement.id() == node.id())
    else {
        return (Vec::new(), Vec::new());
    };
    (
        statements[..position].to_vec(),
        statements[position + 1..].to_vec(),
    )
}

/// `node.parent`, with the argument list the grammar puts between a call and its arguments passed
/// over.
fn upstream_parent<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    match parent.kind_str() {
        "argument_list" => parent.parent(),
        _ => Some(parent),
    }
}

/// `MethodDispatchNode#def_modifier`: the definition a chain of bare calls such as
/// `private public def foo` wraps.
fn def_modifier<'tree>(call: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = call;
    loop {
        if current.kind_str() != "call" || current.field("receiver").is_some() {
            return None;
        }
        let arguments = current.field("arguments")?;
        let argument = arguments.named_child(0)?;
        if matches!(argument.kind_str(), "method" | "singleton_method") {
            return Some(argument);
        }
        current = argument;
    }
}

/// The constant a `casgn` names, which the grammar writes as an assignment to a constant.
fn constant_target<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() != "assignment" {
        return None;
    }
    let left = node.field("left")?;
    let _ = context;
    matches!(left.kind_str(), "constant" | "scope_resolution").then_some(left)
}

/// `node.casgn_type? && node.namespace.nil?`: a constant named without a scope in front of it.
fn plain_constant_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let left = constant_target(node, context)?;
    (left.kind_str() == "constant").then(|| context.source.node_text(left))
}
