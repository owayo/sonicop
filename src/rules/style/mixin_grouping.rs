//! `Style/MixinGrouping`: one `include` per module, or one `include` naming every module.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `MIXIN_METHODS`.
const MIXIN_METHODS: &[&str] = &["extend", "include", "prepend"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let separated = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "separated");

    // Upstream only has `on_class` / `on_module`. A `class << self` nested in either one's body
    // is not a class node to that callback, so its mixins are deliberately outside this cop.
    for holder in context.nodes_of_any(&["class", "module"]) {
        let Some(body) = holder.field("body") else {
            continue;
        };
        let statements = super::nodes::children(body);
        for node in &statements {
            let Some(mixin) = Mixin::new(context, *node) else {
                continue;
            };
            let suffix = match separated {
                // `check_separated_style`: one statement naming one module is already right.
                true if mixin.modules.len() == 1 => continue,
                true => "separate statements",
                // `check_grouped_style`: one statement is already the whole group.
                false if siblings(context, &statements, &mixin).len() == 1 => continue,
                false => "a single statement",
            };
            let message = format!("Put `{}` mixins in {suffix}.", mixin.method);
            let edit = match separated {
                true => separate(context, &mixin),
                false => group(context, &statements, &mixin),
            };
            offenses.push(
                context
                    .offense(message, node.byte_range())
                    .corrected_by(edit),
            );
        }
    }
}

/// One `include Foo, Bar` macro.
struct Mixin<'t> {
    node: Node<'t>,
    method: String,
    modules: Vec<std::ops::Range<usize>>,
}

impl<'t> Mixin<'t> {
    /// `macro?` narrowed to the mixin methods: a receiverless call written straight in a class or
    /// module body, with at least one module named.
    fn new(context: &RuleContext<'_>, node: Node<'t>) -> Option<Self> {
        if node.kind_str() != "call" || node.field("receiver").is_some() {
            return None;
        }
        let method = context.source.node_text(node.field("method")?).to_owned();
        if !MIXIN_METHODS.contains(&method.as_str()) {
            return None;
        }
        let modules: Vec<std::ops::Range<usize>> = send_node::arguments(node)
            .iter()
            .map(send_node::Argument::range)
            .collect();
        (!modules.is_empty()).then_some(Self {
            node,
            method,
            modules,
        })
    }
}

/// `sibling_mixins`: the statements of the same body that mix in with the same method.
fn siblings<'t>(
    context: &RuleContext<'_>,
    statements: &[Node<'t>],
    mixin: &Mixin<'_>,
) -> Vec<Mixin<'t>> {
    statements
        .iter()
        .filter_map(|node| Mixin::new(context, *node))
        .filter(|sibling| sibling.method == mixin.method)
        .collect()
}

/// `separate_mixins`: one statement per module, in reverse order -- the modules a single statement
/// names take effect from the last one back.
fn separate(context: &RuleContext<'_>, mixin: &Mixin<'_>) -> Edit {
    let indent = " ".repeat(context.source.line_column(mixin.node.start_byte()).1 - 1);
    let written: Vec<String> = mixin
        .modules
        .iter()
        .rev()
        .enumerate()
        .map(|(position, module)| {
            let leading = match position {
                0 => "",
                _ => indent.as_str(),
            };
            format!(
                "{leading}{} {}",
                mixin.method,
                context.source.slice(module.clone())
            )
        })
        .collect();
    Edit {
        start: mixin.node.start_byte(),
        end: mixin.node.end_byte(),
        replacement: written.join("\n"),
        safe: true,
    }
}

/// `check_grouped_style`: the first statement takes every module, and the ones after it go.
fn group(context: &RuleContext<'_>, statements: &[Node<'_>], mixin: &Mixin<'_>) -> Edit {
    let siblings = siblings(context, statements, mixin);
    if siblings
        .first()
        .is_some_and(|first| first.node.byte_range() == mixin.node.byte_range())
    {
        let names: Vec<&str> = siblings
            .iter()
            .rev()
            .flat_map(|sibling| &sibling.modules)
            .map(|module| context.source.slice(module.clone()))
            .collect();
        return Edit {
            start: mixin.node.start_byte(),
            end: mixin.node.end_byte(),
            replacement: format!("{} {}", mixin.method, names.join(", ")),
            safe: true,
        };
    }
    // `range_to_remove_for_subsequent_mixin`: the statement goes, along with the whitespace that
    // separated it from the mixin before it.
    let mut start = mixin.node.start_byte();
    if let Some(previous) = siblings
        .iter()
        .rev()
        .find(|sibling| sibling.node.start_byte() < mixin.node.start_byte())
    {
        let between = context.source.slice(previous.node.end_byte()..start);
        if between.trim().is_empty() {
            start = previous.node.end_byte();
        }
    }
    Edit {
        start,
        end: mixin.node.end_byte(),
        replacement: String::new(),
        safe: true,
    }
}
