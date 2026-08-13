//! An `attr_reader` and an `attr_writer` naming the same attribute are one `attr_accessor`.
//!
//! The pairing is drawn per class body and per visibility: two macros only combine when the same
//! run of `private` / `protected` / `public` governs both, because combining them across a modifier
//! would move a method into the other visibility. `VisibilityHelp` answers that with nothing but
//! the statement's left siblings -- a bare modifier is in force until the next one.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::access_modifier::{bare_send_name, send_name, statements};
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

/// One `attr_reader`, `attr_writer` or `attr` call, read as the attributes it names.
struct AttrMacro<'tree> {
    node: Node<'tree>,
    /// `reader?`: `attr_reader` and `attr` read, `attr_writer` writes.
    reader: bool,
    /// `attrs`: `node.arguments.to_h { |attr| [attr.source, attr] }`. A name written twice keeps the
    /// place of its first appearance and the node of its last, the way assigning to a key an
    /// ordered hash already holds does.
    attrs: Vec<(String, Range<usize>)>,
    /// `node_visibility`: for a call, the last bare modifier written above it, or `public`.
    visibility: &'static str,
}

impl AttrMacro<'_> {
    /// `bisect(*names)`: `attrs.slice(*names).values`, which reads the names in the order they were
    /// given rather than the order they were written in.
    fn bisect(&self, names: &[String]) -> Vec<(String, Range<usize>)> {
        names
            .iter()
            .filter_map(|name| {
                self.attrs
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(key, range)| (key.clone(), range.clone()))
            })
            .collect()
    }

    /// `rest`: `attr_names - bisected_names`, the attributes the macro keeps.
    fn rest(&self, bisected: &[(String, Range<usize>)]) -> Vec<&str> {
        self.attrs
            .iter()
            .map(|(key, _)| key.as_str())
            .filter(|key| !bisected.iter().any(|(name, _)| name == key))
            .collect()
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for class in context.nodes_of_any(&["class", "module", "singleton_class"]) {
        check_class(context, class, offenses);
    }
}

fn check_class<'tree>(context: &RuleContext<'tree>, class: Node<'tree>, offenses: &mut Vec<Offense>) {
    let macros = find_macros(context, class);
    // `group_by(&:visibility)`, whose groups come out in the order their visibility first appeared.
    let mut visibilities: Vec<&'static str> = Vec::new();
    for attr_macro in &macros {
        if !visibilities.contains(&attr_macro.visibility) {
            visibilities.push(attr_macro.visibility);
        }
    }
    for visibility in visibilities {
        let group: Vec<&AttrMacro<'_>> = macros
            .iter()
            .filter(|attr_macro| attr_macro.visibility == visibility)
            .collect();
        let bisection = find_bisection(&group);
        if bisection.is_empty() {
            continue;
        }
        for attr_macro in group {
            let bisected = attr_macro.bisect(&bisection);
            if bisected.is_empty() {
                continue;
            }
            report(context, attr_macro, &bisected, offenses);
        }
    }
}

/// `find_macros`: the `attr*` calls written directly in the class body, each carrying the
/// visibility in force where it stands.
///
/// A body of one statement is that statement upstream rather than a `begin`, and a body that
/// bisects needs both a reader and a writer, so only a body of two or more can hold a pair. A
/// `rescue` or `ensure` clause makes the body a node of its own whose children are the clauses,
/// which likewise cannot hold two macros side by side.
fn find_macros<'tree>(context: &RuleContext<'tree>, class: Node<'tree>) -> Vec<AttrMacro<'tree>> {
    let Some(body) = class.field("body") else {
        return Vec::new();
    };
    let Some(body_statements) = statements(body) else {
        return Vec::new();
    };
    if body_statements.len() < 2 {
        return Vec::new();
    }
    let mut visibility = "public";
    let mut macros = Vec::new();
    for statement in body_statements {
        // `find_visibility_start` reads backwards for the last bare modifier, which is the one
        // tracked forward here. `module_function` is not one of `VISIBILITY_SCOPES`.
        if let Some(name) = bare_send_name(statement, context)
            && matches!(name, "private" | "protected" | "public")
        {
            visibility = match name {
                "private" => "private",
                "protected" => "protected",
                _ => "public",
            };
            continue;
        }
        // `Macro.macro?` asks only for the method name, so a call with a receiver counts as much as
        // a bare one.
        let reader = match send_name(statement, context) {
            Some("attr_reader" | "attr") => true,
            Some("attr_writer") => false,
            _ => continue,
        };
        macros.push(AttrMacro {
            node: statement,
            reader,
            attrs: attrs(context, statement),
            visibility,
        });
    }
    macros
}

