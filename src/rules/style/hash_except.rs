//! `Style/HashExcept`: dropping keys from a hash is `except`.

use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

/// `minimum_target_ruby_version 3.0`: `Hash#except` arrived in 3.0.
const MINIMUM: RubyVersion = RubyVersion::new(3, 0);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    super::hash_subset::check(context, offenses, "except", true);
}
