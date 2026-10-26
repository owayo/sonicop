
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
    // 本家の register_offense と同じく、指定された形式と無関係な名前は報告しない。
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
    let expanded = expand_path(context.source.path());
    let path = expanded.to_string_lossy();
    // 本家 1.89 の Windows CLI は message 内にも / を使う。Unix の実ファイル名に
    // 含まれるバックスラッシュは区切りではないため、その環境では置換しない。
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    // add_global_offense は本文ではなくファイル名を指すため、先頭の空範囲に置く。
    offenses.push(context.offense(message.replace("%<path>s", &path), 0..0));
}

