//! `Lint/UnusedPrivateMethod`.
//!
//! `on_def` opens with `return unless project_index`. The index is built only when
//! `AllCops: UseProjectIndex` is switched on *and* the `rubydex` gem is installed -- neither of
//! which is the default -- so without it the cop reports nothing whatever the file holds.
//!
//! What it needs the index for is not incidental: the question is whether anything *anywhere in
//! the project* calls the method, and whether the class it belongs to has a subclass or an
//! ancestor declaring the same name. None of that can be answered from one file.

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(_context: &RuleContext<'_>, _offenses: &mut Vec<Offense>) {}
