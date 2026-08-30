use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_iter;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < RubyVersion::new(2, 7) {
        return;
    }
    for case in context.nodes_of("case_match") {
        let _cursor = case.walk();
        let mut catch_all_found = false;
        let mut else_clause = None;
        for branch in named_children_iter(case, context) {
            match branch.kind_str() {
                "else" => else_clause = Some(branch),
                "in_clause" => {
                    if catch_all_found {
                        offenses.push(context.offense(
                            "Unreachable `in` pattern branch detected.",
                            // A trailing comment is no part of the branch upstream, which ends
                            // at the last statement the branch holds.
                            crate::rules::support::expression_range(branch),
                        ));
                        continue;
                    }
                    // A guard makes even a bare name conditional, so the branches below it can
                    // still be reached.
                    if branch.field("guard").is_none()
                        && branch.field("pattern").is_some_and(is_catch_all)
                    {
                        catch_all_found = true;
                    }
                }
                _ => {}
            }
        }
        if !catch_all_found {
            continue;
        }
        // `case_node.loc.else`: the keyword alone, not the branch it opens.
        if let Some(keyword) = else_clause.and_then(|clause| clause.child(0)) {
            offenses
                .push(context.offense("Unreachable `else` branch detected.", keyword.byte_range()));
        }
    }
}

/// `catch_all_pattern?`: a pattern that binds whatever it is given.
fn is_catch_all(pattern: Node<'_>) -> bool {
    match pattern.kind_str() {
        // `match_var`: a bare name binds anything. A pinned name reads a variable instead and is
        // a node of its own.
        "identifier" => true,
        // `match_as` and the `begin` a parenthesized pattern builds: the test is the left half.
        "as_pattern" => pattern.field("value").is_some_and(is_catch_all),
        "parenthesized_pattern" => pattern.named_child(0).is_some_and(is_catch_all),
        "alternative_pattern" => {
            let mut cursor = pattern.walk();
            pattern.named_children(&mut cursor)
                .any(|alternative| is_catch_all(alternative))
        }
        _ => false,
    }
}
