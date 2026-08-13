use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "`else` without `rescue` is useless.";

/// `maximum_target_ruby_version 2.5`: writing one became a syntax error in 2.6, which the parser
/// reports instead -- so the cop is not even built for a run targeting a later version.
const MAXIMUM_VERSION: RubyVersion = RubyVersion::new(2, 5);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() > MAXIMUM_VERSION {
        return;
    }
    // The `:useless_else` diagnostic the parser emits: a body split by an `else` that no `rescue`
    // precedes. The `else` keyword is what it points at.
    for clause in context.nodes_of("else") {
        let Some(body) = clause.parent() else {
            continue;
        };
        // A `case` keeps its `else` under the `case` itself, so only the bodies that a `rescue`
        // could have split are reached here.
        if !matches!(body.kind_str(), "begin" | "body_statement" | "block_body") {
            continue;
        }
        let mut cursor = body.walk();
        if body
            .named_children(&mut cursor)
            .any(|child| child.kind_str() == "rescue")
        {
            continue;
        }
        let Some(keyword) = clause.child(0) else {
            continue;
        };
        offenses.push(context.offense(MSG, keyword.byte_range()));
    }
}
