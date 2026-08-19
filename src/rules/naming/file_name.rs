use std::sync::LazyLock;

use regex::Regex;

use super::support::{ruby_regex, ruby_regex_to_s};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

/// `SNAKE_CASE`, whose POSIX class is Unicode-aware in Ruby. A dot is allowed because only the
/// last extension is stripped before the name is judged.
static SNAKE_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[0-9\p{Lowercase}_.?!]+$").unwrap());

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let path = context.source.path();
    if context.config.allowed_camel_case_file(path) {
        return;
    }
    let Some(basename) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let pattern: Option<&Regex> = context
        .setting::<serde_yaml_ng::Value>("Regex")
        .as_ref()
        .and_then(ruby_regex);
    // `ExpectMatchingDefinition` is off by default, and the checks it turns on are the only ones
    // that inspect the file's contents; a name that is already snake_case is otherwise fine.
    if filename_good(basename, pattern.unwrap_or(&SNAKE_CASE)) {
        return;
    }
    let ignore_scripts: bool = context.setting("IgnoreExecutableScripts").unwrap_or(true);
    if ignore_scripts && context.source.text().starts_with("#!") {
        return;
    }
    let configured = context
        .setting::<serde_yaml_ng::Value>("Regex")
        .as_ref()
        .and_then(ruby_regex_to_s);
    let message = match configured {
        Some(source) => format!("`{basename}` should match `{source}`."),
        None => format!("The name of this source file (`{basename}`) should use snake_case."),
    };
    // `add_global_offense` places the offense nowhere in particular, which the formatters render
    // as the very start of the file.
    offenses.push(context.offense(message, 0..0));
}

/// `filename_good?`: the leading dot and the last extension are dropped, the one `+` an Action Pack
/// variant name carries becomes an underscore, and what is left has to match the pattern.
fn filename_good(basename: &str, pattern: &Regex) -> bool {
    let stem = basename.strip_prefix('.').unwrap_or(basename);
    let stem = match stem.rfind('.') {
        Some(dot) => &stem[..dot],
        None => stem,
    };
    let stem = stem.replacen('+', "_", 1);
    pattern.is_match(&stem)
}
