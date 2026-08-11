//! `Layout/EmptyLinesAroundAccessModifier`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::{RuleContext, walk_named};

const MODIFIERS: [&str; 4] = ["public", "protected", "private", "module_function"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "around".to_owned());
    let mut scope = Scope::default();
    let text = context.source.text();
    // RuboCop keeps the enclosing class and block on instance variables that its callbacks fill in
    // as the walk reaches them, so what a modifier sees is whatever was visited last -- never
    // unwound on the way back out. A single pre-order pass reproduces that.
    walk_named(context.root_node(), &mut |node| match node.kind() {
        "class" => {
            let header = node
                .child_by_field_name("superclass")
                .and_then(|superclass| superclass.named_child(0))
                .unwrap_or(node);
            scope.class_first = Some(header.start_position().row + 1);
            scope.class_last = Some(node.end_position().row + 1);
        }
        "module" => {
            scope.class_first = Some(node.start_position().row + 1);
            scope.class_last = Some(node.end_position().row + 1);
        }
        "singleton_class" => {
            let identifier = node.child_by_field_name("value").unwrap_or(node);
            scope.class_first = Some(identifier.start_position().row + 1);
            scope.class_last = Some(node.end_position().row + 1);
        }
        // A block node upstream spans the call it hangs off, so its first line is the call's.
        "block" | "do_block" => {
            let owner = node.parent().unwrap_or(node);
            scope.block_line = Some(owner.start_position().row + 1);
        }
        "identifier" if MODIFIERS.contains(&&text[node.byte_range()]) => {
            inspect(context, &scope, &style, node, offenses);
        }
        _ => {}
    });
}

#[derive(Default)]
struct Scope {
    class_first: Option<usize>,
    class_last: Option<usize>,
    block_line: Option<usize>,
}

impl Scope {
    fn class_def(&self, line: usize) -> bool {
        self.class_first == Some(line.wrapping_sub(1))
    }

    fn body_end(&self, line: usize) -> bool {
        self.class_last == Some(line + 1)
    }

    fn block_start(&self, line: usize) -> bool {
        self.block_line == Some(line.wrapping_sub(1))
    }
}

fn inspect(
    context: &RuleContext<'_>,
    scope: &Scope,
    style: &str,
    node: Node<'_>,
    offenses: &mut Vec<Offense>,
) {
    if !in_macro_scope(context.source.text(), node) {
        return;
    }
    if right_sibling(node).is_some_and(|sibling| sibling.start_position() == node.start_position())
    {
        return;
    }
    let line = node.start_position().row + 1;
    let before = previous_line_empty(context, scope, line);
    let after = next_line_empty(context, scope, line);
    let modifier = &context.source.text()[node.byte_range()];

    let message = match style {
        "only_before" => {
            if allowed_only_before(context, node, line, before, after) {
                return;
            }
            if after {
                format!("Remove a blank line after `{modifier}`.")
            } else {
                format!("Keep a blank line before `{modifier}`.")
            }
        }
        _ => {
            if before && after {
                return;
            }
            if scope.block_start(line) || scope.class_def(line) {
                format!("Keep a blank line after `{modifier}`.")
            } else {
                format!("Keep a blank line before and after `{modifier}`.")
            }
        }
    };

    let mut offense = context.offense(message, node.byte_range());
    if let Some(edit) = correction(context, style, node, line, before, after) {
        offense = offense.corrected_by(edit);
    }
    offenses.push(offense);
}

/// `allowed_only_before_style?`.
fn allowed_only_before(
    context: &RuleContext<'_>,
    node: Node<'_>,
    line: usize,
    before: bool,
    after: bool,
) -> bool {
    let modifier = &context.source.text()[node.byte_range()];
    if modifier == "private" || modifier == "protected" {
        if context.source.line(line + 1).trim_end_matches(['\r', '\n']) == "end" {
            return true;
        }
        if after && line + 1 != context.source.line_count() {
            return false;
        }
    }
    before
}

/// `previous_line_empty?`: the nearest line above that is not a comment is blank, or the modifier
/// opens a class or block body.
fn previous_line_empty(context: &RuleContext<'_>, scope: &Scope, line: usize) -> bool {
    // `processed_source[0..(send_line - 2)]` is the whole file when the modifier is on line 1,
    // because Ruby reads the resulting `0..-1` as "everything".
    let last = if line == 1 {
        context.source.line_count()
    } else {
        line - 1
    };
    let previous = (1..=last)
        .rev()
        .map(|number| context.source.line(number))
        .find(|text| !is_comment_line(text));
    let Some(previous) = previous else {
        return true;
    };
    scope.block_start(line) || scope.class_def(line) || previous.trim().is_empty()
}

/// `next_line_empty?`.
fn next_line_empty(context: &RuleContext<'_>, scope: &Scope, line: usize) -> bool {
    scope.body_end(line) || context.source.line(line + 1).trim().is_empty()
}

fn is_comment_line(text: &str) -> bool {
    text.trim_start_matches([' ', '\t']).starts_with('#')
}

