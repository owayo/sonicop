//! `Style/AccessorGrouping`: one `attr_reader` naming every attribute, or one naming each.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::send_node;

/// `attribute_accessor?`: the macros that declare an attribute.
const ACCESSORS: &[&str] = &["attr_reader", "attr_writer", "attr_accessor", "attr"];

/// `VisibilityHelp::VISIBILITY_SCOPES`, which `module_function` is not one of.
const VISIBILITY_SCOPES: &[&str] = &["private", "protected", "public"];

/// `access_modifier?`, which does include `module_function`.
const ACCESS_MODIFIERS: &[&str] = &["private", "protected", "public", "module_function"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let grouped = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "grouped");
    let locals = LocalVariables::new(context);

    for holder in context.nodes_of_any(&["class", "module", "singleton_class"]) {
        let Some(body) = holder.child_by_field_name("body") else {
            continue;
        };
        let body = Body {
            context,
            statements: super::nodes::children(body),
            locals: &locals,
        };
        for index in 0..body.statements.len() {
            let Some(accessor) = body.accessor(index) else {
                continue;
            };
            if body.preceded_by_comment(body.statements[index]) || !body.groupable(index) {
                continue;
            }
            let siblings = body.groupable_siblings(index);
            if !((grouped && siblings.len() > 1) || (!grouped && accessor.arguments.len() > 1)) {
                continue;
            }
            let message = match grouped {
                true => format!("Group together all `{}` attributes.", accessor.name),
                false => format!("Use one attribute per `{}`.", accessor.name),
            };
            offenses.push(
                context
                    .offense(message, body.statements[index].byte_range())
                    .corrected_by(body.autocorrect(index, &accessor, &siblings, grouped)),
            );
        }
    }
}

/// The statements of one class, module or singleton class body, which is the scope every question
/// this cop asks is answered in.
struct Body<'a, 't> {
    context: &'a RuleContext<'a>,
    statements: Vec<Node<'t>>,
    locals: &'a LocalVariables<'a>,
}

/// One `attr_reader :a, :b` macro.
struct Accessor<'t> {
    name: String,
    arguments: Vec<Node<'t>>,
}

/// A statement upstream's parser builds a `send` for, and the offset its `last_line` is taken at.
///
/// A macro written with a block is a `block` node there whose `send` child stops at the block, so
/// the line the declaration ends on is the line the block opens on rather than the line it closes.
struct Send<'t> {
    node: Node<'t>,
    end: usize,
}

