use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

/// `minimum_target_ruby_version 2.4`: `Hash#transform_values` arrived in 2.4.
const MINIMUM: RubyVersion = RubyVersion::new(2, 4);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    super::hash_transform::check(context, offenses, super::hash_transform::Half::Value);
}
