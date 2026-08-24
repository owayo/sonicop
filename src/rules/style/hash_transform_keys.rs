use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

/// `minimum_target_ruby_version 2.5`: `Hash#transform_keys` arrived in 2.5.
const MINIMUM: RubyVersion = RubyVersion::new(2, 5);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    super::hash_transform::check(context, offenses, super::hash_transform::Half::Key);
}
