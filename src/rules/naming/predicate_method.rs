use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::push_named_children;
use crate::rules::regex_cache;
use crate::rules::send_node::named_children;

const MSG_PREDICATE: &str = "Predicate method names should end with `?`.";
const MSG_NON_PREDICATE: &str = "Non-predicate method names should not end with `?`.";

/// `OPERATOR_METHODS`: a method named after an operator is exempt whatever it returns.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

/// `COMPARISON_OPERATORS`.
const COMPARISON_METHODS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

/// The operators upstream builds an `and` or an `or` node for rather than a `send`. The grammar
/// writes them as ordinary binary operators, so they have to be told apart by the token.
const AND_OR_OPERATORS: &[&str] = &["&&", "||", "and", "or"];

/// `Node::LITERALS`, as the grammar spells them. `regopt` never stands alone as a value.
const LITERAL_KINDS: &[&str] = &[
    "string",
    "chained_string",
    "bare_string",
    "heredoc_beginning",
    "subshell",
    "character",
    "simple_symbol",
    "delimited_symbol",
    "hash_key_symbol",
    "integer",
    "float",
    "rational",
    "complex",
    "array",
    "string_array",
    "symbol_array",
    "hash",
    "regex",
    "range",
    "true",
    "false",
    "nil",
];

const NUMERIC_KINDS: &[&str] = &["integer", "float", "rational", "complex"];

/// Node kinds whose named children upstream folds into one `begin` node when there is more than one.
const CONTAINERS: &[&str] = &[
    "body_statement",
    "then",
    "else",
    "do",
    "block_body",
    "parenthesized_statements",
];

/// Clause kinds a body list holds that are not statements of it. A list carrying one of them is a
/// `rescue` or `ensure` node upstream rather than a `begin`.
const BODY_CLAUSES: &[&str] = &["rescue", "ensure", "else"];

/// One of the values a method body can hand back.
#[derive(Clone, Copy)]
enum Value<'tree> {
    Node(Node<'tree>),
    /// A node upstream synthesized rather than parsed: the `s(:nil)` a bare `return` stands for, the
    /// `s(:array)` a `return a, b` does, or the `hash` a braceless run of pairs stands for. All
    /// three are literal types and none of them is a boolean, which is all this cop asks of them.
    Synthesized,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed_methods: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    let allowed_patterns: Vec<String> = context.setting("AllowedPatterns").unwrap_or_default();
    let wayward: Vec<String> = context.setting("WaywardPredicates").unwrap_or_default();
    let allow_bang: bool = context.setting("AllowBangMethods").unwrap_or(false);
    let conservative = context
        .setting::<String>("Mode")
        .unwrap_or_else(|| "conservative".to_owned())
        == "conservative";
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        let Some(name_node) = node.field("name") else {
            continue;
        };
        let name = context.source.node_text(name_node);
        // `allowed?`
        if name == "initialize"
            || allowed_methods.iter().any(|allowed| allowed == name)
            || allowed_patterns.iter().any(|pattern| {
                regex_cache::compiled(pattern).is_some_and(|regex| regex.is_match(name))
            })
            || (allow_bang && name.ends_with('!'))
            || OPERATOR_METHODS.contains(&name)
        {
            continue;
        }
        let Some(body) = node.field("body") else {
            continue;
        };
        let values = return_values(body, context);
        // `acceptable?`: a value the cop cannot read leaves the name alone in conservative mode.
        if conservative
            && values.iter().any(|value| {
                is_super(*value, context)
                    || (is_call(*value, context) && !returns_boolean(*value, &wayward, context))
            })
        {
            continue;
        }
        let predicate = name.ends_with('?');
        if predicate && potential_non_predicate(&values, conservative, &wayward, context) {
            offenses.push(context.offense(MSG_NON_PREDICATE, name_node.byte_range()));
        } else if !predicate && all_boolean(&values, &wayward, context) {
            offenses.push(context.offense(MSG_PREDICATE, name_node.byte_range()));
        }
    }
}

