use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const SHEBANG: &str = "#!";

/// Grants no correction on purpose. Upstream's block runs `FileUtils.chmod('+x', ...)` and never
/// touches the corrector it was handed, so the corrector stays empty, the offense is reported with
/// `correctable: false`, and `-a` leaves the text of the file exactly as it was.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if cfg!(windows) || !context.source.line(1).starts_with(SHEBANG) || executable(context) {
        return;
    }
    let Some(comment) = context.comment_ranges().first() else {
        return;
    };
    let name = context
        .source
        .path()
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    offenses.push(context.offense(
        format!("Script file {name} doesn't have execute permission."),
        comment.clone(),
    ));
}

/// `File.exist?` then `File.stat(...).executable?`: a source with no file behind it is left alone,
/// since there is nothing to grant permission on.
#[cfg(unix)]
fn executable(context: &RuleContext<'_>) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::metadata(context.source.path()) else {
        return true;
    };
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_context: &RuleContext<'_>) -> bool {
    true
}
