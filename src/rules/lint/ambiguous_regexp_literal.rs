use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::ambiguity::scan;

const MSG: &str = "Ambiguous regexp literal. Parenthesize the method arguments if it's surely a \
     regexp literal, or add a whitespace to the right of the `/` if it should be a division.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for ambiguity in scan(context, &["/"]) {
        offenses.push(
            context
                .offense(MSG, ambiguity.operator.clone())
                .corrections_anchored_at(ambiguity.owner.byte_range())
                .corrected_by_all(ambiguity.parenthesize(context)),
        );
    }
}