/// `return_values`: every value the body can hand back, with conditionals and `and`/`or` broken down
/// into the values their branches hand back.
fn return_values<'tree>(container: Node<'tree>, context: &RuleContext<'_>) -> Vec<Value<'tree>> {
    let body = upstream_body(container);
    let mut values = Vec::new();
    // `Set.new(node.begin_type? ? [] : [extract_return_value(node)])`
    if let Body::One(node) = body {
        values.push(extract_return_value(node, context));
    }
    // `node.each_descendant(:return)`
    let mut stack = vec![container];
    while let Some(current) = stack.pop() {
        if current.kind_str() == "return" {
            values.push(extract_return_value(current, context));
        }
        push_named_children(current, &mut stack);
    }
    values.push(last_value_of(body, context));
    process(values, context)
}

/// `process_return_values`.
fn process<'tree>(values: Vec<Value<'tree>>, context: &RuleContext<'_>) -> Vec<Value<'tree>> {
    let mut out = Vec::new();
    for value in values {
        match value {
            Value::Node(node) if is_conditional(node) => {
                out.extend(process(conditional_branches(node, context), context));
            }
            Value::Node(node) if is_and_or(node, context) => {
                out.extend(process(and_or_clauses(node, context), context));
            }
            other => out.push(other),
        }
    }
    out
}

/// `and_or?`: the node upstream builds as an `and` or an `or`.
fn is_and_or(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && operator(node, context).is_some_and(|text| AND_OR_OPERATORS.contains(&text))
}

/// `extract_and_or_clauses`.
fn and_or_clauses<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Vec<Value<'tree>> {
    let mut out = Vec::new();
    for side in [node.field("left"), node.field("right")] {
        match side {
            Some(side) if is_and_or(side, context) => out.extend(and_or_clauses(side, context)),
            Some(side) => out.push(Value::Node(side)),
            None => {}
        }
    }
    out
}

/// `extract_conditional_branches`.
fn conditional_branches<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Vec<Value<'tree>> {
    if matches!(
        node.kind_str(),
        "while" | "until" | "while_modifier" | "until_modifier"
    ) {
        return match node.field("body") {
            Some(body) => vec![last_value(body, context)],
            None => vec![Value::Synthesized],
        };
    }
    let (bodies, has_else) = branch_bodies(node);
    let mut branches: Vec<Value<'tree>> = bodies
        .into_iter()
        .map(|body| match body {
            Some(body) => last_value(body, context),
            None => Value::Synthesized,
        })
        .collect();
    if !has_else {
        branches.push(Value::Synthesized);
    }
    branches
}

/// `node.branches` for a conditional that is not a loop, and whether it has an `else` at all.
///
/// An `elsif` chain is one nested `if` per step upstream, and `branches` walks into it -- but the
/// `s(:nil)` for a missing final `else` is only added when the *outermost* conditional has no
/// alternative at all, which an `elsif` counts as.
fn branch_bodies<'tree>(node: Node<'tree>) -> (Vec<Option<Node<'tree>>>, bool) {
    if matches!(node.kind_str(), "case" | "case_match") {
        return case_bodies(node);
    }
    let mut bodies = vec![branch_body(
        node.field("consequence").or(node.field("body")),
    )];
    let mut current = node;
    let mut has_else = false;
    while let Some(alternative) = current.field("alternative") {
        has_else = true;
        if alternative.kind_str() == "elsif" {
            bodies.push(branch_body(alternative.field("consequence")));
            current = alternative;
            continue;
        }
        // An `else` written empty is no branch at all upstream.
        bodies.push(branch_body(Some(alternative)));
        has_else = branch_body(Some(alternative)).is_some();
        break;
    }
    (bodies, has_else)
}

/// `when_branches.map(&:body)` / `in_pattern_branches.map(&:body)`, plus the `else` body.
fn case_bodies<'tree>(node: Node<'tree>) -> (Vec<Option<Node<'tree>>>, bool) {
    let mut bodies = Vec::new();
    let mut has_else = false;
    for child in named_children(node) {
        match child.kind_str() {
            "when" | "in_clause" => bodies.push(branch_body(child.field("body"))),
            "else" => {
                bodies.push(branch_body(Some(child)));
                has_else = branch_body(Some(child)).is_some();
            }
            _ => {}
        }
    }
    (bodies, has_else)
}

