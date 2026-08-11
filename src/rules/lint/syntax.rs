use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SyntaxFeature {
    BeginlessRange,
    ArgumentForwarding,
}

#[derive(Clone, Copy, Debug)]
struct SyntaxFeatureSpec {
    feature: SyntaxFeature,
    available_since: RubyVersion,
}

const SYNTAX_FEATURES: &[SyntaxFeatureSpec] = &[
    SyntaxFeatureSpec {
        feature: SyntaxFeature::BeginlessRange,
        available_since: RubyVersion::new(2, 7),
    },
    SyntaxFeatureSpec {
        feature: SyntaxFeature::ArgumentForwarding,
        available_since: RubyVersion::new(2, 7),
    },
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.root_node().has_error() {
        for node in context.nodes() {
            let nested_error = node.parent().is_some_and(|parent| parent.is_error());
            if (!node.is_error() && !node.is_missing()) || nested_error {
                continue;
            }
            let token = context.source.node_text(node).trim();
            let message = if node.is_missing() {
                format!("unexpected end-of-input; expected {}", node.kind())
            } else if token.is_empty() {
                "unexpected token".to_owned()
            } else {
                let display: String = token.chars().take(24).collect();
                format!("unexpected token `{display}`")
            };
            offenses.push(
                context.offense(
                    syntax_message(&message, context.target_ruby_version()),
                    node.start_byte()
                        ..node
                            .end_byte()
                            .max(node.start_byte() + usize::from(!context.source.is_empty())),
                ),
            );
        }
    }
    version_gated_syntax(context, offenses);
}

fn version_gated_syntax(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let target = context.target_ruby_version();
    for node in context.nodes() {
        let Some((feature, start, end, token_name)) = feature_use(node, context) else {
            continue;
        };
        let Some(spec) = SYNTAX_FEATURES.iter().find(|spec| spec.feature == feature) else {
            continue;
        };
        if target >= spec.available_since {
            continue;
        }
        offenses.push(context.offense(
            syntax_message(&format!("unexpected token {token_name}"), target),
            start..end,
        ));
        if feature == SyntaxFeature::ArgumentForwarding && node.kind() == "forward_parameter" {
            legacy_forwarding_recovery(node, context, offenses);
        }
    }
}

fn feature_use(
    node: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<(SyntaxFeature, usize, usize, &'static str)> {
    match node.kind() {
        "range" if node.child_by_field_name("begin").is_none() => {
            let text = context.source.node_text(node);
            if text.starts_with("...") {
                Some((
                    SyntaxFeature::BeginlessRange,
                    node.start_byte(),
                    node.start_byte() + 3,
                    "tDOT3",
                ))
            } else if text.starts_with("..") {
                Some((
                    SyntaxFeature::BeginlessRange,
                    node.start_byte(),
                    node.start_byte() + 2,
                    "tDOT2",
                ))
            } else {
                None
            }
        }
        "forward_parameter" | "forward_argument" => Some((
            SyntaxFeature::ArgumentForwarding,
            node.start_byte(),
            node.end_byte(),
            "tDOT3",
        )),
        _ => None,
    }
}

fn legacy_forwarding_recovery(
    parameter: Node<'_>,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let Some(method) = ancestor_matching(parameter, |node| {
        matches!(node.kind(), "method" | "singleton_method")
    }) else {
        return;
    };
    let Some(container) =
        ancestor_matching(method, |node| matches!(node.kind(), "class" | "module"))
    else {
        return;
    };
    let Some(body) = container.child_by_field_name("body") else {
        return;
    };
    let later_nodes = significant_named_children(body)
        .filter(|node| node.start_byte() >= method.end_byte())
        .collect::<Vec<_>>();
    if later_nodes.is_empty() {
        return;
    }

    let (keyword, reason) = if container.kind() == "class" {
        ("class", "class definition in method body")
    } else {
        ("module", "module definition in method body")
    };
    offenses.push(context.offense(
        syntax_message(reason, context.target_ruby_version()),
        container.start_byte()..container.start_byte() + keyword.len(),
    ));

    let has_preceding_top_level_statement =
        std::iter::successors(container.prev_named_sibling(), |node| {
            node.prev_named_sibling()
        })
        .any(|node| node.kind() != "comment");
    let later_nonempty_method = later_nodes.iter().any(|node| {
        matches!(node.kind(), "method" | "singleton_method")
            && node
                .child_by_field_name("body")
                .is_some_and(|body| significant_named_children(body).next().is_some())
    });
    if container.kind() != "module" || !has_preceding_top_level_statement || !later_nonempty_method
    {
        return;
    }

    let end = container.end_byte();
    let start = end.saturating_sub(3);
    if context.source.slice(start..end) == "end" {
        offenses.push(context.offense(
            syntax_message("unexpected token kEND", context.target_ruby_version()),
            start..end,
        ));
    }
}

fn significant_named_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind() != "comment")
        .collect::<Vec<_>>()
        .into_iter()
}

fn ancestor_matching(mut node: Node<'_>, predicate: impl Fn(Node<'_>) -> bool) -> Option<Node<'_>> {
    while let Some(parent) = node.parent() {
        if predicate(parent) {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn syntax_message(reason: &str, target: RubyVersion) -> String {
    format!(
        "{reason}\n(Using Ruby {target} parser; configure using `TargetRubyVersion` parameter, under `AllCops`)"
    )
}
