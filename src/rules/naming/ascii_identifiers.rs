use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const IDENTIFIER_MSG: &str = "Use only ascii symbols in identifiers.";
const CONSTANT_MSG: &str = "Use only ascii symbols in constants.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ascii_constants: bool = context.setting("AsciiConstants").unwrap_or(true);

    for node in context.nodes_of_any(&["identifier", "constant"]) {
        let is_constant = node.kind_str() == "constant";
        if is_constant && !ascii_constants {
            continue;
        }
        // RuboCop walks lexer tokens and only inspects `tIDENTIFIER` and `tCONSTANT`. Instance,
        // class and global variables, symbols and labels carry their own token types, so they stay
        // unreported even when they hold non-ascii characters. tree-sitter has no such token
        // distinction: it reuses `identifier` for keyword-argument labels, which are `tLABEL`
        // upstream and must be skipped here to keep the two in step.
        if is_keyword_argument_label(node, context) {
            continue;
        }

        let text = context.source.node_text(node);
        let Some(offset) = text.find(|character: char| !character.is_ascii()) else {
            continue;
        };
        // Only the first run of non-ascii characters is reported, matching `first_offense_range`.
        let tail = &text[offset..];
        let length = tail
            .find(|character: char| character.is_ascii())
            .unwrap_or(tail.len());

        let start = node.byte_range().start + offset;
        let message = if is_constant {
            CONSTANT_MSG
        } else {
            IDENTIFIER_MSG
        };
        offenses.push(context.offense(message, start..start + length));
    }
}

fn is_keyword_argument_label(node: tree_sitter::Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = context.parent(node) else {
        return false;
    };
    matches!(parent.kind_str(), "keyword_parameter" | "pair")
        && parent
            .field("name")
            .or_else(|| parent.field("key"))
            == Some(node)
}
