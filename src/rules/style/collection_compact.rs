//! `Style/CollectionCompact`: dropping the `nil`s of a collection is `compact`.

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{arguments, symbol_name};

/// `minimum_target_ruby_version 2.4`.
const MINIMUM: RubyVersion = RubyVersion::new(2, 4);

/// `RESTRICT_ON_SEND`.
const REJECT_METHODS: &[&str] = &["reject", "reject!"];
const SELECT_METHODS: &[&str] = &["select", "select!", "filter", "filter!"];

/// `TO_ENUM_METHODS`.
const TO_ENUM_METHODS: &[&str] = &["to_enum", "lazy"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let name = context.source.node_text(selector);
        if !REJECT_METHODS.contains(&name) && !SELECT_METHODS.contains(&name) && name != "grep_v" {
            continue;
        }
        // `filter` and `filter!` only became aliases of `select` in 2.6.
        if context.target_ruby_version() < RubyVersion::new(2, 6)
            && matches!(name, "filter" | "filter!")
        {
            continue;
        }
        if !matches(node, name, context, &locals) {
            continue;
        }
        if node
            .field("receiver")
            .is_some_and(|receiver| allowed_receiver(receiver, context))
        {
            continue;
        }
        // `to_enum_method?`: before 3.1 `Enumerator#compact` did not exist, so a lazy or
        // enumerated collection has nothing to replace the block with.
        if context.target_ruby_version() <= RubyVersion::new(3, 0) && to_enum_method(node, context)
        {
            continue;
        }
        let good = if name.ends_with('!') {
            "compact!"
        } else {
            "compact"
        };
        let range = selector.start_byte()..node.end_byte();
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{good}` instead of `{}`.",
                        context.source.slice(range.clone())
                    ),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: good.to_owned(),
                    safe: true,
                }),
        );
    }
}

/// The four shapes: `reject(&:nil?)`, `grep_v(nil)`, and the block forms of `reject` and `select`.
fn matches(
    node: Node<'_>,
    name: &str,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    let list = arguments(node);
    if name == "grep_v" {
        // `(call _ :grep_v {(nil) (const {nil? cbase} :NilClass)})`: the receiver may be absent.
        return node.field("block").is_none()
            && match list.as_slice() {
                [argument] => {
                    let argument = argument.first();
                    argument.kind_str() == "nil"
                        || super::nodes::is_top_level_constant(argument, "NilClass", context)
                }
                _ => false,
            };
    }
    // Every other shape asks for a receiver.
    if node.field("receiver").is_none() {
        return false;
    }
    // `reject_method_with_block_pass?`: `(block_pass (sym :nil?))`.
    if REJECT_METHODS.contains(&name)
        && node.field("block").is_none()
        && let [argument] = list.as_slice()
    {
        let argument = argument.first();
        if argument.kind_str() == "block_argument" {
            let inner = super::nodes::children(argument);
            return matches!(inner.as_slice(), [symbol]
                if symbol_name(*symbol, context) == Some("nil?"));
        }
        return false;
    }
    if !list.is_empty() {
        return false;
    }
    let Some(block) = node.field("block") else {
        return false;
    };
    let body = super::nodes::children(match block.field("body") {
        Some(body) => body,
        None => return false,
    });
    let [statement] = body.as_slice() else {
        return false;
    };
    // `select` inverts the test with a `!`, and `reject` does not.
    let tested = if SELECT_METHODS.contains(&name) {
        // `(call (call $(lvar _) :nil?) :!)`: 本家の `call` は `send` と `csend` の両方を指す
        // ので、`!x.nil?` にも `x&.nil?&.!` にも当たる。文法は前者を `unary`、後者を
        // `call` (method が `!`) に割るため、`unary` だけを見ると `&.!` の形が丸ごと落ちる。
        match negated(*statement, context) {
            Some(operand) => operand,
            None => return false,
        }
    } else {
        *statement
    };
    let Some(subject) = nil_check(tested, context) else {
        return false;
    };
    match block.field("parameters") {
        // `args.last.source == receiver.source`: the block's own last parameter is what is asked.
        Some(parameters) => {
            if !locals.is_lvar(subject) {
                return false;
            }
            super::nodes::children(parameters)
                .last()
                .is_some_and(|last| {
                    context.source.node_text(*last) == context.source.node_text(subject)
                })
        }
        // A block with no parameters names its argument `_1` or, from 3.4, `it`.
        None => match context.source.node_text(subject) {
            "_1" => true,
            "it" => context.target_ruby_version() >= RubyVersion::new(3, 4),
            _ => false,
        },
    }
}

/// `(call X :!)`: what a negation was applied to, whichever way the `!` was written.
fn negated<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "unary" => (node.child(0)?.kind_str() == "!").then(|| node.field("operand"))?,
        "call" => (context.source.node_text(node.field("method")?) == "!")
            .then(|| node.field("receiver"))?,
        _ => None,
    }
}

/// `(call $(lvar _) :nil?)`: the variable a `nil?` was asked of.
fn nil_check<'tree>(node: Node<'tree>, context: &RuleContext<'_>) -> Option<Node<'tree>> {
    if node.kind_str() != "call" || node.field("block").is_some() || !arguments(node).is_empty() {
        return None;
    }
    if context.source.node_text(node.field("method")?) != "nil?" {
        return None;
    }
    let receiver = node.field("receiver")?;
    (receiver.kind_str() == "identifier").then_some(receiver)
}

/// `to_enum_method?`.
fn to_enum_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.field("receiver").is_some_and(|receiver| {
        receiver.kind_str() == "call"
            && receiver
                .field("method")
                .is_some_and(|name| TO_ENUM_METHODS.contains(&context.source.node_text(name)))
    })
}

/// `AllowedReceivers#allowed_receiver?`, whose list is empty by default.
fn allowed_receiver(receiver: Node<'_>, context: &RuleContext<'_>) -> bool {
    let allowed: Vec<String> = context.setting("AllowedReceivers").unwrap_or_default();
    if allowed.is_empty() {
        return false;
    }
    allowed.contains(&receiver_name(receiver, context))
}

/// `receiver_name`: the innermost non-constant receiver, spelled as a dotted chain of selectors.
///
/// Upstream stops at a `const_type?` receiver, and **upstream's `const` covers `Foo::Bar` as much as
/// `Foo`** (`Foo::Bar` is a `const` whose child is a `const`). The grammar splits those into two
/// kinds, so both have to be named or the walk runs past the constant and drops the selector that
/// followed it: `Foo::Bar.baz` would answer `Foo::Bar` and match an `AllowedReceivers` entry it
/// should not.
fn receiver_name(receiver: Node<'_>, context: &RuleContext<'_>) -> String {
    if let Some(inner) = receiver.field("receiver")
        && !matches!(inner.kind_str(), "constant" | "scope_resolution")
    {
        return receiver_name(inner, context);
    }
    if receiver.kind_str() != "call" {
        return context.source.node_text(receiver).to_owned();
    }
    let selector = receiver.field("method").map_or_else(String::new, |name| {
        context.source.node_text(name).to_owned()
    });
    match receiver.field("receiver") {
        Some(inner) => format!("{}.{selector}", receiver_name(inner, context)),
        None => selector,
    }
}
