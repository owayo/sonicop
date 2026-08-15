//! `Lint/NameTypo`.
//!
//! Both handlers go through `check?`, which is `project_index && defined?(DidYouMean::SpellChecker)
//! && cop_config[key]`. The index is built only when `AllCops: UseProjectIndex` is switched on and
//! the `rubydex` gem is installed, neither of which is the default, so the cop reports nothing
//! without it.
//!
//! The index is what the cop compares a name *against*: the constants and methods the project
//! declares, which is where the "did you mean" suggestion comes from. One file cannot supply it.

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(_context: &RuleContext<'_>, _offenses: &mut Vec<Offense>) {}
