use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Useless trailing comma present in block arguments.";

/// A `|a, b,|` whose last comma says nothing, because more than one parameter is already written.
///
/// A single parameter is different: `|a,|` destructures the first element, so the comma is load
/// bearing and upstream leaves it alone.
///
/// A `->(a, b,)` is exempt, and its parameters are a `lambda_parameters` node here rather than the
/// `block_parameters` this walks.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("block_parameters") {
        if declared_count(node) <= 1 {
            continue;
        }
        let Some(comma) = trailing_comma(node, context) else {
            continue;
        };
        offenses.push(context.offense(MSG, comma..comma + 1).corrected_by(Edit {
            start: comma,
            end: comma + 1,
            replacement: String::new(),
            safe: true,
        }));
    }
}

/// `arg_count`: `each_descendant(:arg, :optarg, :kwoptarg)`, so a destructured parameter counts its
/// own parts and `*rest`, `&block` and a bare keyword do not count at all.
fn declared_count(list: Node<'_>) -> usize {
    let written: usize = super::parameters::parameters(list)
        .iter()
        .map(|parameter| match parameter.kind {
            "identifier" | "optional_parameter" => 1,
            "keyword_parameter" if parameter.value.is_some() => 1,
            _ => 0,
        })
        .sum();
    let nested: usize = super::nodes::children(list)
        .iter()
        .filter(|child| child.kind_str() == "destructured_parameter")
        .map(|child| declared_count(*child))
        .sum();
    written + nested
}

/// The offset of the comma that closes the parameter list, when that is the last thing written in
/// it.
///
/// Upstream reads the tokens between the two pipes and asks whether the last is a comma, so a
/// comment written after it means the comma is not last.
fn trailing_comma(list: Node<'_>, context: &RuleContext<'_>) -> Option<usize> {
    let text = context.source.text();
    let inner = text.get(list.start_byte()..list.end_byte())?;
    let inner = inner.strip_prefix('|')?.strip_suffix('|')?;
    let trimmed = inner.trim_end();
    if !trimmed.ends_with(',') {
        return None;
    }
    Some(list.start_byte() + 1 + trimmed.len() - 1)
}
