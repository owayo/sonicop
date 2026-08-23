use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const NESTED_MSG: &str = "Use nested module/class definitions instead of compact style.";
const COMPACT_MSG: &str = "Use compact module/class definition instead of nested style.";

/// Node kinds that hold a sequence of statements, which is where a definition has a left sibling
/// to read the namespace's kind off.
const STATEMENT_SEQUENCE_KINDS: &[&str] = &[
    "program",
    "body_statement",
    "then",
    "else",
    "do",
    "begin",
    "block_body",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "nested".to_owned());
    let for_classes: String = context
        .setting("EnforcedStyleForClasses")
        .unwrap_or_else(|| style.clone());
    let for_modules: String = context
        .setting("EnforcedStyleForModules")
        .unwrap_or_else(|| style.clone());

    for node in context.nodes_of_any(&["class", "module"]) {
        let is_class = node.kind_str() == "class";
        let style = match is_class {
            true => for_classes.as_str(),
            false => for_modules.as_str(),
        };
        // A class with a superclass cannot be compacted, so only the nested style has anything to
        // say about it.
        if is_class && node.field("superclass").is_some() && style != "nested" {
            continue;
        }
        let Some(name) = node.field("name") else {
            continue;
        };
        // `::Foo` names no namespace of its own, and a namespace that is not a constant -- `self::`
        // or a method call -- cannot be split apart.
        let splittable = match namespace(name) {
            Namespace::None => true,
            Namespace::Constant(inner) => constant_namespace(inner),
            Namespace::CBase | Namespace::Other => false,
        };
        if !splittable {
            continue;
        }
        // Either style leaves a definition alone when it is the whole body of another one.
        if sole_statement_of_definition(node) {
            continue;
        }

        if style == "nested" {
            check_nested(context, node, name, offenses);
        } else if let [only] = body_statements(node).as_slice()
            && matches!(only.kind_str(), "class" | "module")
        {
            let inner_style = match only.kind_str() == "class" {
                true => for_classes.as_str(),
                false => for_modules.as_str(),
            };
            let offense = context.offense(COMPACT_MSG, name.byte_range());
            offenses.push(
                match compact_correction(context, node, name, *only, inner_style) {
                    Some(edits) => offense.corrected_by_all(edits),
                    None => offense,
                },
            );
        }
    }
}

/// What stands to the left of the last `::` of a definition's name.
enum Namespace<'tree> {
    /// A plain name such as `Foo`.
    None,
    /// A leading `::`, as in `::Foo`.
    CBase,
    /// Another constant, as in `Foo::Bar`.
    Constant(Node<'tree>),
    /// Anything else -- `self::Foo`, `foo::Bar` -- which cannot be nested.
    Other,
}

fn namespace(name: Node<'_>) -> Namespace<'_> {
    if name.kind_str() != "scope_resolution" {
        return Namespace::None;
    }
    match name.field("scope") {
        None => Namespace::CBase,
        Some(scope) if matches!(scope.kind_str(), "constant" | "scope_resolution") => {
            Namespace::Constant(scope)
        }
        Some(_) => Namespace::Other,
    }
}

/// `const_namespace?`: every level of the name down to the root is a constant.
fn constant_namespace(node: Node<'_>) -> bool {
    match namespace(node) {
        Namespace::None | Namespace::CBase => true,
        Namespace::Constant(inner) => constant_namespace(inner),
        Namespace::Other => false,
    }
}

fn check_nested(
    context: &RuleContext<'_>,
    node: Node<'_>,
    name: Node<'_>,
    offenses: &mut Vec<Offense>,
) {
    if !context.source.node_text(name).contains("::") {
        return;
    }
    let offense = context.offense(NESTED_MSG, name.byte_range());
    offenses.push(match nested_correction(context, node, name) {
        Some(edits) => offense.corrected_by_all(edits),
        None => offense,
    });
}

/// Whether upstream would see the definition's parent as another definition, which happens exactly
/// when it is the only statement of that definition's body.
fn sole_statement_of_definition(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind_str() != "body_statement" {
        return false;
    }
    let statements = super::nodes::children(parent).len();
    statements == 1
        && parent
            .parent()
            .is_some_and(|grandparent| matches!(grandparent.kind_str(), "class" | "module"))
}

fn body_statements<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(body) = node.field("body") else {
        return Vec::new();
    };
    super::nodes::children(body)
}

