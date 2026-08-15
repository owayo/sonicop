//! Parentheses around the arguments of a method call, required or forbidden.
//!
//! Upstream splits the two styles into a mixin each, and they have almost nothing in common:
//! `require_parentheses` asks four short questions, while `omit_parentheses` has to enumerate
//! every place where dropping the parentheses would change what the code means.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::access_modifier::in_macro_scope;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children;

const REQUIRE_MSG: &str = "Use parentheses for method calls with arguments.";

/// `Node::OPERATOR_METHODS`.
const OPERATOR_METHODS: [&str; 30] = [
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `omit_parentheses` is the other half of the cop and is not ported; leaving the default style
    // to answer for it would report the exact opposite of what was asked for.
    if context
        .setting::<String>("EnforcedStyle")
        .is_some_and(|style| style != "require_parentheses")
    {
        return;
    }
    let cop = Cop::new(context);
    for node in context.nodes_of_any(&["call", "yield"]) {
        let Some(call) = Call::of(node, context) else {
            continue;
        };
        cop.require_parentheses(&call, offenses);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    allowed_methods: Vec<String>,
    allowed_patterns: Vec<regex::Regex>,
    ignore_macros: bool,
    included_macros: Vec<String>,
    included_macro_patterns: Vec<regex::Regex>,
}

impl<'a, 'tree> Cop<'a, 'tree> {
    fn new(context: &'a RuleContext<'tree>) -> Self {
        Self {
            context,
            allowed_methods: context.setting("AllowedMethods").unwrap_or_default(),
            allowed_patterns: patterns(context, "AllowedPatterns"),
            ignore_macros: context.setting("IgnoreMacros").unwrap_or(true),
            included_macros: context.setting("IncludedMacros").unwrap_or_default(),
            included_macro_patterns: patterns(context, "IncludedMacroPatterns"),
        }
    }

    fn require_parentheses(&self, call: &Call<'tree>, offenses: &mut Vec<Offense>) {
        let name = self.context.source.node_text(call.selector);
        if self.allowed_method_name(name) || self.eligible_for_omission(call, name) {
            return;
        }
        let list = call.arguments();
        if list.is_empty() || call.parenthesized(self.context) {
            return;
        }
        // `args_begin`: the one character after the selector, or the two that a written `(` makes
        // it, which the `(` is put in place of.
        let width = match args_parenthesized(&list) {
            true => 2,
            false => 1,
        };
        let Some(begin) = following(call.selector.end_byte(), width, self.context) else {
            return;
        };
        let mut edits = vec![Edit {
            start: begin.start,
            end: begin.end,
            replacement: "(".to_owned(),
            safe: true,
        }];
        // `args_end`: the end of the send, which the `)` goes after.
        let end = call.send_end();
        if !args_parenthesized(&list) {
            edits.push(Edit {
                start: end,
                end,
                replacement: ")".to_owned(),
                safe: true,
            });
        }
        offenses.push(
            self.context
                .offense(REQUIRE_MSG, call.node.start_byte()..end)
                .corrected_by_all(edits),
        );
    }

    /// `allowed_method_name?`.
    fn allowed_method_name(&self, name: &str) -> bool {
        self.allowed_methods.iter().any(|allowed| allowed == name)
            || self
                .allowed_patterns
                .iter()
                .any(|pattern| pattern.is_match(name))
    }

    /// `eligible_for_parentheses_omission?`.
    ///
    /// `setter_method?` needs no test of its own: what upstream reads as `foo.bar = baz` is an
    /// assignment here, and the call on its left never carries arguments.
    fn eligible_for_omission(&self, call: &Call<'tree>, name: &str) -> bool {
        OPERATOR_METHODS.contains(&name) || self.ignored_macro(call, name)
    }

    /// `ignored_macro?`.
    fn ignored_macro(&self, call: &Call<'tree>, name: &str) -> bool {
        self.ignore_macros
            && call.node.field("receiver").is_none()
            && in_macro_scope(call.node, self.context)
            && !self
                .included_macros
                .iter()
                .any(|macro_name| macro_name == name)
            && !self
                .included_macro_patterns
                .iter()
                .any(|pattern| pattern.is_match(name))
    }
}

fn patterns(context: &RuleContext<'_>, key: &str) -> Vec<regex::Regex> {
    context
        .setting::<Vec<String>>(key)
        .unwrap_or_default()
        .iter()
        .filter_map(|pattern| regex::Regex::new(pattern).ok())
        .collect()
}

/// A call as the cop reads it: what `on_send`, `on_csend` and `on_yield` are handed.
struct Call<'tree> {
    node: Node<'tree>,
    /// `loc.selector`, which for a `yield` is `loc.keyword`.
    selector: Node<'tree>,
    /// The argument list, which is missing from a call written without one.
    list: Option<Node<'tree>>,
}

