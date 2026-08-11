use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

const MSG: &str = "Avoid single-line method definitions.";

/// Endless definitions only exist from this version on.
const ENDLESS_SINCE: RubyVersion = RubyVersion::new(3, 0);

/// Clauses that make upstream wrap the whole body in one `rescue` or `ensure` node, which then
/// takes a single line break in front of it rather than one per statement.
const BODY_CLAUSE_KINDS: &[&str] = &["rescue", "ensure", "else"];

/// `BASIC_CONDITIONALS`, whose members upstream spells `if`, `while` and `until` whichever way they
/// are written.
const CONDITIONAL_KINDS: &[&str] = &[
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "conditional",
    "while",
    "until",
    "while_modifier",
    "until_modifier",
];

/// Bodies an endless definition cannot hold.
const UNSUPPORTED_BODY_KINDS: &[&str] = &["return", "break", "next"];

/// `ARITHMETIC_OPERATORS` and `COMPARISON_OPERATORS`: calls upstream leaves written as they are.
const OPERATORS_WITHOUT_PARENTHESES: &[&str] = &[
    "+", "-", "*", "/", "%", "**", "==", "===", "!=", "<=", ">=", ">", "<",
];

/// Binary operators upstream's parser builds a `send` for. The logical ones become `and` and `or`
/// nodes instead, which are not calls and so keep their source.
const BINARY_CALLS: &[&str] = &[
    "+", "-", "*", "/", "%", "**", "==", "===", "!=", "<=", ">=", ">", "<", "<=>", "<<", ">>", "&",
    "|", "^", "=~",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_empty: bool = context.setting("AllowIfMethodIsEmpty").unwrap_or(true);
    let width: usize = context
        .setting_of("Layout/IndentationWidth", "Width")
        .unwrap_or(2);
    let endless_allowed =
        context.target_ruby_version() >= ENDLESS_SINCE && !disallow_endless_method_style(context);

    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        if node.start_position().row != node.end_position().row {
            continue;
        }
        // An endless definition closes with its body rather than an `end`, and is already one line
        // by design.
        let Some(closing) = node.child(node.child_count().saturating_sub(1) as u32) else {
            continue;
        };
        if closing.kind() != "end" {
            continue;
        }
        let body = node.child_by_field_name("body").and_then(body_expression);
        if allow_empty && body.is_none() {
            continue;
        }

        let correction = match body.filter(|body| endless_allowed && correct_to_endless(node, body))
        {
            Some(body) => endless_correction(context, node, &body),
            None => multiline_correction(context, node, closing, width),
        };
        offenses.push(
            context
                .offense(MSG, node.byte_range())
                .corrected_by(correction),
        );
    }
}

/// `disallow_endless_method_style?`: a `Style/EndlessMethod` that is switched off, or set to reject
/// the form outright, keeps the correction on the multi-line path.
///
/// Upstream reads `Enabled` for truth rather than for `true`, so the `pending` a new cop carries
/// still counts as enabled.
fn disallow_endless_method_style(context: &RuleContext<'_>) -> bool {
    if context.setting_of::<bool>("Style/EndlessMethod", "Enabled") == Some(false) {
        return true;
    }
    context
        .setting_of::<String>("Style/EndlessMethod", "EnforcedStyle")
        .is_some_and(|style| style == "disallow")
}

/// The single expression upstream calls the definition's body, and the span it covers.
struct Body {
    kind: &'static str,
    range: std::ops::Range<usize>,
    /// The node itself, when the body is one expression rather than a sequence.
    node: Option<usize>,
}

fn body_expression(body: Node<'_>) -> Option<Body> {
    let statements = super::nodes::children(body);
    let first = statements.first()?;
    if statements
        .iter()
        .any(|child| BODY_CLAUSE_KINDS.contains(&child.kind()))
    {
        // A protected body is a single `rescue` or `ensure` node reaching to the last clause.
        let end = statements.last().map_or(first.end_byte(), Node::end_byte);
        return Some(Body {
            kind: "rescue",
            range: first.start_byte()..end,
            node: None,
        });
    }
    match statements.as_slice() {
        [only] => Some(Body {
            kind: only.kind(),
            range: only.byte_range(),
            node: Some(only.id()),
        }),
        several => Some(Body {
            kind: "begin",
            range: first.start_byte()..several.last()?.end_byte(),
            node: None,
        }),
    }
}

/// `correct_to_endless?`: the body fits on the right of an `=`.
fn correct_to_endless(node: Node<'_>, body: &Body) -> bool {
    if CONDITIONAL_KINDS.contains(&body.kind)
        || UNSUPPORTED_BODY_KINDS.contains(&body.kind)
        || matches!(body.kind, "begin" | "parenthesized_statements")
    {
        return false;
    }
    // `node.parent.assignment_method?`: a writer cannot be written endlessly.
    !node
        .child_by_field_name("name")
        .is_some_and(|name| name.kind() == "setter")
}

