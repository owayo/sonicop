use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::send_node::top_level_constant;

use super::blocks::BLOCK_KINDS;
use super::literals::is_constant;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Method definitions must not be nested. Use `lambda` instead.";

/// The blocks that open a scope of their own, so that a definition inside one is not a definition
/// inside the method the block was written in.
const SCOPING_METHODS: &[&str] = &[
    "instance_eval",
    "class_eval",
    "module_eval",
    "instance_exec",
    "class_exec",
    "module_exec",
];

/// `class_constructor?`: the calls that build a class or a module from a block.
const CONSTRUCTORS: &[(&str, &str)] = &[
    ("Class", "new"),
    ("Module", "new"),
    ("Struct", "new"),
    ("Data", "define"),
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_default();
    let allowed_patterns: Vec<regex::Regex> = context
        .setting::<Vec<String>>("AllowedPatterns")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pattern| regex::Regex::new(&pattern).ok())
        .collect();
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        // `def obj.name` defines a method on something else, so it is only nested when the thing
        // it defines on cannot be named from outside -- which is what `self` alone is.
        if node.kind_str() == "singleton_method"
            && node
                .field("object")
                .is_some_and(|subject| is_allowed_subject(subject, context))
        {
            continue;
        }
        if !has_enclosing_definition(node) {
            continue;
        }
        if opens_its_own_scope(node, context, &allowed, &allowed_patterns) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}

/// `allowed_subject_type?`: 変数・定数・呼び出しは外側のメソッドに属さない対象を指す。
/// `self` はどれでもないので、メソッド内の `def self.name` は入れ子の定義になる。
fn is_allowed_subject(subject: Node<'_>, context: &RuleContext<'_>) -> bool {
    if subject.kind_str() == "identifier" {
        // grammar はこの位置のキーワードリテラルも identifier と呼ぶが、本家では変数ではない。
        return !matches!(context.source.node_text(subject), "nil" | "true" | "false");
    }
    if subject.kind_str() == "assignment" {
        // 属性・添字への単純代入は本家の send/csend。変数代入や複合代入は含めない。
        return subject
            .field("left")
            .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference"));
    }
    if subject.kind_str() == "binary" {
        return subject.field("operator").is_some_and(|operator| {
            let operator = context.source.node_text(operator);
            // 論理式と match_with_lvasgn は、本家では演算子の send にならない。
            !matches!(operator, "&&" | "||" | "and" | "or")
                && !(operator == "=~"
                    && subject
                        .field("left")
                        .is_some_and(|left| context.static_regexp(left)))
        });
    }
    if subject.kind_str() == "unary" {
        return subject.field("operator").is_some_and(|operator| {
            let operator = context.source.node_text(operator);
            // 符号を数値へ畳み込むのは直下が数値のときだけ。--1.5 の外側は send。
            matches!(operator, "!" | "~" | "+" | "-" | "not")
                && !(matches!(operator, "+" | "-")
                    && subject.field("operand").is_some_and(|operand| {
                        matches!(operand.kind_str(), "integer" | "float" | "rational" | "complex")
                    }))
        });
    }
    if matches!(subject.kind_str(), "call" | "element_reference") {
        // call や添字でもブロック付きなら block、super(...) なら super になる。
        return subject.field("block").is_none()
            && !(subject.field("receiver").is_none()
                && subject
                    .field("method")
                    .is_some_and(|method| context.source.node_text(method) == "super"));
    }
    matches!(
        subject.kind_str(),
        "instance_variable" | "class_variable" | "global_variable"
    ) || is_constant(subject, context)
}

fn has_enclosing_definition(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "method" | "singleton_method") {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

/// `each_ancestor(:any_block, :sclass).any? { |a| scoping_method_call?(a) }`.
fn opens_its_own_scope(
    node: Node<'_>,
    context: &RuleContext<'_>,
    allowed: &[String],
    allowed_patterns: &[regex::Regex],
) -> bool {
    let mut current = node.parent_of(context);
    while let Some(ancestor) = current {
        if ancestor.kind_str() == "singleton_class" {
            return true;
        }
        if BLOCK_KINDS.contains(&ancestor.kind_str())
            && is_scoping_block(ancestor, context, allowed, allowed_patterns)
        {
            return true;
        }
        current = ancestor.parent_of(context);
    }
    false
}

fn is_scoping_block(
    block: Node<'_>,
    context: &RuleContext<'_>,
    allowed: &[String],
    allowed_patterns: &[regex::Regex],
) -> bool {
    let Some(call) = block
        .parent_of(context)
        .filter(|call| call.kind_str() == "call")
    else {
        return false;
    };
    let Some(selector) = call.field("method") else {
        return false;
    };
    let name = context.source.node_text(selector);
    if SCOPING_METHODS.contains(&name)
        || allowed.iter().any(|method| method == name)
        || allowed_patterns
            .iter()
            .any(|pattern| pattern.is_match(name))
    {
        return true;
    }
    call.field("receiver").is_some_and(|receiver| {
        CONSTRUCTORS.iter().any(|(constant, method)| {
            name == *method && top_level_constant(receiver, constant, context)
        })
    })
}
