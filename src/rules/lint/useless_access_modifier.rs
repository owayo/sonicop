use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::access_modifier::{
    bare_access_modifier, child_nodes, class_constructor, modifier_name, send_name, statements,
};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// `static_method_definition?`'s macro half. A call to any of these defines methods however many
/// names it was given, including none at all.
const ATTRIBUTE_MACROS: [&str; 4] = ["attr", "attr_reader", "attr_writer", "attr_accessor"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut cop = Cop {
        context,
        context_creating: setting(context, "ContextCreatingMethods"),
        method_creating: setting(context, "MethodCreatingMethods"),
        active_support: context
            .setting_of("AllCops", "ActiveSupportExtensionsEnabled")
            .unwrap_or(false),
        reported: HashSet::new(),
        offenses,
    };
    // `on_begin` fires for the root and nothing else. The root is a `begin` only when the file
    // holds more than one statement; with one, the statement itself is the root.
    let root = context.root_node();
    if let Some(top_level) = statements(root)
        && top_level.len() >= 2
    {
        for child in top_level {
            if cop.access_modifier(child) {
                // Every modifier at the top level is useless, whatever came before it: upstream
                // passes the call's own name as the current visibility so the two always match.
                let visibility = send_name(child, context).and_then(modifier_name);
                cop.check_send_node(child, visibility, None);
            }
        }
    }
    // The remaining handlers fire as the commissioner walks the tree, so the nodes have to be
    // visited in the order it enters them: the first message wins where two walks meet.
    for node in context.nodes() {
        match node.kind() {
            "class" | "module" | "singleton_class" => {
                if let Some(body) = node.child_by_field_name("body") {
                    cop.check_node(body);
                }
            }
            // A `block` node upstream stands where tree-sitter puts the call it hangs off, so the
            // call is what carries `on_block`'s position in the walk.
            "call" if node.child_by_field_name("block").is_some() => {
                if !(cop.eval_call(node) || cop.included_block(node)) {
                    continue;
                }
                if let Some(body) = node
                    .child_by_field_name("block")
                    .and_then(|block| block.child_by_field_name("body"))
                {
                    cop.check_node(body);
                }
            }
            _ => {}
        }
    }
}

fn setting(context: &RuleContext<'_>, key: &str) -> Vec<String> {
    context
        .setting::<Vec<String>>(key)
        .unwrap_or_default()
        .into_iter()
        // Some configurations still list `included`, which upstream skips rather than redefine the
        // matcher it already has for it.
        .filter(|method| method != "included")
        .collect()
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    context_creating: Vec<String>,
    method_creating: Vec<String>,
    active_support: bool,
    /// Offense ranges already reported. `Cop::Base#add_offense` keeps a set of them per cop and
    /// file, and this cop reaches the same modifier twice whenever a nested scope is walked both
    /// by its own handler and by the scope around it.
    reported: HashSet<(usize, usize)>,
    offenses: &'a mut Vec<Offense>,
}

