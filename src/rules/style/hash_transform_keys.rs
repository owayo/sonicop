use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    super::hash_transform::check(context, offenses, super::hash_transform::Half::Key);
}
