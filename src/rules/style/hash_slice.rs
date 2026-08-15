//! `Style/HashSlice`: keeping only some keys of a hash is `slice`.

use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;

/// `minimum_target_ruby_version 2.5`: `Hash#slice` arrived in 2.5.
const MINIMUM: RubyVersion = RubyVersion::new(2, 5);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM {
        return;
    }
    // `semantically_slice_method?` is the negation of `semantically_except_method?`.
    super::hash_subset::check(context, offenses, "slice", false);
}
