//! `Lint/DeprecatedReference`.
//!
//! Every handler of this cop opens with `return unless project_index`, and the index is built only
//! when `AllCops: UseProjectIndex` is switched on *and* the `rubydex` gem is installed -- neither
//! of which is the default. Without it the cop reports nothing, whatever the file holds, because
//! the `@deprecated` tags it looks for live in the *declarations* the index resolves to rather
//! than in the file being inspected.
//!
//! sonicop indexes nothing across files, so it stands where upstream stands with the switch off.
//! Reproducing the other half means resolving a method or a constant to its definition anywhere in
//! the project and reading the comments above it, which is a whole-project index rather than a
//! cop.

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(_context: &RuleContext<'_>, _offenses: &mut Vec<Offense>) {}
