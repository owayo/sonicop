use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn find_config(start: &Path) -> Option<PathBuf> {
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let project_root = find_project_root(&start);

    for directory in start.ancestors() {
        let candidate = directory.join(".rubocop.yml");
        if candidate.is_file() {
            return fs::canonicalize(candidate).ok();
        }
        if project_root.as_deref().is_none_or(|root| directory == root) {
            break;
        }
    }

    if let Some(root) = project_root {
        for candidate in [
            root.join(".config/.rubocop.yml"),
            root.join(".config/rubocop/config.yml"),
        ] {
            if candidate.is_file() {
                return fs::canonicalize(candidate).ok();
            }
        }
    }

    let home = home_directory();
    if let Some(candidate) = home.as_ref().map(|home| home.join(".rubocop.yml"))
        && candidate.is_file()
    {
        return fs::canonicalize(candidate).ok();
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".config")));
    if let Some(candidate) = xdg.map(|root| root.join("rubocop/config.yml"))
        && candidate.is_file()
    {
        return fs::canonicalize(candidate).ok();
    }
    None
}

fn home_directory() -> Option<PathBuf> {
    resolve_home_directory(|key| std::env::var_os(key))
}

/// RuboCop expands `~` through `Dir.home`, which on Windows falls back to
/// `USERPROFILE` and then `HOMEDRIVE`+`HOMEPATH` because `HOME` is normally unset
/// there. Reading `HOME` alone hides the user-global configuration on Windows.
fn resolve_home_directory(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let present = |key: &str| lookup(key).filter(|value| !value.is_empty());
    if let Some(home) = present("HOME").or_else(|| present("USERPROFILE")) {
        return Some(PathBuf::from(home));
    }
    let mut home = present("HOMEDRIVE")?;
    home.push(present("HOMEPATH")?);
    Some(PathBuf::from(home))
}

pub(super) fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .filter(|directory| {
            directory.join("Gemfile").is_file() || directory.join("gems.rb").is_file()
        })
        .last()
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::resolve_home_directory;

    fn environment<'a>(pairs: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(*value))
        }
    }

    #[test]
    fn resolves_home_directory_across_platforms() {
        assert_eq!(
            resolve_home_directory(environment(&[("HOME", "/home/dev")])),
            Some("/home/dev".into())
        );
        // Windows leaves HOME unset, so RuboCop falls through to USERPROFILE.
        assert_eq!(
            resolve_home_directory(environment(&[("USERPROFILE", r"C:\Users\dev")])),
            Some(r"C:\Users\dev".into())
        );
        assert_eq!(
            resolve_home_directory(environment(&[("HOME", ""), ("USERPROFILE", r"D:\dev")])),
            Some(r"D:\dev".into())
        );
        assert_eq!(
            resolve_home_directory(environment(&[
                ("HOMEDRIVE", "C:"),
                ("HOMEPATH", r"\Users\dev")
            ])),
            Some(r"C:\Users\dev".into())
        );
        assert_eq!(
            resolve_home_directory(environment(&[("HOMEDRIVE", "C:")])),
            None
        );
        assert_eq!(resolve_home_directory(environment(&[])), None);
    }
}
