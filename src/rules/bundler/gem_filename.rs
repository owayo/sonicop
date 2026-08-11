use std::path::{Component, Path, PathBuf};

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

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

/// `File.expand_path`: RuboCop resolves every target against the working directory before it
/// inspects it, so the path this cop writes into its message is always an absolute one.
fn expand_path(path: &Path) -> PathBuf {
    let absolute = match path.is_absolute() {
        true => path.to_path_buf(),
        false => std::env::current_dir().unwrap_or_default().join(path),
    };
    let mut expanded = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                expanded.pop();
            }
            component => expanded.push(component),
        }
    }
    expanded
}