/// `correct_to_endless`: `def foo() = bar`.
fn endless_correction(context: &RuleContext<'_>, node: Node<'_>, body: &Body) -> Edit {
    let text = context.source.text();
    let receiver = node
        .child_by_field_name("object")
        .map(|object| format!("{}.", context.source.node_text(object)))
        .unwrap_or_default();
    let name = node
        .child_by_field_name("name")
        .map_or("", |name| context.source.node_text(name));
    let arguments = node.child_by_field_name("parameters").map_or_else(
        || "()".to_owned(),
        |list| match super::nodes::children(list).is_empty() {
            true => "()".to_owned(),
            false => context.source.node_text(list).to_owned(),
        },
    );
    let body_source = method_body_source(context, node, body).unwrap_or_else(|| {
        text[body.range.clone()]
            .trim_end_matches([';', ' '])
            .to_owned()
    });

    Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: format!("def {receiver}{name}{arguments} = {body_source}"),
        safe: true,
    }
}

/// `method_body_source`: a call carrying arguments is rewritten with parentheses, so that the `=`
/// of the endless form cannot swallow them.
fn method_body_source(context: &RuleContext<'_>, node: Node<'_>, body: &Body) -> Option<String> {
    let expression = body
        .node
        .and_then(|id| find_child(node.child_by_field_name("body")?, id))?;
    let (receiver, name, arguments) = match expression.kind() {
        "call" => {
            let list = expression.child_by_field_name("arguments")?;
            let arguments = super::nodes::children(list);
            if arguments.is_empty() {
                return None;
            }
            (
                expression.child_by_field_name("receiver"),
                context
                    .source
                    .node_text(expression.child_by_field_name("method")?),
                arguments,
            )
        }
        "binary" => {
            let operator = expression.child_by_field_name("operator")?;
            let name = context.source.node_text(operator);
            if !BINARY_CALLS.contains(&name) {
                return None;
            }
            (
                expression.child_by_field_name("left"),
                name,
                vec![expression.child_by_field_name("right")?],
            )
        }
        "element_reference" => {
            let mut arguments = super::nodes::children(expression);
            if expression.child_by_field_name("object").is_some() && !arguments.is_empty() {
                arguments.remove(0);
            }
            if arguments.is_empty() {
                return None;
            }
            (expression.child_by_field_name("object"), "[]", arguments)
        }
        _ => return None,
    };
    if OPERATORS_WITHOUT_PARENTHESES.contains(&name) {
        return None;
    }
    let joined = arguments
        .iter()
        .map(|argument| context.source.node_text(*argument))
        .collect::<Vec<_>>()
        .join(", ");
    Some(match receiver {
        Some(receiver) => format!("{}.{name}({joined})", context.source.node_text(receiver)),
        None => format!("{name}({joined})"),
    })
}

fn find_child<'tree>(body: Node<'tree>, id: usize) -> Option<Node<'tree>> {
    super::nodes::children(body)
        .into_iter()
        .find(|child| child.id() == id)
}

/// `correct_to_multiline`: a line break in front of every statement, one in front of the `end`, and
/// a trailing comment lifted to its own line above the definition.
fn multiline_correction(
    context: &RuleContext<'_>,
    node: Node<'_>,
    closing: Node<'_>,
    width: usize,
) -> Edit {
    let text = context.source.text();
    let (line, column) = context.source.line_column(node.start_byte());
    let indent = " ".repeat(column - 1);
    let inner = " ".repeat(column - 1 + width);
    let comment = trailing_comment(context, line, node.end_byte());

    let mut replacement = String::new();
    if let Some(comment) = &comment {
        replacement.push_str(&text[comment.clone()]);
        replacement.push('\n');
        replacement.push_str(&indent);
    }
    let mut cursor = node.start_byte();
    for offset in statement_starts(node) {
        replacement.push_str(&text[cursor..offset]);
        replacement.push('\n');
        replacement.push_str(&inner);
        cursor = offset;
    }
    replacement.push_str(&text[cursor..closing.start_byte()]);
    replacement.push('\n');
    replacement.push_str(&indent);
    replacement.push_str(&text[closing.start_byte()..node.end_byte()]);

    let end = match &comment {
        // The comment moves rather than being copied, so the blanks in front of it stay behind.
        Some(range) => {
            replacement.push_str(&text[node.end_byte()..range.start]);
            range.end
        }
        None => node.end_byte(),
    };
    Edit {
        start: node.start_byte(),
        end,
        replacement,
        safe: true,
    }
}

/// Where each line break goes, which is the start of every statement upstream's `each_part` yields.
fn statement_starts(node: Node<'_>) -> Vec<usize> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };
    let statements = super::nodes::children(body);
    let clause = statements
        .iter()
        .any(|child| BODY_CLAUSE_KINDS.contains(&child.kind()));
    match clause {
        // A protected body is one `rescue` node upstream, starting where its first statement does.
        true => statements
            .first()
            .map(|first| vec![first.start_byte()])
            .unwrap_or_default(),
        false => statements
            .iter()
            .map(|statement| statement.start_byte())
            .collect(),
    }
}

/// `comment_at_line`: the comment closing the definition's line, which the correction moves above it.
fn trailing_comment(
    context: &RuleContext<'_>,
    line: usize,
    after: usize,
) -> Option<std::ops::Range<usize>> {
    let range = context.source.line_range(line);
    context
        .comment_ranges()
        .iter()
        .rfind(|comment| comment.start >= range.start && comment.start < range.end)
        .filter(|comment| comment.start >= after)
        .cloned()
}