impl<'tree> Cop<'_, 'tree> {
    /// `check_node`: a body of several statements opens a scope, and a body that is nothing but a
    /// modifier is useless on its own.
    fn check_node(&mut self, body: Node<'tree>) {
        let Some(statements) = statements(body) else {
            return;
        };
        match statements.as_slice() {
            [] => {}
            [only] => {
                if let Some(visibility) = bare_access_modifier(*only, self.context) {
                    self.report(*only, visibility);
                }
            }
            _ => self.check_scope(body),
        }
    }

    /// `check_scope`: walk one visibility scope from `public`, and report the modifier left over
    /// with nothing to govern.
    fn check_scope(&mut self, node: Node<'tree>) {
        let (visibility, unused) = self.check_child_nodes(node, None, Some("public"));
        if let Some(unused) = unused {
            self.report(unused, visibility.unwrap_or_default());
        }
    }

    fn check_child_nodes(
        &mut self,
        node: Node<'tree>,
        mut unused: Option<Node<'tree>>,
        mut visibility: Option<&'static str>,
    ) -> (Option<&'static str>, Option<Node<'tree>>) {
        for child in child_nodes(node) {
            if self.access_modifier(child) {
                (visibility, unused) = self.check_send_node(child, visibility, unused);
            } else if self.included_block(child) {
                continue;
            } else if self.method_definition(child) {
                unused = None;
            } else if self.start_of_new_scope(child) {
                self.check_scope(child);
            } else if child.kind() != "singleton_method" {
                // A `defs` is neither a method definition for visibility purposes nor something to
                // walk into: `private` never reaches a singleton method.
                (visibility, unused) = self.check_child_nodes(child, unused, visibility);
            }
        }
        (visibility, unused)
    }

    fn check_send_node(
        &mut self,
        node: Node<'tree>,
        visibility: Option<&'static str>,
        unused: Option<Node<'tree>>,
    ) -> (Option<&'static str>, Option<Node<'tree>>) {
        if let Some(declared) = bare_access_modifier(node, self.context) {
            return self.check_new_visibility(node, unused, declared, visibility);
        }
        if !has_arguments(node) {
            self.report(node, "private_class_method");
            return (visibility, unused);
        }
        // `private_class_method` with arguments falls off the end of upstream's `if`, and the
        // `nil` it returns is destructured into both the visibility and the pending modifier.
        (None, None)
    }

    fn check_new_visibility(
        &mut self,
        node: Node<'tree>,
        mut unused: Option<Node<'tree>>,
        declared: &'static str,
        visibility: Option<&'static str>,
    ) -> (Option<&'static str>, Option<Node<'tree>>) {
        if Some(declared) == visibility {
            self.report(node, declared);
        } else {
            if let Some(unused) = unused {
                self.report(unused, visibility.unwrap_or_default());
            }
            // Once a modifier has been reported, the one that replaced it becomes the candidate --
            // the same modifier is never reported twice for going unused.
            unused = Some(node);
        }
        (Some(declared), unused)
    }

    /// `access_modifier?`: a bare modifier, or any call to `private_class_method` whatever its
    /// receiver.
    fn access_modifier(&self, node: Node<'_>) -> bool {
        bare_access_modifier(node, self.context).is_some()
            || send_name(node, self.context) == Some("private_class_method")
    }

    /// `included_block?`, which only ever holds where the ActiveSupport extensions are enabled.
    fn included_block(&self, node: Node<'_>) -> bool {
        self.active_support
            && node.child_by_field_name("block").is_some()
            && node
                .child_by_field_name("method")
                .is_some_and(|method| self.context.source.node_text(method) == "included")
    }

    /// `method_definition?`: a `def`, an `attr` macro, a `define_method`, or one of the configured
    /// method-creating macros. A `defs` is none of them.
    fn method_definition(&self, node: Node<'_>) -> bool {
        if node.kind() == "method" {
            return true;
        }
        let Some(name) = self.receiverless_name(node) else {
            return false;
        };
        // `define_method` counts with or without the block that gives the method its body.
        ATTRIBUTE_MACROS.contains(&name)
            || name == "define_method"
            || self.method_creating.iter().any(|method| method == name)
    }

    /// `start_of_new_scope?`
    fn start_of_new_scope(&self, node: Node<'_>) -> bool {
        matches!(node.kind(), "module" | "class" | "singleton_class") || self.eval_call(node)
    }

    /// `eval_call?`: a block that reopens a class -- `class_eval`, `instance_eval`, a class
    /// constructor, or one of the configured context-creating macros.
    fn eval_call(&self, node: Node<'_>) -> bool {
        if node.kind() != "call" {
            return false;
        }
        let has_block = node.child_by_field_name("block").is_some();
        let name = node
            .child_by_field_name("method")
            .map(|method| self.context.source.node_text(method));
        // `(any_block (send _ {:class_eval :instance_eval}) ...)` takes no arguments; a receiver
        // of any shape, including none, satisfies the `_`.
        if has_block && !has_arguments(node) && matches!(name, Some("class_eval" | "instance_eval"))
        {
            return true;
        }
        if class_constructor(node, self.context) {
            return true;
        }
        has_block
            && self.receiverless_or_constant(node)
            && name.is_some_and(|name| self.context_creating.iter().any(|method| method == name))
    }

    /// The name of a call written without a receiver, which is how the `(send nil? :name ...)`
    /// half of the method-definition patterns is spelled.
    fn receiverless_name<'a>(&'a self, node: Node<'_>) -> Option<&'a str> {
        if node.kind() == "call" && node.child_by_field_name("receiver").is_some() {
            return None;
        }
        send_name_allowing_block(node, self.context)
    }

    /// `{nil? const}`: the receiver a context-creating macro is allowed, which is either absent or
    /// a plain constant.
    fn receiverless_or_constant(&self, node: Node<'_>) -> bool {
        node.child_by_field_name("receiver")
            .is_none_or(|receiver| receiver.kind() == "constant")
    }

    fn report(&mut self, node: Node<'_>, visibility: &str) {
        let range = node.byte_range();
        if !self.reported.insert((range.start, range.end)) {
            return;
        }
        let lines = whole_lines(self.context, &range);
        self.offenses.push(
            self.context
                .offense(format!("Useless `{visibility}` access modifier."), range)
                .corrected_by(Edit {
                    start: lines.start,
                    end: lines.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// The name of a call that may carry a block, which the method-definition patterns reach through
/// `any_block`.
fn send_name_allowing_block<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind() {
        "call" => node
            .child_by_field_name("method")
            .map(|method| context.source.node_text(method)),
        _ => send_name(node, context),
    }
}

fn has_arguments(node: Node<'_>) -> bool {
    node.child_by_field_name("arguments")
        .is_some_and(|arguments| arguments.named_child_count() > 0)
}

/// `range_by_whole_lines(range, include_final_newline: true)`: the lines the offense sits on, with
/// the line break that ends the last of them.
fn whole_lines(context: &RuleContext<'_>, range: &Range<usize>) -> Range<usize> {
    let (first, _) = context.source.line_column(range.start);
    let (last, _) = context.source.line_column(range.end);
    context.source.line_start(first)..context.source.line_range(last).end
}