impl<'tree> Call<'tree> {
    fn of(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Self> {
        let (selector, list) = match node.kind_str() {
            // `super 1` is a `super` node upstream, which no handler of this cop is called for --
            // `Style/SuperWithArgsParentheses` has it instead.
            "call" => match node.field("method") {
                Some(method) if method.kind_str() != "super" => (method, node.field("arguments")),
                _ => return None,
            },
            "yield" => (
                node.child(0)?,
                named_children(node)
                    .into_iter()
                    .find(|child| child.kind_str() == "argument_list"),
            ),
            _ => return None,
        };
        let _ = context;
        Some(Self {
            node,
            selector,
            list,
        })
    }

    /// `node.arguments`, which folds a brace-less hash into the one argument upstream builds.
    fn arguments(&self) -> Vec<Node<'tree>> {
        self.list.map(argument_heads).unwrap_or_default()
    }

    /// `node.source_range.end`: where the send itself stops.
    ///
    /// A block is a node *around* the send upstream and a child of the call here, so the call
    /// runs on past the send by however long the block is.
    fn send_end(&self) -> usize {
        match self.node.field("block") {
            Some(_) => self
                .list
                .map_or(self.selector.end_byte(), |list| list.end_byte()),
            None => self.node.end_byte(),
        }
    }

    /// `node.parenthesized?`: the list opens with a `(` written against the selector. A space in
    /// between makes it a parenthesized *argument* instead, which is a `begin` upstream.
    fn parenthesized(&self, context: &RuleContext<'_>) -> bool {
        self.list.is_some_and(|list| {
            list.start_byte() == self.selector.end_byte()
                && context.source.slice(list.byte_range()).starts_with('(')
        })
    }
}

/// `node.arguments`: the list upstream builds, in which the trailing run of `key: value` pairs
/// and `**splat`s is one `hash` argument rather than several. Only the count and the first entry
/// are ever asked for, so each argument is just the node it opens with.
fn argument_heads<'tree>(list: Node<'tree>) -> Vec<Node<'tree>> {
    let mut heads: Vec<Node<'tree>> = Vec::new();
    let mut in_hash = false;
    for child in named_children(list) {
        if child.kind_str() == "comment" {
            continue;
        }
        let pair = matches!(child.kind_str(), "pair" | "hash_splat_argument");
        if pair && in_hash {
            continue;
        }
        in_hash = pair;
        heads.push(child);
    }
    heads
}

/// `args_parenthesized?`: the one argument is itself written in parentheses, whose `(` the
/// correction moves rather than adding one of its own.
fn args_parenthesized(list: &[Node<'_>]) -> bool {
    matches!(list, [only] if only.kind_str() == "parenthesized_statements")
}

/// The span of the next `width` characters, which is what `Range#resize` measures.
fn following(
    start: usize,
    width: usize,
    context: &RuleContext<'_>,
) -> Option<std::ops::Range<usize>> {
    let text = context.source.text();
    let end = text
        .get(start..)?
        .char_indices()
        .nth(width)
        .map_or(text.len(), |(offset, _)| start + offset);
    (end > start).then_some(start..end)
}
