use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Consider replacing `case-when` with a hash lookup.";

/// `LITERAL_RECURSIVE_METHODS`: the operators whose operands decide whether the whole expression is
/// still a literal.
const RECURSIVE_METHODS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<", "*", "!", "<=>"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let minimum: usize = context
        .setting::<i64>("MinBranchesCount")
        .filter(|count| *count > 0)
        .unwrap_or(3) as usize;

    for node in context.nodes_of("case") {
        let branches = super::nodes::children_in(node, context);
        let whens: Vec<Node<'_>> = branches
            .iter()
            .copied()
            .filter(|child| child.kind_str() == "when")
            .collect();
        // `min_branches_count?`, and `nil?` for the `else` the pattern forbids.
        if whens.len() < minimum
            || branches.iter().any(|child| child.kind_str() == "else")
            || whens.len() + 1 != branches.len()
        {
            continue;
        }
        let mut conditions = Vec::new();
        let mut bodies = Vec::new();
        for when in &whens {
            let children = super::nodes::children_in(*when, context);
            let [pattern, body] = children.as_slice() else {
                conditions.clear();
                break;
            };
            if pattern.kind_str() != "pattern" || body.kind_str() != "then" {
                conditions.clear();
                break;
            }
            let patterns = super::nodes::children_in(*pattern, context);
            let statements = super::nodes::children_in(*body, context);
            let ([condition], [statement]) = (patterns.as_slice(), statements.as_slice()) else {
                conditions.clear();
                break;
            };
            conditions.push(*condition);
            bodies.push(*statement);
        }
        if conditions.len() != whens.len() || conditions.is_empty() {
            continue;
        }
        // `${str_type? sym_type?}` for every condition, and a literal body for every branch.
        if !conditions.iter().all(|condition| {
            matches!(
                upstream_type(context, *condition),
                Some("str") | Some("sym")
            )
        }) || !bodies
            .iter()
            .all(|body| recursive_basic_literal(context, *body))
        {
            continue;
        }
        if !same_type(context, &conditions) || !same_type(context, &bodies) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

fn same_type(context: &RuleContext<'_>, nodes: &[Node<'_>]) -> bool {
    let first = upstream_type(context, nodes[0]);
    nodes
        .iter()
        .all(|node| upstream_type(context, *node) == first)
}

/// `recursive_basic_literal?`.
fn recursive_basic_literal(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        "binary" | "unary" => {
            node.field("operator")
                .is_some_and(|operator| {
                    RECURSIVE_METHODS.contains(&context.source.node_text(operator))
                })
                && super::nodes::children_in(node, context)
                    .into_iter()
                    .all(|child| recursive_basic_literal(context, child))
        }
        "call" => {
            node.field("method")
                .is_some_and(|method| RECURSIVE_METHODS.contains(&context.source.node_text(method)))
                && super::nodes::children_in(node, context)
                    .into_iter()
                    .all(|child| recursive_basic_literal(context, child))
        }
        // `LITERAL_RECURSIVE_TYPES`.
        "array"
        | "hash"
        | "pair"
        | "range"
        | "regex"
        | "subshell"
        | "boolean"
        | "parenthesized_statements" => super::nodes::children_in(node, context)
            .into_iter()
            .all(|child| recursive_basic_literal(context, child)),
        "string" | "delimited_symbol" if is_interpolated(node) => super::nodes::children_in(node, context)
            .into_iter()
            .all(|child| recursive_basic_literal(context, child)),
        _ => matches!(upstream_type(context, node), Some(name) if BASIC_LITERALS.contains(&name)),
    }
}

/// `BASIC_LITERALS`, which is every literal that holds no other node.
const BASIC_LITERALS: &[&str] = &[
    "str", "int", "float", "sym", "true", "false", "nil", "complex", "rational",
];

/// The parser's node type, for the comparisons the cop makes between branches.
fn upstream_type(context: &RuleContext<'_>, node: Node<'_>) -> Option<&'static str> {
    let _ = context;
    Some(match node.kind_str() {
        // A literal that does not fit on one line is a `dstr` even without interpolation.
        "string" | "heredoc_beginning" => match is_interpolated(node) || is_multiline(node) {
            true => "dstr",
            false => "str",
        },
        "character" => "str",
        "delimited_symbol" => match is_interpolated(node) {
            true => "dsym",
            false => "sym",
        },
        "simple_symbol" | "hash_key_symbol" => "sym",
        "integer" => "int",
        "float" => "float",
        "rational" => "rational",
        "complex" => "complex",
        "true" => "true",
        "false" => "false",
        "nil" => "nil",
        "array" => "array",
        "hash" => "hash",
        "regex" => "regexp",
        "subshell" => "xstr",
        "range" => "range",
        _ => return None,
    })
}

fn is_interpolated(node: Node<'_>) -> bool {
    super::nodes::children(node)
        .iter()
        .any(|child| child.kind_str() == "interpolation")
}

fn is_multiline(node: Node<'_>) -> bool {
    node.start_position().row != node.end_position().row
}
