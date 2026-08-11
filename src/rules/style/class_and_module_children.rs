use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

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
        let is_class = node.kind() == "class";
        let style = match is_class {
            true => for_classes.as_str(),
            false => for_modules.as_str(),
        };
        // A class with a superclass cannot be compacted, so only the nested style has anything to
        // say about it.
        if is_class && node.child_by_field_name("superclass").is_some() && style != "nested" {
            continue;
        }
        let Some(name) = node.child_by_field_name("name") else {
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
        } else if matches!(body_statements(node).as_slice(), [only] if matches!(only.kind(), "class" | "module"))
        {
            // The compact correction rewrites the body's indentation as well, which is not
            // expressible as the single edit an offense carries here, so the offense is reported
            // without one.
            offenses.push(context.offense(COMPACT_MSG, name.byte_range()));
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
    if name.kind() != "scope_resolution" {
        return Namespace::None;
    }
    match name.child_by_field_name("scope") {
        None => Namespace::CBase,
        Some(scope) if matches!(scope.kind(), "constant" | "scope_resolution") => {
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
        Some(edit) => offense.corrected_by(edit),
        None => offense,
    });
}

/// Whether upstream would see the definition's parent as another definition, which happens exactly
/// when it is the only statement of that definition's body.
fn sole_statement_of_definition(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "body_statement" {
        return false;
    }
    let statements = super::nodes::children(parent).len();
    statements == 1
        && parent
            .parent()
            .is_some_and(|grandparent| matches!(grandparent.kind(), "class" | "module"))
}

fn body_statements<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };
    super::nodes::children(body)
}

/// `nest_definition`: turn `class Foo::Bar` into a `Foo` wrapper holding `class Bar`.
///
/// Upstream writes three separate replacements -- the keyword, the `::` and the closing `end` --
/// which together span the whole definition, so they are emitted here as the one rewrite of it.
fn nested_correction(context: &RuleContext<'_>, node: Node<'_>, name: Node<'_>) -> Option<Edit> {
    let keyword = node.child(0)?;
    let closing = node.child(node.child_count().saturating_sub(1) as u32)?;
    if closing.kind() != "end" {
        return None;
    }
    let mut cursor = name.walk();
    let separator = name
        .children(&mut cursor)
        .find(|child| child.kind() == "::")?;

    let text = context.source.text();
    let (_, column) = context.source.line_column(node.start_byte());
    let (_, end_column) = context.source.line_column(closing.start_byte());
    let width: usize = context
        .setting("IndentationWidth")
        .or_else(|| context.setting_of("Layout/IndentationWidth", "Width"))
        .unwrap_or(2);

    let leading = leading_spaces(context, node.start_byte());
    let padding = " ".repeat(column - 1 + width) + &leading;
    let keyword_source = context.source.node_text(keyword);
    let replacement = format!(
        "{}{}\n{padding}{keyword_source} {}{}end\n{leading}end",
        namespace_keyword(context, node, name),
        &text[keyword.end_byte()..separator.start_byte()],
        &text[separator.end_byte()..closing.start_byte()],
        drop_one_run(&padding, end_column - 1),
    );
    Some(Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement,
        // The cop is declared `SafeAutoCorrect: false`: the wrapper's kind is only a guess.
        safe: false,
    })
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
    let Some(scope) = name.child_by_field_name("scope") else {
        return "module";
    };
    let wanted = normalized(context.source.node_text(scope));
    let Some(sibling) = previous_statement(node) else {
        return "module";
    };
    let mut stack = vec![sibling];
    while let Some(current) = stack.pop() {
        if current.kind() == "class"
            && current
                .child_by_field_name("name")
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
    if !STATEMENT_SEQUENCE_KINDS.contains(&parent.kind()) {
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
