use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::visibility::{node_visibility, statements};

const MSG_SCLASS: &str = "Do not define public methods within class << self.";
const MSG_DEF_SELF: &str = "Use `class << self` to define a class method.";

/// Which of the two ways of writing a class method the class was asked for.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "def_self".to_owned());
    if style == "def_self" {
        for node in context.nodes_of("singleton_class") {
            check_singleton_class(node, context, offenses);
        }
        return;
    }
    // `on_defs`: `def self.foo`, wherever it was written. `def Other.foo` opens no singleton class
    // of this one.
    for node in context.nodes_of("singleton_method") {
        if node
            .field("object")
            .is_some_and(|object| object.kind_str() == "self")
        {
            offenses.push(context.offense(MSG_DEF_SELF, node.byte_range()));
        }
    }
}

/// `on_sclass`: a `class << self` holding nothing but methods anyone may call.
fn check_singleton_class(node: Node<'_>, context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if node
        .field("value")
        .is_none_or(|value| value.kind_str() != "self")
    {
        return;
    }
    let definitions = def_nodes(node);
    // `all_methods_public?`.
    if definitions.is_empty()
        || !definitions
            .iter()
            .all(|definition| node_visibility(*definition, context) == "public")
    {
        return;
    }
    let ranges: Vec<Range<usize>> = definitions
        .iter()
        .map(|definition| with_comment(*definition, context))
        .collect();
    // A definition sharing its last line with the `end` that closes the singleton class leaves the
    // correction rewriting the text it also inserts into, which upstream's rewriter refuses. The
    // exception it raises leaves the offense unreported.
    if ranges.iter().any(|range| range.end >= node.end_byte()) {
        return;
    }
    let mut rewritten: Vec<String> = definitions
        .iter()
        .zip(&ranges)
        .map(|(definition, range)| rewrite(*definition, range, node, context))
        .collect();
    let mut edits: Vec<Edit> = ranges
        .iter()
        .map(|range| Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        })
        .collect();
    // `sclass_only_has_methods?`: with nothing else left in it, the singleton class goes too and
    // the first definition takes its place.
    if only_holds_methods(node) {
        edits.push(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: String::new(),
            safe: true,
        });
        if let Some(first) = rewritten.first_mut() {
            *first = first.trim().to_owned();
        }
    } else {
        rewritten.insert(0, String::new());
    }
    edits.push(Edit {
        start: node.end_byte(),
        end: node.end_byte(),
        replacement: rewritten.join("\n"),
        safe: true,
    });
    offenses.push(
        context
            .offense(MSG_SCLASS, node.byte_range())
            .corrected_by_all(edits),
    );
}

/// `def_nodes`: the definitions written straight in the singleton class body.
///
/// `def self.foo` is a `defs` upstream rather than a `def` and is not among them, and neither is a
/// definition handed to a modifier -- `private def foo` is a `send` holding one.
fn def_nodes<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    node.field("body").map_or_else(Vec::new, |body| {
        statements(body)
            .into_iter()
            .filter(|child| child.kind_str() == "method")
            .collect()
    })
}

/// `sclass_only_has_methods?`.
///
/// A body that is one statement other than a definition reaches upstream's `each_child_node` over
/// that statement's own children instead, which no body holding a definition to move ever is.
fn only_holds_methods(node: Node<'_>) -> bool {
    node.field("body").is_some_and(|body| {
        statements(body)
            .iter()
            .all(|child| child.kind_str() == "method")
    })
}

/// `extract_def_from_sclass`: the definition as it reads once moved out.
fn rewrite(
    definition: Node<'_>,
    range: &Range<usize>,
    sclass: Node<'_>,
    context: &RuleContext<'_>,
) -> String {
    let text = context.source.text();
    let mut source = text[range.clone()].to_owned();
    // `prefix_def_with_self`: the keyword rather than the first `def` in the text, which a comment
    // above the definition may also hold.
    if let Some(name) = definition.field("name") {
        let keyword = definition.start_byte() - range.start;
        let end = name.end_byte() - range.start;
        source.replace_range(
            keyword..end,
            &format!("def self.{}", context.source.node_text(name)),
        );
    }
    // `source.gsub(/^ {n}/, '')`: what the definition sat further in than the singleton class.
    let outdent = context
        .source
        .line_column(definition.start_byte())
        .1
        .saturating_sub(context.source.line_column(sclass.start_byte()).1);
    outdented(&source, outdent)
}

/// Every line with the leading spaces it can spare taken off, which is what a `^ {n}` substitution
/// does to the whole of a source.
fn outdented(source: &str, spaces: usize) -> String {
    if spaces == 0 {
        return source.to_owned();
    }
    source
        .split('\n')
        .map(
            |line| match line.len() - line.trim_start_matches(' ').len() >= spaces {
                true => &line[spaces..],
                false => line,
            },
        )
        .collect::<Vec<&str>>()
        .join("\n")
}

/// `source_range_with_comment`: from the line break above the topmost comment written over the
/// node through the end of its last line.
///
/// The comments a node carries are the ones between it and whatever code came before, so a blank
/// line between a comment and the node it documents does not separate them, while a comment
/// trailing a line of code belongs to that code.
fn with_comment(node: Node<'_>, context: &RuleContext<'_>) -> Range<usize> {
    let first_line = context.source.line_column(node.start_byte()).0;
    let mut top = first_line;
    let mut line = first_line;
    while line > 1 {
        line -= 1;
        let text = context.source.line(line).trim();
        if text.starts_with('#') {
            top = line;
        } else if !text.is_empty() {
            break;
        }
    }
    let start = context.source.line_start(top).saturating_sub(1);
    // `end_position_for`: through the end of the last line the node is written on, short of its
    // line break.
    let last_line = context.source.line_column(node.end_byte()).0;
    let last = context.source.line_range(last_line);
    let end = last.end - usize::from(context.source.slice(last.clone()).ends_with('\n'));
    start..end
}
