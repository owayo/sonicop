use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::ordered_gem::{self, Declaration};
use crate::rules::send_node::string_text;

use super::support::gem_declarations;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let declarations: Vec<Declaration<'_>> = gem_declarations(context)
        .map(|(node, name)| Declaration {
            node,
            name: string_text(name, context).to_owned(),
        })
        .collect();

    ordered_gem::check(
        context,
        offenses,
        &declarations,
        &|current, previous| {
            format!(
                "Gems should be sorted in an alphabetical order within their section of the \
                 Gemfile. Gem `{current}` should appear before `{previous}`."
            )
        },
        // Every neighbouring pair is comparable: a Gemfile has no sections beyond the groups its
        // blank lines and comments already draw.
        &|_, _| true,
    );
}