/// A branch body, or `None` when the branch holds no statement and so is `nil` upstream.
fn branch_body<'tree>(body: Option<Node<'tree>>) -> Option<Node<'tree>> {
    let body = body?;
    match CONTAINERS.contains(&body.kind_str()) && matches!(upstream_body(body), Body::Missing) {
        true => None,
        false => Some(body),
    }
}

/// Upstream's body of a statement list.
enum Body<'tree> {
    Missing,
    One(Node<'tree>),
    Begin(Vec<Node<'tree>>),
}

fn upstream_body<'tree>(container: Node<'tree>) -> Body<'tree> {
    // An endless definition's body is the expression itself rather than a statement list, and so is
    // a `then` written with a single statement on the same line.
    if !CONTAINERS.contains(&container.kind_str()) {
        return Body::One(container);
    }
    let children: Vec<Node<'tree>> = named_children(container)
        .into_iter()
        .filter(|child| child.kind_str() != "comment")
        .collect();
    // A list carrying a `rescue` or `ensure` clause is one of those nodes upstream, which this cop
    // reads no further into.
    if children
        .iter()
        .any(|child| BODY_CLAUSES.contains(&child.kind_str()))
    {
        return Body::One(container);
    }
    match children.as_slice() {
        [] => Body::Missing,
        [only] if only.kind_str() == "parenthesized_statements" => Body::Begin(
            named_children(*only)
                .into_iter()
                .filter(|child| child.kind_str() != "comment")
                .collect(),
        ),
        [only] => Body::One(*only),
        several => Body::Begin(several.to_vec()),
    }
}

/// `last_value` for a body already broken down.
fn last_value_of<'tree>(body: Body<'tree>, context: &RuleContext<'_>) -> Value<'tree> {
    match body {
        Body::Missing => Value::Synthesized,
        Body::One(node) => extract_return_value(node, context),
        Body::Begin(statements) => match statements.last() {
            Some(node) => extract_return_value(*node, context),
            None => Value::Synthesized,
        },
    }
}

/// `last_value`.
fn last_value<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Value<'tree> {
    last_value_of(upstream_body(node), context)
}

/// `extract_return_value`.
fn extract_return_value<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Value<'tree> {
    if node.kind_str() != "return" {
        return Value::Node(node);
    }
    let Some(list) = named_children(node).into_iter().next() else {
        return Value::Synthesized;
    };
    let arguments: Vec<Node<'tree>> = fold_pairs(
        named_children(list)
            .into_iter()
            .filter(|child| child.kind_str() != "comment")
            .collect(),
    );
    let _ = context;
    match arguments.as_slice() {
        [] => Value::Synthesized,
        [only] if matches!(only.kind_str(), "pair" | "hash_splat_argument") => {
            Value::Synthesized
        }
        [only] => Value::Node(*only),
        _ => Value::Synthesized,
    }
}

/// A trailing run of `key: value` arguments is one `hash` upstream.
fn fold_pairs<'tree>(children: Vec<Node<'tree>>) -> Vec<Node<'tree>> {
    let mut folded = Vec::with_capacity(children.len());
    let mut in_hash = false;
    for child in children {
        match matches!(child.kind_str(), "pair" | "hash_splat_argument") {
            true => {
                if !in_hash {
                    folded.push(child);
                }
                in_hash = true;
            }
            false => {
                folded.push(child);
                in_hash = false;
            }
        }
    }
    folded
}

