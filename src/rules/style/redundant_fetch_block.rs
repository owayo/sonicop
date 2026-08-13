use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::magic_comment::MagicComment;
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, has_interpolation, top_level_constant};
use crate::rules::node_ext::NodeExt;

/// `basic_literal?`: the literal types that hold one value rather than a structure.
const BASIC_LITERALS: &[&str] = &["integer", "float", "rational", "complex", "true", "false", "nil"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let safe_for_constants = context.setting("SafeForConstants").unwrap_or(false);
    let frozen = frozen_string_literals_enabled(context);
    for call in context.nodes_of("call") {
        let Some(block) = call.field("block") else {
            continue;
        };
        // `(args)`: a block that names a parameter is a `numblock` or takes one, and neither
        // matches the pattern.
        if block.field("parameters").is_some() {
            continue;
        }
        let Some(method) = call.field("method") else {
            continue;
        };
        if context.source.node_text(method) != "fetch"
            || call.field("receiver").is_none()
        {
            continue;
        }
        let list = arguments(call);
        let [only] = list.as_slice() else {
            continue;
        };
        let [key] = only.parts() else {
            continue;
        };
        let statements =
            super::conditional::self_statements(block.field("body").unwrap_or(block));
        let body = match block.field("body") {
            Some(_) => match statements.as_slice() {
                [only] => Some(*only),
                _ => continue,
            },
            None => None,
        };
        if let Some(body) = body
            && !is_allowed_body(body, safe_for_constants, frozen)
        {
            continue;
        }
        // `rails_cache?`: `Rails.cache.fetch` takes options the two-argument form cannot express.
        if is_rails_cache(call, context) {
            continue;
        }
        let default = body.map_or("nil", |node| context.source.node_text(node));
        let key_source = context.source.node_text(*key);
        let written = match body {
            Some(node) => format!("{{ {} }}", context.source.node_text(node)),
            None => "{}".to_owned(),
        };
        let range = method.start_byte()..call.end_byte();
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `fetch({key_source}, {default})` instead of `fetch({key_source}) {written}`."
                    ),
                    range.clone(),
                )
                .corrected_by(Edit {
                    start: range.start,
                    end: range.end,
                    replacement: format!("fetch({key_source}, {default})"),
                    safe: true,
                }),
        );
    }
}

/// `{nil? basic_literal? const_type?}` narrowed by `should_not_check?`.
fn is_allowed_body(
    node: Node<'_>,
    safe_for_constants: bool,
    frozen: bool,
) -> bool {
    match node.kind_str() {
        "constant" | "scope_resolution" => safe_for_constants,
        // A string default is only interchangeable while the literal is frozen; otherwise the
        // block hands out a fresh object each time.
        // `?a` is a `str` upstream, so it needs the same frozen-literal check a quoted one does.
        "string" => !has_interpolation(node) && single_line(node) && frozen,
        "character" => frozen,
        "simple_symbol" => true,
        "delimited_symbol" => !has_interpolation(node) && single_line(node),
        kind => BASIC_LITERALS.contains(&kind),
    }
}

/// A literal spread over more than one line is a `dstr` upstream rather than a `str`.
fn single_line(node: Node<'_>) -> bool {
    node.start_position().row == node.end_position().row
}

/// `(send (const _ :Rails) :cache)`.
fn is_rails_cache(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(receiver) = call.field("receiver") else {
        return false;
    };
    if receiver.kind_str() != "call"
        || receiver
            .field("method")
            .is_none_or(|method| context.source.node_text(method) != "cache")
    {
        return false;
    }
    // `(const _ :Rails)`: any scope, so a plain `Rails` and a nested `Foo::Rails` both count.
    receiver
        .field("receiver")
        .is_some_and(|node| {
            top_level_constant(node, "Rails", context)
                || (node.kind_str() == "scope_resolution"
                    && node
                        .field("name")
                        .is_some_and(|name| context.source.node_text(name) == "Rails"))
        })
}

/// `frozen_string_literals_enabled?`: only a magic comment can turn them on for the file, since
/// `AllCops/StringLiteralsFrozenByDefault` is unset by default.
fn frozen_string_literals_enabled(context: &RuleContext<'_>) -> bool {
    for line_number in 1..=context.source.line_count() {
        let line = context.source.line(line_number);
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            break;
        }
        let comment = MagicComment::parse(line);
        if comment.frozen_string_literal_specified() {
            return comment.frozen_string_literal_enabled();
        }
    }
    false
}
