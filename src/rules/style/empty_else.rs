use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Redundant `else`-clause.";

/// The conditionals `on_normal_if_unless` and `on_case` reach. A modifier form and a ternary have
/// no `else` to begin with, and `case ... in` is a `case_match` upstream, which neither handler
/// is registered for.
const CONDITIONALS: &[&str] = &["if", "unless", "elsif", "case"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "both".to_owned());
    let (empty_style, nil_style) = (
        matches!(style.as_str(), "empty" | "both"),
        matches!(style.as_str(), "nil" | "both"),
    );
    let allow_comments = context.setting::<bool>("AllowComments").unwrap_or(false);
    // `autocorrect_forbidden?`: `Style/MissingElse` asks for the very branch this cop removes, so
    // while it is on for this node's type the removal is withheld and the offense stays as a
    // report.
    let missing_else = context
        .setting_of::<bool>("Style/MissingElse", "Enabled")
        .unwrap_or(false)
        .then(|| {
            context
                .setting_of::<String>("Style/MissingElse", "EnforcedStyle")
                .unwrap_or_else(|| "both".to_owned())
        });

    for node in context.nodes_of_any(CONDITIONALS) {
        let Some(clause) = else_clause(node) else {
            continue;
        };
        let Some(keyword) = clause.child(0) else {
            continue;
        };
        let body = super::nodes::children(clause);
        let reportable = match body.as_slice() {
            [] => empty_style,
            [only] => nil_style && only.kind_str() == "nil",
            _ => false,
        };
        if !reportable {
            continue;
        }
        if allow_comments && comment_in_else(context, node) {
            continue;
        }
        let offense = context.offense(MSG, keyword.byte_range());
        let forbidden = missing_else.as_deref().is_some_and(|configured| {
            configured == "both" || configured == upstream_type(node.kind_str())
        });
        offenses.push(match forbidden || comment_in_else(context, node) {
            true => offense,
            false => match closing_keyword(node) {
                Some(end) => offense.corrected_by(Edit {
                    start: keyword.start_byte(),
                    end: end.start_byte(),
                    replacement: String::new(),
                    safe: true,
                }),
                None => offense,
            },
        });
    }
}

/// The `else` clause of a conditional, and only that: an `elsif` stands where the `else` would be
/// but carries a branch of its own, which is never the empty one this cop reports.
fn else_clause<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let clause = match node.kind_str() {
        "case" => super::nodes::children(node)
            .into_iter()
            .find(|child| child.kind_str() == "else")?,
        _ => node.field("alternative")?,
    };
    (clause.kind_str() == "else").then_some(clause)
}

fn upstream_type(kind: &str) -> &str {
    match kind {
        "case" => "case",
        _ => "if",
    }
}

/// `base_node(node).loc.end`: an `elsif` has no `end` of its own, so the removal runs to the one
/// closing the conditional it belongs to.
fn closing_keyword<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node;
    while current.kind_str() == "elsif" {
        current = current.parent()?;
    }
    let last = current.child(current.child_count().checked_sub(1)? as u32)?;
    (last.kind_str() == "end").then_some(last)
}

/// `comment_in_else?`: a comment anywhere from the `else` -- or from the first `elsif`, for a
/// branch inside a chain -- to the end of the conditional keeps the clause as it is.
fn comment_in_else(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let mut outermost = node;
    while outermost.kind_str() == "elsif" {
        let Some(parent) = outermost.parent() else {
            break;
        };
        outermost = parent;
    }
    let Some(clause) = outermost
        .field("alternative")
        .or_else(|| else_clause(outermost))
    else {
        return false;
    };
    let lines = clause.start_position().row..=outermost.end_position().row;
    context.comment_ranges().iter().any(|comment| {
        lines.contains(
            &context
                .source
                .line_column(comment.start)
                .0
                .saturating_sub(1),
        )
    })
}