/// `all_return_values_boolean?`.
fn all_boolean(values: &[Value<'_>], wayward: &[String], context: &RuleContext<'_>) -> bool {
    let values: Vec<Value<'_>> = values
        .iter()
        .copied()
        .filter(|value| !is_super(*value, context))
        .collect();
    !values.is_empty()
        && values
            .iter()
            .all(|value| boolean_return(*value, wayward, context))
}

/// `potential_non_predicate?`.
fn potential_non_predicate(
    values: &[Value<'_>],
    conservative: bool,
    wayward: &[String],
    context: &RuleContext<'_>,
) -> bool {
    if conservative
        && values
            .iter()
            .any(|value| boolean_return(*value, wayward, context))
    {
        return false;
    }
    values
        .iter()
        .any(|value| is_literal(*value) && !is_boolean(*value))
}

/// `boolean_return?`.
fn boolean_return(value: Value<'_>, wayward: &[String], context: &RuleContext<'_>) -> bool {
    is_boolean(value) || returns_boolean(value, wayward, context)
}

/// `method_returning_boolean?`.
fn returns_boolean(value: Value<'_>, wayward: &[String], context: &RuleContext<'_>) -> bool {
    let Some(name) = method_name(value, context) else {
        return false;
    };
    if wayward.iter().any(|entry| entry == name) {
        return false;
    }
    COMPARISON_METHODS.contains(&name) || name.ends_with('?') || name == "!"
}

/// `value.call_type?` together with the name of the method it dispatches, or `None` when the value is
/// not a call at all.
fn method_name<'a>(value: Value<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let Value::Node(node) = value else {
        return None;
    };
    // A call carrying a block is the `block` node wrapped around it upstream, which is no `send`.
    if node.field("block").is_some() {
        return None;
    }
    match node.kind_str() {
        "call" => {
            let method = node.field("method")?;
            // `super(1)` is a `super` node upstream, not a call of a method named `super`.
            match method.kind_str() == "super" {
                true => None,
                false => Some(context.source.node_text(method)),
            }
        }
        // `a[1]` is `(send a :[] 1)`.
        "element_reference" => Some("[]"),
        // An operator is a `send` of that operator upstream, except for `&&` / `||` / `and` / `or`,
        // which build an `and` or an `or` node instead.
        "binary" => operator(node, context).filter(|text| !AND_OR_OPERATORS.contains(text)),
        "unary" => match operator(node, context)? {
            "!" | "not" => Some("!"),
            // A signed number is one numeric literal upstream rather than a call, and `defined?` is
            // a node type of its own.
            "-" | "+" if is_signed_number(node) => None,
            "defined?" => None,
            "-" => Some("-@"),
            "+" => Some("+@"),
            other => Some(other),
        },
        // A bare name the parser could not resolve to a local variable is a receiverless call.
        "identifier" => match context.variable_analysis().is_variable_reference(node) {
            true => None,
            false => Some(context.source.node_text(node)),
        },
        _ => None,
    }
}

/// `-1` and `+1`, which upstream's parser folds into a single numeric literal.
fn is_signed_number(node: Node<'_>) -> bool {
    node.field("operand")
        .is_some_and(|operand| NUMERIC_KINDS.contains(&operand.kind_str()))
}

fn is_call(value: Value<'_>, context: &RuleContext<'_>) -> bool {
    method_name(value, context).is_some()
}

/// `value.type?(:super, :zsuper)`.
fn is_super(value: Value<'_>, context: &RuleContext<'_>) -> bool {
    let Value::Node(node) = value else {
        return false;
    };
    let _ = context;
    match node.kind_str() {
        "super" => true,
        "call" => node
            .field("method")
            .is_some_and(|method| method.kind_str() == "super"),
        _ => false,
    }
}

/// `value.literal?`.
fn is_literal(value: Value<'_>) -> bool {
    match value {
        // `s(:nil)` and `s(:array)` are both literal types; so is the `hash` a braceless run of
        // pairs stands for.
        Value::Synthesized => true,
        Value::Node(node) => {
            LITERAL_KINDS.contains(&node.kind_str())
                // A signed number is one numeric literal to upstream's parser rather than a call.
                || (node.kind_str() == "unary" && is_signed_number(node))
        }
    }
}

/// `value.boolean_type?`.
fn is_boolean(value: Value<'_>) -> bool {
    match value {
        Value::Synthesized => false,
        Value::Node(node) => matches!(node.kind_str(), "true" | "false"),
    }
}

/// `value.conditional?`, which covers neither a post-condition loop nor a bare `begin`.
fn is_conditional(node: Node<'_>) -> bool {
    match node.kind_str() {
        "if" | "unless" | "conditional" | "if_modifier" | "unless_modifier" | "case"
        | "case_match" | "while" | "until" => true,
        // `begin ... end while x` is a `while_post`, which upstream does not count.
        "while_modifier" | "until_modifier" => node
            .field("body")
            .is_some_and(|body| body.kind_str() != "begin"),
        _ => false,
    }
}

/// The operator a node was written with, which the grammar leaves as an anonymous token.
fn operator<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| !child.is_named())
        .map(|child| context.source.node_text(child))
}
