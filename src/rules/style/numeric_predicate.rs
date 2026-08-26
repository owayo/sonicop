use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// The version `positive?` and `negative?` were added in.
const PREDICATES_SINCE: crate::ruby_version::RubyVersion =
    crate::ruby_version::RubyVersion::new(2, 3);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let predicate_style = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "predicate");
    let allowed = Allowed::new(context);

    for node in context.nodes_of_any(&["binary", "call"]) {
        let found = match predicate_style {
            true => comparison(context, node),
            false => predicate(context, node),
        };
        let Some((numeric, operator)) = found else {
            continue;
        };
        let selector = selector_name(context, node);
        if allowed.matches(selector.as_deref())
            || (allowed.is_set() && ancestor_is_allowed(context, node, &allowed))
        {
            continue;
        }
        // `replacement_supported?`: `positive?` and `negative?` arrived in 2.3, so a comparison is
        // left alone below that -- `zero?` has always been there.
        if matches!(operator, ">" | "<") && context.target_ruby_version() < PREDICATES_SINCE {
            continue;
        }

        let replacement = match predicate_style {
            true => format!(
                "{}.{}",
                parenthesized_source(context, numeric),
                predicate_name(operator)
            ),
            false => match negated(context, node) {
                true => format!("({} {} 0)", context.source.node_text(numeric), operator),
                false => format!("{} {} 0", context.source.node_text(numeric), operator),
            },
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{replacement}` instead of `{}`.",
                        context.source.node_text(node)
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: false,
                }),
        );
    }
}

/// `comparison` and `inverted_comparison`: the operand that is not the literal zero, and the
/// operator as it reads from that operand's side.
fn comparison<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str)> {
    let (left, right, operator) = operands(context, node)?;
    let operator = ["==", ">", "<"]
        .into_iter()
        .find(|candidate| *candidate == operator)?;
    if is_zero(context, right) && left.kind_str() != "global_variable" {
        return Some((left, operator));
    }
    if is_zero(context, left) && right.kind_str() != "global_variable" {
        // `invert`: read from the other side, `>` and `<` swap.
        let inverted = match operator {
            ">" => "<",
            "<" => ">",
            other => other,
        };
        return Some((right, inverted));
    }
    None
}

/// `predicate`: `zero?`, `positive?` or `negative?` called on something.
fn predicate<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str)> {
    // Upstream dispatches `on_send` but does not alias it to `on_csend`: a predicate reached
    // through `&.` is deliberately outside this cop even though the grammar calls both forms a
    // `call`.
    if node.kind_str() != "call"
        || !send_node::is_plain_send(node, context)
        || node.field("arguments").is_some()
    {
        return None;
    }
    let receiver = node.field("receiver")?;
    let method = node.field("method")?;
    let operator = match context.source.node_text(method) {
        "zero?" => "==",
        "positive?" => ">",
        "negative?" => "<",
        _ => return None,
    };
    Some((receiver, operator))
}

fn predicate_name(operator: &str) -> &'static str {
    match operator {
        ">" => "positive?",
        "<" => "negative?",
        _ => "zero?",
    }
}

/// The two operands and the selector of a binary send, whichever way it was spelled.
fn operands<'a, 'tree>(
    context: &'a RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>, &'a str)> {
    match node.kind_str() {
        "binary" => {
            let left = node.field("left")?;
            if super::nodes::is_bare_jump(left) {
                return None;
            }
            let operator = node.field("operator")?;
            Some((
                left,
                node.field("right")?,
                context.source.node_text(operator),
            ))
        }
        _ => {
            // `comparison` is a `(send …)` pattern, and `&.` is a `csend`: the grammar spells both
            // a `call`, so the operator reached through a safe navigation has to be excluded here.
            if !send_node::is_plain_send(node, context) {
                return None;
            }
            let method = node.field("method")?;
            let arguments = super::nodes::children(node.field("arguments")?);
            match arguments.as_slice() {
                [only] => Some((
                    node.field("receiver")?,
                    *only,
                    context.source.node_text(method),
                )),
                _ => None,
            }
        }
    }
}

/// `(int 0)`: the literal zero, however it was written.
fn is_zero(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let node = match node.kind_str() {
        "unary" => match node.field("operand") {
            Some(operand) if operand.kind_str() == "integer" => operand,
            _ => return false,
        },
        _ => node,
    };
    if node.kind_str() != "integer" {
        return false;
    }
    let digits = context.source.node_text(node).replace('_', "");
    let digits = ["0x", "0X", "0b", "0B", "0o", "0O", "0d", "0D"]
        .into_iter()
        .find_map(|prefix| digits.strip_prefix(prefix).map(str::to_owned))
        .unwrap_or(digits);
    !digits.is_empty() && digits.chars().all(|digit| digit == '0')
}

/// `parenthesized_source`: an operator call written without parentheses has to gain some before a
/// predicate can hang off it.
fn parenthesized_source(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let source = context.source.node_text(node);
    match require_parentheses(context, node) {
        true => format!("({source})"),
        false => source.to_owned(),
    }
}

fn require_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        // `&&` and `||` are not sends upstream, so they are not binary operations either.
        "binary" => node.field("operator").is_some_and(|operator| {
            super::nodes::is_operator_method(context.source.node_text(operator))
        }),
        // `a[b]` is a call to `:[]`, and its `loc.begin` is a bracket rather than a parenthesis.
        "element_reference" => true,
        "call" => {
            node.field("receiver").is_some()
                && node.field("method").is_some_and(|method| {
                    super::nodes::is_operator_method(context.source.node_text(method))
                })
                && !node
                    .field("arguments")
                    .is_some_and(|arguments| context.source.node_text(arguments).starts_with('('))
        }
        _ => false,
    }
}

/// `negated?`: the comparison this cop is about to write sits under a `!`.
fn negated(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        parent.kind_str() == "unary"
            && parent
                .field("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "!")
    })
}

fn selector_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let selector = match node.kind_str() {
        "binary" => node.field("operator"),
        _ => node.field("method"),
    }?;
    Some(context.source.node_text(selector).to_owned())
}

/// `AllowedMethods` and `AllowedPatterns`, which spare a call and everything written inside it.
struct Allowed {
    methods: Vec<String>,
    patterns: Vec<Regex>,
}

impl Allowed {
    fn new(context: &RuleContext<'_>) -> Self {
        let patterns: Vec<String> = context.setting("AllowedPatterns").unwrap_or_default();
        Self {
            methods: context.setting("AllowedMethods").unwrap_or_default(),
            patterns: patterns
                .iter()
                .filter_map(|pattern| Regex::new(pattern).ok())
                .collect(),
        }
    }

    fn is_set(&self) -> bool {
        !self.methods.is_empty() || !self.patterns.is_empty()
    }

    fn matches(&self, name: Option<&str>) -> bool {
        let Some(name) = name else {
            return false;
        };
        self.methods.iter().any(|allowed| allowed == name)
            || self.patterns.iter().any(|pattern| pattern.is_match(name))
    }
}

/// `node.each_ancestor(:send, :any_block)`: a call or block written around this one.
fn ancestor_is_allowed(context: &RuleContext<'_>, node: Node<'_>, allowed: &Allowed) -> bool {
    let mut current = node.parent_of(context);
    while let Some(parent) = current {
        if matches!(
            parent.kind_str(),
            "binary" | "call" | "unary" | "element_reference"
        ) && allowed.matches(selector_name(context, parent).as_deref())
        {
            return true;
        }
        current = parent.parent_of(context);
    }
    false
}
