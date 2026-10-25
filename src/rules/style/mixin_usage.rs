use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_iter;

/// 本家の `{kwbegin begin if def}` と、対応する AST ノードを持たない文の入れ物。
const TRANSPARENT: &[&str] = &[
    "begin",
    "program",
    "then",
    "else",
    "elsif",
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "method",
    "body_statement",
    "parenthesized_statements",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        if node.field("receiver").is_some() || node.field("block").is_some() {
            continue;
        }
        let Some(method) = node.field("method") else {
            continue;
        };
        let statement = context.source.node_text(method);
        if !matches!(statement, "include" | "extend" | "prepend") {
            continue;
        }
        // 本家の `const+` は、1 個以上の引数がすべて定数である場合だけに一致する。
        let arguments = node
            .field("arguments")
            .map(|arguments| super::nodes::children_in(arguments, context))
            .unwrap_or_default();
        if arguments.is_empty()
            || !arguments
                .iter()
                .all(|argument| matches!(argument.kind_str(), "constant" | "scope_resolution"))
        {
            continue;
        }
        if !in_top_level_scope(node, context) {
            continue;
        }
        offenses.push(context.offense(
            format!("`{statement}` is used at the top level. Use inside `class` or `module`."),
            node.byte_range(),
        ));
    }
}

/// `in_top_level_scope?` は本家の許可する祖先だけをたどり、根まで届くかを調べる。
fn in_top_level_scope(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    std::iter::successors(node.parent_of(context), |current| current.parent_of(context)).all(
        |ancestor| {
            // 本家の rescue/ensure は本体全体を包むが、文法は節を本体の兄弟に置く。
            if matches!(ancestor.kind_str(), "begin" | "body_statement")
                && named_children_iter(ancestor, context)
                    .any(|child| matches!(child.kind_str(), "rescue" | "ensure"))
            {
                return false;
            }
            TRANSPARENT.contains(&ancestor.kind_str())
        },
    )
}
