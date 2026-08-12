use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Empty file detected.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let source = context.source.text();
    // With `AllowComments` set -- as it is by default -- only a file with nothing in it at all is
    // reported; otherwise a file of nothing but comments and blank lines counts as empty too.
    let allow_comments: bool = context.setting("AllowComments").unwrap_or(true);
    let offending = context.source.is_empty_as_read()
        || (!allow_comments
            && source
                .lines()
                .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#')));
    if !offending {
        return;
    }
    // `add_global_offense`, which upstream anchors at the head of the file: what is wrong is the
    // file itself, so there is no syntax to point at.
    offenses.push(context.offense(MSG, 0..0));
}