fn correction(
    context: &RuleContext<'_>,
    style: &str,
    node: Node<'_>,
    line: usize,
    before: bool,
    after: bool,
) -> Option<Edit> {
    let start = context.source.line_start(line);
    let text = context.source.text();
    let end = start + context.source.line(line).trim_end_matches('\n').len();
    let insert_before = !before && should_insert_line_before(node);
    match style {
        "only_before" => {
            // The blank line after the modifier is removed one character at a time, exactly as
            // `next_empty_line_range` describes it.
            if should_insert_line_after(node) && after && line + 1 != context.source.line_count() {
                let removal = context.source.line_start(line + 1);
                let mut replacement = String::new();
                if insert_before {
                    replacement.push('\n');
                }
                replacement.push_str(&text[start..end]);
                replacement.push_str(&text[end..removal]);
                return Some(Edit {
                    start,
                    end: removal + next_character_length(text, removal),
                    replacement,
                    safe: true,
                });
            }
            if !insert_before {
                return None;
            }
            Some(Edit {
                start,
                end,
                replacement: format!("\n{}", &text[start..end]),
                safe: true,
            })
        }
        _ => {
            let insert_after = should_insert_line_after(node) && !after;
            if !insert_before && !insert_after {
                return None;
            }
            let head = if insert_before { "\n" } else { "" };
            let tail = if insert_after { "\n" } else { "" };
            Some(Edit {
                start,
                end,
                replacement: format!("{head}{}{tail}", &text[start..end]),
                safe: true,
            })
        }
    }
}

fn next_character_length(text: &str, offset: usize) -> usize {
    text[offset..].chars().next().map_or(0, char::len_utf8)
}

fn should_insert_line_before(node: Node<'_>) -> bool {
    let Some(body) = enclosing_body(node) else {
        return true;
    };
    if !body.in_block {
        return true;
    }
    if !body.wrapped_in_begin {
        return true;
    }
    body.statements.first() != Some(&node)
}

fn should_insert_line_after(node: Node<'_>) -> bool {
    let Some(body) = enclosing_body(node) else {
        return true;
    };
    if !body.in_block {
        return true;
    }
    // With a single statement the parent upstream is the block itself, whose last child is that
    // very statement.
    body.wrapped_in_begin && body.statements.last() != Some(&node)
}

/// The statement list the modifier belongs to, and whether it hangs off a block literal.
struct Body<'tree> {
    statements: Vec<Node<'tree>>,
    /// Whether upstream wraps the list in a `begin` node, which it only does past one statement.
    wrapped_in_begin: bool,
    in_block: bool,
}

fn enclosing_body<'tree>(node: Node<'tree>) -> Option<Body<'tree>> {
    let container = node.parent()?;
    if !matches!(
        container.kind(),
        "body_statement" | "block_body" | "begin" | "program" | "then" | "else"
    ) {
        return None;
    }
    let mut cursor = container.walk();
    let statements: Vec<Node<'tree>> = container
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "rescue" | "ensure" | "else"))
        .collect();
    let wrapped_in_begin = statements.len() > 1;
    let in_block = matches!(
        container.parent().map(|parent| parent.kind()),
        Some("block" | "do_block")
    );
    Some(Body {
        statements,
        wrapped_in_begin,
        in_block,
    })
}

/// The next statement upstream would see as `right_sibling`.
fn right_sibling<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let container = node.parent()?;
    if !matches!(
        container.kind(),
        "body_statement" | "block_body" | "begin" | "program"
    ) {
        return None;
    }
    node.next_named_sibling()
}

/// `in_macro_scope?`: the modifier stands directly in a class-like body, or inside wrappers that
/// are themselves in one.
fn in_macro_scope(text: &str, node: Node<'_>) -> bool {
    let mut child = node;
    loop {
        // A block literal spans the call it hangs off upstream, so the call's own parent is what
        // the pattern climbs to.
        let anchor = match child.kind() {
            "block" | "do_block" => match child.parent() {
                Some(call) => call,
                None => return true,
            },
            _ => child,
        };
        let Some(parent) = anchor.parent() else {
            return true;
        };
        match parent.kind() {
            "class" | "module" | "singleton_class" | "program" => return true,
            "block" | "do_block" if is_class_constructor(text, parent) => return true,
            "block" | "do_block" | "then" | "else" | "elsif" => {}
            // `begin ... rescue ... end` files its statements under a `rescue` node upstream,
            // which is not one of the wrappers a macro may sit in.
            "body_statement" | "block_body" | "begin" => {
                if has_clause(parent) {
                    return false;
                }
            }
            "if" | "unless" | "if_modifier" | "unless_modifier" => {
                if parent.child_by_field_name("condition") == Some(anchor) {
                    return false;
                }
            }
            _ => return false,
        }
        child = parent;
    }
}

fn has_clause(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| matches!(child.kind(), "rescue" | "ensure" | "else"))
}

/// `class_constructor?`: a block over `Class.new`, `Module.new`, `Struct.new` or `Data.define`.
fn is_class_constructor(text: &str, block: Node<'_>) -> bool {
    let Some(call) = block.parent() else {
        return false;
    };
    let (Some(receiver), Some(method)) = (
        call.child_by_field_name("receiver"),
        call.child_by_field_name("method"),
    ) else {
        return false;
    };
    let mut receiver = receiver;
    if receiver.kind() == "scope_resolution" {
        match receiver.child_by_field_name("name") {
            Some(name) => receiver = name,
            None => return false,
        }
    }
    if receiver.kind() != "constant" {
        return false;
    }
    match &text[method.byte_range()] {
        "new" => matches!(&text[receiver.byte_range()], "Class" | "Module" | "Struct"),
        "define" => &text[receiver.byte_range()] == "Data",
        _ => false,
    }
}
