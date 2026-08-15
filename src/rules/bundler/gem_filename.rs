
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::support::expand_path;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "Gemfile".to_owned());
    let basename = context
        .source
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // The manifest the configuration asks for is fine; so is a name this cop was never pointed at,
    // which upstream reaches through `register_offense` without either branch matching.
    let message = match (style.as_str(), basename) {
        ("Gemfile", "gems.rb") => {
            "`gems.rb` file was found but `Gemfile` is required (file path: %<path>s)."
        }
        ("Gemfile", "gems.locked") => {
            "Expected a `Gemfile.lock` with `Gemfile` but found `gems.locked` file \
             (file path: %<path>s)."
        }
        ("gems.rb", "Gemfile") => {
            "`Gemfile` was found but `gems.rb` file is required (file path: %<path>s)."
        }
        ("gems.rb", "Gemfile.lock") => {
            "Expected a `gems.locked` file with `gems.rb` but found `Gemfile.lock` \
             (file path: %<path>s)."
        }
        _ => return,
    };
    // `add_global_offense`, which upstream anchors at the head of the file: the offense is the name
    // of the file, and no part of what is in it.
    offenses.push(context.offense(
        message.replace(
            "%<path>s",
            &expand_path(context.source.path()).to_string_lossy(),
        ),
        0..0,
    ));
}

