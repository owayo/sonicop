use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = Allowed::new(context);
    for node in context.nodes_of("call") {
        let Some(list) = node.field("arguments") else {
            continue;
        };
        // `!node.arguments? && node.parenthesized?`: an empty `()`, which is the only argument list
        // that can go without changing what the call means.
        if !super::nodes::children_in(list, context).is_empty()
            || !context.source.node_text(list).starts_with('(')
        {
            continue;
        }
        // `implicit_call?`: `foo.()` has no selector to keep the parentheses off.
        let Some(method) = node.field("method") else {
            continue;
        };
        // `super()` is a node of its own upstream rather than a call, so `on_send` never sees it.
        if method.kind_str() == "super" {
            continue;
        }
        let name = context.source.node_text(method);
        // `camel_case_method?`: `Integer()` reads as a constant without them.
        if name.starts_with(|character: char| character.is_ascii_uppercase()) {
            continue;
        }
        if allowed.covers(name) {
            continue;
        }
        // `default_argument?`: `def m(a = foo())` needs them to stay a call.
        if node
            .parent_of(context)
            .is_some_and(|parent| parent.kind_str() == "optional_parameter")
        {
            continue;
        }
        if node.field("receiver").is_none()
            && (same_name_assignment(node, name, context) || parenthesized_it_in_block(node, name))
        {
            continue;
        }
        let range = list.byte_range();
        if super::nodes::contains_comment(&range, context) {
            continue;
        }
        offenses.push(
            context
                .offense(
                    "Do not use parentheses for method calls with no arguments.",
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// `same_name_assignment?`: `foo = foo()` has to keep the parentheses, since `foo = foo` reads the
/// variable being assigned rather than calling the method.
fn same_name_assignment(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent_of(context) {
        current = parent;
        let shorthand = parent.kind_str() == "operator_assignment";
        if !shorthand && parent.kind_str() != "assignment" {
            continue;
        }
        let Some(left) = parent.field("left") else {
            continue;
        };
        // `next if asgn_node.shorthand_asgn? && asgn_node.lhs.call_type?`
        if shorthand && matches!(left.kind_str(), "call" | "element_reference") {
            continue;
        }
        let assigned = match left.kind_str() {
            "left_assignment_list" => super::nodes::children_in(left, context)
                .iter()
                .any(|target| assignment_name(*target, context) == Some(name)),
            _ => assignment_name(left, context) == Some(name),
        };
        if assigned {
            return true;
        }
    }
    false
}

/// `loc.name.source` of an assignment target, or `None` for a target upstream reports as a call.
fn assignment_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "identifier" | "instance_variable" | "class_variable" | "global_variable" | "constant" => {
            Some(context.source.node_text(node))
        }
        "scope_resolution" => Some(context.source.node_text(node.field("name")?)),
        _ => None,
    }
}

/// `parenthesized_it_method_in_block?`: from Ruby 3.4 a bare `it` in a block without parameters
/// names the block's argument, so `it()` is the only way left to call the method.
fn parenthesized_it_in_block(node: Node<'_>, name: &str) -> bool {
    if name != "it" {
        return false;
    }
    let mut current = node;
    while let Some(parent) = current.parent() {
        current = parent;
        if !matches!(parent.kind_str(), "block" | "do_block") {
            continue;
        }
        return match parent.field("parameters") {
            // `!block_node.arguments.empty_and_without_delimiters?`: written `| |`, the block has
            // no `it` of its own.
            Some(_) => false,
            None => node.field("block").is_none(),
        };
    }
    false
}

/// `AllowedMethods` and `AllowedPatterns`, both empty by default.
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

    fn covers(&self, name: &str) -> bool {
        self.methods.iter().any(|allowed| allowed == name)
            || self.patterns.iter().any(|pattern| pattern.is_match(name))
    }
}
