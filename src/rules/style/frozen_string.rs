//! `FrozenStringLiteral`: which literals the frozen string literal feature covers, and whether the
//! file turned it on.
//!
//! Shared by the two cops that reason about it -- `Style/RedundantFreeze` asks whether a `.freeze`
//! is already implied, `Style/MutableConstant` whether a constant still needs one.

use tree_sitter::Node;

use crate::magic_comment::MagicComment;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `frozen_string_literals_enabled?`: whether the file's magic comments turn string literals
/// frozen. The default configuration leaves `StringLiteralsFrozenByDefault` unset, so nothing but a
/// comment can enable it.
pub(super) fn literals_enabled(context: &RuleContext<'_>) -> bool {
    leading_comment_lines(context)
        .find_map(|line| {
            let comment = MagicComment::parse(line);
            comment
                .frozen_string_literal_specified()
                .then(|| comment.frozen_string_literal_enabled())
        })
        .unwrap_or(false)
}

/// The lines above the first one holding code, which is where Ruby reads magic comments.
pub(super) fn leading_comment_lines<'a>(
    context: &'a RuleContext<'_>,
) -> impl Iterator<Item = &'a str> + 'a {
    let first_code = (1..=context.source.line_count()).find(|line_number| {
        let line = context.source.line(*line_number).trim();
        !line.is_empty() && !line.starts_with('#')
    });
    let end = first_code.unwrap_or(context.source.line_count() + 1);
    (1..end).map(|line_number| context.source.line(line_number))
}

/// The node kind, with the keyword variables this grammar leaves as plain identifiers folded back
/// in. `__FILE__` is a `str` upstream holding the path the parser was given and `__LINE__` an
/// `int`, but `_keyword_variable` is declared at a lower precedence than `identifier` here and
/// never wins. None of the three can be a local variable: Ruby will not let one be assigned.
pub(super) fn kind_of(node: Node<'_>, context: &RuleContext<'_>) -> &'static str {
    if node.kind_str() != "identifier" {
        return node.kind_str();
    }
    match context.source.node_text(node) {
        "__FILE__" => "file",
        "__LINE__" => "line",
        "__ENCODING__" => "encoding",
        _ => "identifier",
    }
}

/// `frozen_string_literal?` once the file is known to freeze its literals: which literals the
/// feature covers, which widened in Ruby 3.0 from "every `str` and `dstr`" to "the ones nothing is
/// interpolated into".
pub(super) fn is_frozen(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let string = match kind_of(node, context) {
        "string" | "chained_string" | "character" | "heredoc_beginning" | "file" => true,
        // A `%w` word is only ever an array element, and a backtick literal is an `xstr` the feature
        // never covered.
        _ => false,
    };
    if !string {
        return false;
    }
    if context.target_ruby_version() < RubyVersion::new(3, 0) {
        return true;
    }
    !interpolated(context, node)
}

/// Whether anything is interpolated into a string literal, which is what upstream's
/// `each_descendant(:begin, :ivar, :cvar, :gvar)` finds inside a `dstr`.
pub(super) fn interpolated(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let body = match node.kind_str() {
        "heredoc_beginning" => match send_node::heredoc_body(node, context) {
            Some(body) => body,
            None => return false,
        },
        _ => node,
    };
    send_node::any_descendant(body, &mut |child| child.kind_str() == "interpolation")
}