impl<'t> Body<'_, 't> {
    /// `attribute_accessor?`: a receiverless `attr_*` with at least one name.
    fn accessor(&self, index: usize) -> Option<Accessor<'t>> {
        let node = self.statements[index];
        if node.kind() != "call" || node.child_by_field_name("receiver").is_some() {
            return None;
        }
        let method = node.child_by_field_name("method")?;
        let name = self.context.source.node_text(method);
        if !ACCESSORS.contains(&name) {
            return None;
        }
        let arguments: Vec<Node<'t>> = send_node::arguments(node)
            .iter()
            .map(|argument| argument.first())
            .collect();
        (!arguments.is_empty()).then(|| Accessor {
            name: name.to_owned(),
            arguments,
        })
    }

    /// `each_child_node(:send)` after `block_type?` has been unwrapped: a bare `private` is a
    /// `send` upstream, which tree-sitter writes as a plain identifier.
    fn send(&self, node: Node<'t>) -> Option<Send<'t>> {
        match node.kind() {
            "call" if send_node::is_plain_send(node, self.context) => Some(Send {
                node,
                end: send_node::send_range(node, self.context).end,
            }),
            "identifier" if !self.locals.is_lvar(node) => Some(Send {
                node,
                end: node.end_byte(),
            }),
            // `a + b` and `!a` are calls upstream, named after the operator.
            "unary" | "binary" | "element_reference" => Some(Send {
                node,
                end: node.end_byte(),
            }),
            _ => None,
        }
    }

    /// `previous_line_comment?`: the line above the macro holds a comment, which is a reason to
    /// leave the macro where it is.
    fn preceded_by_comment(&self, node: Node<'_>) -> bool {
        let line = self.context.source.line_column(node.start_byte()).0;
        line > 1
            && self
                .context
                .source
                .line(line - 1)
                .trim_start()
                .starts_with('#')
    }

    /// `groupable_accessor?`: whether what precedes the macro leaves it free to move.
    fn groupable(&self, index: usize) -> bool {
        let node = self.statements[index];
        let Some(previous) = index.checked_sub(1).map(|before| self.statements[before]) else {
            return true;
        };
        let Some(previous) = self.send(previous) else {
            return true;
        };
        // An RBS::Inline annotation documents the declaration it sits on.
        let previous_line = self
            .context
            .source
            .line_column(previous.node.start_byte())
            .0;
        if self.context.comment_ranges().iter().any(|comment| {
            self.context.source.line_column(comment.start).0 == previous_line
                && self.context.source.slice(comment.clone()).starts_with("#:")
        }) {
            return false;
        }
        if self.is_accessor(previous.node) || self.is_access_modifier(previous.node) {
            return true;
        }
        // A blank line between the two means the macro was already set apart on purpose.
        let first_line = self.context.source.line_column(node.start_byte()).0;
        let last_line = self.context.source.line_column(previous.end).0;
        first_line.saturating_sub(last_line) > 1
    }

    fn is_accessor(&self, node: Node<'_>) -> bool {
        self.statements
            .iter()
            .position(|other| other.id() == node.id())
            .and_then(|index| self.accessor(index))
            .is_some()
    }

    /// `access_modifier?`: a visibility macro, with or without an argument.
    fn is_access_modifier(&self, node: Node<'_>) -> bool {
        ACCESS_MODIFIERS.contains(&self.receiverless_name(node).unwrap_or_default())
    }

    /// The name a receiverless call spells, which is what a `(send nil? :name)` pattern matches.
    fn receiverless_name(&self, node: Node<'_>) -> Option<&str> {
        match node.kind() {
            "identifier" => Some(self.context.source.node_text(node)),
            "call" if node.child_by_field_name("receiver").is_none() => node
                .child_by_field_name("method")
                .map(|method| self.context.source.node_text(method)),
            _ => None,
        }
    }

    /// `node_visibility`: the bare `private` / `protected` / `public` the macro sits under.
    fn visibility(&self, index: usize) -> &'static str {
        for statement in self.statements[..index].iter().rev() {
            // `visibility_block?` is `(send nil? SCOPE)`, so a modifier given a method name marks
            // that method alone and leaves the ones after it where they were.
            if statement.kind() == "call" && !send_node::arguments(*statement).is_empty() {
                continue;
            }
            let Some(name) = self.receiverless_name(*statement) else {
                continue;
            };
            if let Some(scope) = VISIBILITY_SCOPES.iter().find(|scope| **scope == name) {
                return scope;
            }
        }
        "public"
    }

    /// `groupable_sibling_accessors`: every macro in the body -- this one included -- that could
    /// be folded into one declaration with it.
    fn groupable_siblings(&self, index: usize) -> Vec<usize> {
        let Some(accessor) = self.accessor(index) else {
            return Vec::new();
        };
        let visibility = self.visibility(index);
        (0..self.statements.len())
            .filter(|other| {
                self.accessor(*other)
                    .is_some_and(|sibling| sibling.name == accessor.name)
                    && self.visibility(*other) == visibility
                    && self.groupable(*other)
                    && !self.preceded_by_comment(self.statements[*other])
            })
            .collect()
    }

    /// `skip_for_grouping?`: a constant written after the macro is what the group moves below.
    fn skip_for_grouping(&self, index: usize) -> bool {
        self.statements[index + 1..]
            .iter()
            .any(is_constant_assignment)
            && self
                .groupable_siblings(index)
                .iter()
                .any(|sibling| *sibling > index)
    }

    fn autocorrect(
        &self,
        index: usize,
        accessor: &Accessor<'_>,
        siblings: &[usize],
        grouped: bool,
    ) -> Edit {
        let node = self.statements[index];
        if !grouped {
            let range = self.range_with_trailing_comment(node);
            return Edit {
                start: range.start,
                end: range.end,
                replacement: self.separate_accessors(node, accessor),
                safe: true,
            };
        }
        let first = siblings.first().copied();
        if !self.skip_for_grouping(index)
            && (first == Some(index) || first.is_some_and(|first| self.skip_for_grouping(first)))
        {
            return Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: self.group_accessors(accessor, siblings),
                safe: true,
            };
        }
        // Every macro but the one that holds the group goes, along with the line it sat on.
        Edit {
            start: super::ranges::extended_left(
                self.context.source.text(),
                node.start_byte(),
                true,
            ),
            end: node.end_byte(),
            replacement: String::new(),
            safe: true,
        }
    }

    /// `group_accessors`: one declaration naming every attribute the group holds, in the order
    /// they were written and without repeating one.
    fn group_accessors(&self, accessor: &Accessor<'_>, siblings: &[usize]) -> String {
        let mut names: Vec<String> = Vec::new();
        for sibling in siblings {
            let Some(other) = self.accessor(*sibling) else {
                continue;
            };
            for argument in other.arguments {
                let text = self.context.source.node_text(argument).to_owned();
                if !names.contains(&text) {
                    names.push(text);
                }
            }
        }
        format!("{} {}", accessor.name, names.join(", "))
    }

    /// `separate_accessors`: one declaration per attribute, each indented as the original was.
    fn separate_accessors(&self, node: Node<'_>, accessor: &Accessor<'_>) -> String {
        let (_, column) = self.context.source.line_column(node.start_byte());
        let indent = " ".repeat(column - 1);
        let mut lines: Vec<String> = Vec::new();
        let mut previous = node
            .child_by_field_name("method")
            .map_or(node.start_byte(), |method| method.end_byte());
        for (position, argument) in accessor.arguments.iter().enumerate() {
            // `ast_with_comments[arg]`: a comment written before an attribute travels with it.
            let mut written: Vec<String> = self
                .context
                .comment_ranges()
                .iter()
                .filter(|comment| comment.start >= previous && comment.end <= argument.start_byte())
                .map(|comment| self.context.source.slice(comment.clone()).to_owned())
                .collect();
            written.push(format!(
                "{} {}",
                accessor.name,
                self.context.source.node_text(*argument)
            ));
            for line in written {
                lines.push(match position {
                    0 => line,
                    _ => format!("{indent}{line}"),
                });
            }
            previous = argument.end_byte();
        }
        lines.join("\n")
    }

    /// `range_with_trailing_argument_comment`: a comment written after the declaration belongs to
    /// the last attribute it names, so splitting the declaration has to carry it along.
    fn range_with_trailing_comment(&self, node: Node<'_>) -> Range<usize> {
        let line = self.context.source.line_column(node.end_byte()).0;
        let trailing = self.context.comment_ranges().iter().find(|comment| {
            comment.start >= node.end_byte()
                && self.context.source.line_column(comment.start).0 == line
        });
        node.start_byte()..trailing.map_or(node.end_byte(), |comment| comment.end)
    }
}

/// `casgn_type?`: `CONST = 1`, which the group is written below rather than above.
fn is_constant_assignment(node: &Node<'_>) -> bool {
    node.kind() == "assignment"
        && node
            .child_by_field_name("left")
            .is_some_and(|left| matches!(left.kind(), "constant" | "scope_resolution"))
}