/// `node.arguments.to_h { |attr| [attr.source, attr] }`.
fn attrs(context: &RuleContext<'_>, node: Node<'_>) -> Vec<(String, Range<usize>)> {
    let mut attrs: Vec<(String, Range<usize>)> = Vec::new();
    for argument in send_node::arguments(node) {
        let range = argument.range();
        let source = context.source.slice(range.clone()).to_owned();
        match attrs.iter_mut().find(|(key, _)| *key == source) {
            Some(entry) => entry.1 = range,
            None => attrs.push((source, range)),
        }
    }
    attrs
}

/// `find_bisection`: `readers.flat_map(&:attr_names) & writers.flat_map(&:attr_names)`, which keeps
/// the readers' order and drops repeats.
fn find_bisection(group: &[&AttrMacro<'_>]) -> Vec<String> {
    let mut bisection: Vec<String> = Vec::new();
    for reader in group.iter().filter(|attr_macro| attr_macro.reader) {
        for (name, _) in &reader.attrs {
            let written = group
                .iter()
                .filter(|attr_macro| !attr_macro.reader)
                .any(|writer| writer.attrs.iter().any(|(key, _)| key == name));
            if written && !bisection.contains(name) {
                bisection.push(name.clone());
            }
        }
    }
    bisection
}

/// One offense per bisected attribute, and the rewrite of the macro they came from.
///
/// Upstream registers the offenses in `on_class` with no corrector of their own and rewrites the
/// macro once in `after_class`, because a macro can have several attributes bisected out of it.
/// That leaves every offense `correctable: false` while the rewrite still lands.
fn report(
    context: &RuleContext<'_>,
    attr_macro: &AttrMacro<'_>,
    bisected: &[(String, Range<usize>)],
    offenses: &mut Vec<Offense>,
) {
    let mut correction = Some(correct(context, attr_macro, bisected));
    for (name, range) in bisected {
        let mut offense = context.offense(
            format!("Combine both accessors into `attr_accessor {name}`."),
            range.clone(),
        );
        if let Some(edits) = correction.take() {
            offense = offense
                // `insert_before(node, ...)` hangs off the macro rather than off the attribute the
                // offense named.
                .corrections_anchored_at(attr_macro.node.byte_range())
                .corrected_without_status(edits);
        }
        offenses.push(offense);
    }
}

fn correct(
    context: &RuleContext<'_>,
    attr_macro: &AttrMacro<'_>,
    bisected: &[(String, Range<usize>)],
) -> Vec<Edit> {
    let node = attr_macro.node;
    let whole_lines = whole_lines(context, node.byte_range());
    let rest = attr_macro.rest(bisected);
    let names = |names: &[&str]| names.join(", ");
    if !attr_macro.reader {
        return match rest.is_empty() {
            true => vec![Edit {
                start: whole_lines.start,
                end: whole_lines.end,
                replacement: String::new(),
                safe: true,
            }],
            false => vec![Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!("attr_writer {}", names(&rest)),
                safe: true,
            }],
        };
    }
    let bisected_names: Vec<&str> = bisected.iter().map(|(name, _)| name.as_str()).collect();
    let attr_accessor = format!("attr_accessor {}\n", names(&bisected_names));
    let indent = indent(context, node);
    if rest.is_empty() {
        return vec![Edit {
            start: whole_lines.start,
            end: whole_lines.end,
            replacement: format!("{indent}{attr_accessor}"),
            safe: true,
        }];
    }
    vec![
        Edit {
            start: node.start_byte(),
            end: node.start_byte(),
            replacement: attr_accessor,
            safe: true,
        },
        Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: format!("{indent}attr_reader {}", names(&rest)),
            safe: true,
        },
    ]
}

/// `range_by_whole_lines(range, include_final_newline: true)`.
fn whole_lines(context: &RuleContext<'_>, range: Range<usize>) -> Range<usize> {
    let (first_line, _) = context.source.line_column(range.start);
    let (last_line, _) = context.source.line_column(range.end);
    context.source.line_start(first_line)..context.source.line_range(last_line).end
}

/// `indent(node)`: as many spaces as the column the node begins at.
fn indent(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let (_, column) = context.source.line_column(node.start_byte());
    " ".repeat(column - 1)
}