/// `nest_definition`: turn `class Foo::Bar` into a `Foo` wrapper holding `class Bar`.
///
/// Three replacements -- the keyword, the `::` and the closing `end` -- with the body between them
/// left untouched, so the cops that correct inside it still can in the same pass.
fn nested_correction(
    context: &RuleContext<'_>,
    node: Node<'_>,
    name: Node<'_>,
) -> Option<Vec<Edit>> {
    let keyword = node.child(0)?;
    let closing = node.child(node.child_count().saturating_sub(1) as u32)?;
    if closing.kind_str() != "end" {
        return None;
    }
    let mut cursor = name.walk();
    let separator = name
        .children(&mut cursor)
        .find(|child| child.kind_str() == "::")?;

    let (_, column) = context.source.line_column(node.start_byte());
    let (_, end_column) = context.source.line_column(closing.start_byte());
    let width: usize = context
        .setting("IndentationWidth")
        .or_else(|| context.setting_of("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);

    let leading = leading_spaces(context, node.start_byte());
    let padding = " ".repeat(column - 1 + width) + &leading;
    let keyword_source = context.source.node_text(keyword);
    // The cop is declared `SafeAutoCorrect: false`: the wrapper's kind is only a guess.
    let safe = false;
    Some(vec![
        Edit {
            start: keyword.start_byte(),
            end: keyword.end_byte(),
            replacement: namespace_keyword(context, node, name).to_owned(),
            safe,
        },
        Edit {
            start: separator.start_byte(),
            end: separator.end_byte(),
            replacement: format!("\n{padding}{keyword_source} "),
            safe,
        },
        Edit {
            start: closing.start_byte(),
            end: closing.end_byte(),
            replacement: format!(
                "{}end\n{leading}end",
                drop_one_run(&padding, end_column - 1)
            ),
            safe,
        },
    ])
}

/// `compact_definition`: `class Foo` holding nothing but `class Bar` becomes `class Foo::Bar`.
///
/// Three edits, as upstream: the header up to the inner name, the inner `end`, and the body's
/// indentation. The last one is a run of per-line edits rather than a single replacement, so the
/// cops correcting inside the body still can in the same pass.
fn compact_correction(
    context: &RuleContext<'_>,
    node: Node<'_>,
    name: Node<'_>,
    inner: Node<'_>,
    inner_style: &str,
) -> Option<Vec<Edit>> {
    // Compacting produces a definition whose own style resolves to `nested`, which would make the
    // correction ping-pong between the two forms.
    if inner_style == "nested" {
        return None;
    }
    let keyword = node.child(0)?;
    let inner_name = inner.field("name")?;
    let inner_end = inner.child(inner.child_count().saturating_sub(1) as u32)?;
    if inner_end.kind_str() != "end" {
        return None;
    }
    // The cop is declared `SafeAutoCorrect: false`: compacting removes a definition of the
    // namespace, so the result needs it defined elsewhere.
    let safe = false;
    let compacted = format!(
        "{} {}::{}",
        inner.kind_str(),
        context.source.node_text(name),
        context.source.node_text(inner_name)
    );
    // `compact_replacement`: the comments `ast_with_comments` ties to the inner definition stand
    // inside the range being replaced, so they have to be written back above the compacted line.
    let comments = leading_comments(context, inner);
    let replacement = match comments.is_empty() {
        true => compacted,
        false => format!("{}\n{compacted}", comments.join("\n")),
    };
    let mut edits = vec![Edit {
        start: keyword.start_byte(),
        end: inner_name.end_byte(),
        replacement,
        safe,
    }];

    // `remove_end`: the inner `end`, the blanks in front of it and the line break after it.
    let same_line = inner_name.end_position().row == inner_end.start_position().row;
    let leading = leading_spaces(context, inner.start_byte());
    let removal_start = match same_line {
        true => inner_name.end_byte(),
        false => inner_end.start_byte().saturating_sub(leading.len()),
    };
    let adjustment = match context.source.text().as_bytes().get(removal_start) {
        Some(b';') => 0,
        _ => 1,
    };
    edits.push(Edit {
        start: removal_start,
        end: (inner_end.end_byte() + adjustment).min(context.source.text().len()),
        replacement: String::new(),
        safe,
    });

    edits.extend(unindent(context, node, inner, inner_name.end_byte(), safe));
    Some(edits)
}

/// `unindent`: the body moves left by the difference between the configured width and the
/// indentation the inner definition gave it.
fn unindent(
    context: &RuleContext<'_>,
    node: Node<'_>,
    inner: Node<'_>,
    header_end: usize,
    safe: bool,
) -> Vec<Edit> {
    let Some(last) = body_statements(inner).last().copied() else {
        return Vec::new();
    };
    let outer_indent = leading_spaces(context, node.start_byte()).chars().count();
    let inner_indent = leading_spaces(context, last.start_byte()).chars().count();
    if outer_indent == inner_indent {
        return Vec::new();
    }
    let width: usize = context
        .setting("IndentationWidth")
        .or_else(|| context.setting_of("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);
    let Some(delta) = inner_indent.checked_sub(width).filter(|delta| *delta > 0) else {
        return Vec::new();
    };
    // Only the lines the header edit does not already cover.
    let first = context.source.line_column(header_end).0 + 1;
    let last_line = context.source.line_column(node.end_byte()).0;
    (first..last_line)
        .filter_map(|line| {
            let text = context.source.line(line);
            let removable = text
                .chars()
                .take_while(|character| *character == ' ' || *character == '\t')
                .count()
                .min(delta);
            let start = context.source.line_start(line);
            (removable > 0).then(|| Edit {
                start,
                end: start + removable,
                replacement: String::new(),
                safe,
            })
        })
        .collect()
}

/// `padding.sub(' ' * column, '')`: one run of that many spaces comes back out of the padding.
fn drop_one_run(padding: &str, spaces: usize) -> String {
    let run = " ".repeat(spaces);
    match padding.find(&run) {
        Some(index) => {
            let mut folded = padding.to_owned();
            folded.replace_range(index..index + run.len(), "");
            folded
        }
        None => padding.to_owned(),
    }
}

/// `heuristic_namespace_keyword`: the statement written just before the definition decides whether
/// the namespace it opens is a class or a module.
fn namespace_keyword(context: &RuleContext<'_>, node: Node<'_>, name: Node<'_>) -> &'static str {
    let Some(scope) = name.field("scope") else {
        return "module";
    };
    let wanted = normalized(context.source.node_text(scope));
    let Some(sibling) = previous_statement(node) else {
        return "module";
    };
    let mut stack = vec![sibling];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "class"
            && current
                .field("name")
                .is_some_and(|other| normalized(context.source.node_text(other)) == wanted)
        {
            return "class";
        }
        let mut cursor = current.walk();
        stack.extend(current.named_children(&mut cursor));
    }
    "module"
}

/// The definition's left sibling in upstream's tree, which is the previous statement of the body or
/// file it sits in.
fn previous_statement<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parent = node.parent()?;
    if !STATEMENT_SEQUENCE_KINDS.contains(&parent.kind_str()) {
        return None;
    }
    let mut previous = None;
    let mut cursor = parent.walk();
    for child in parent.named_children(&mut cursor) {
        if child.id() == node.id() {
            return previous;
        }
        if super::nodes::is_child(child) {
            previous = Some(child);
        }
    }
    None
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// `ast_with_comments[node]` for a definition: the run of comment lines written directly above it,
/// in source order and without their indentation, which is what `Comment#text` gives.
fn leading_comments(context: &RuleContext<'_>, node: Node<'_>) -> Vec<String> {
    let opening = context.source.line_column(node.start_byte()).0;
    let mut collected = Vec::new();
    let mut probe = opening;
    while probe > 1 {
        let line = context.source.line(probe - 1);
        let text = line.trim();
        // A `#` opening a line is a comment only if the tokeniser agrees -- a heredoc body can
        // hold anything.
        let is_comment = text.starts_with('#')
            && context
                .comment_ranges()
                .iter()
                .any(|comment| context.source.line_column(comment.start).0 == probe - 1);
        if !is_comment {
            break;
        }
        collected.push(text.to_owned());
        probe -= 1;
    }
    collected.reverse();
    collected
}

/// `leading_spaces`: the indentation of the line the definition opens on.
fn leading_spaces(context: &RuleContext<'_>, offset: usize) -> String {
    let (line, _) = context.source.line_column(offset);
    context
        .source
        .line(line)
        .chars()
        .take_while(|character| character.is_whitespace() && *character != '\n')
        .collect()
}
