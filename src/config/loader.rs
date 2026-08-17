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
        // `break if dir == stop_dir || dir == FileFinder.root_level`
        //
        // **`stop_dir` が無いことは「止まらない」であって「止まる」ではない。**
        // 上流の `find_project_dotfile` は `find_file_upwards(DOTFILE, target_dir, project_root)`
        // で、`project_root` が nil のときは何とも一致しないので**ファイルシステムの根まで
        // 昇る**。ここを `is_none_or` で書いていたため、Gemfile の無い木では最初の 1 段で
        // 止まり、**`.rubocop.yml` がリポジトリ直下・コードが `lib/` という標準の配置で
        // 設定が 1 つも効かなかった。**
        //
        // `ancestors()` は根で終わるので、`root_level` の側は自然に満たされる。
        if project_root.as_deref() == Some(directory) {
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

/// `base_dir_for_path_parameters`: where the paths a configuration file mentions are taken from.
///
/// A file whose name starts with `.rubocop` speaks about the directory holding it; any other file
/// speaks about the directory the command ran in. The gem's own `default.yml` is the reason for the
/// split -- its paths must not be read as relative to the gem. The dotfile in the home directory
/// describes whatever is being inspected rather than the home directory, so it counts as an other.
pub(super) fn path_parameter_base_directory<'a>(
    config_path: Option<&'a Path>,
    cwd: &'a Path,
) -> &'a Path {
    let home = home_directory();
    base_directory_for(config_path, cwd, home.as_deref())
}

fn base_directory_for<'a>(
    config_path: Option<&'a Path>,
    cwd: &'a Path,
    home: Option<&Path>,
) -> &'a Path {
    let Some(path) = config_path else {
        return cwd;
    };
    let named_for_rubocop = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".rubocop"));
    if !named_for_rubocop || is_home_dotfile(path, home) {
        return cwd;
    }
    path.parent().unwrap_or(cwd)
}

/// The configuration path is stored canonicalized, so the dotfile is compared both as written and
/// as the file system resolves it.
fn is_home_dotfile(path: &Path, home: Option<&Path>) -> bool {
    home.is_some_and(|home| {
        let dotfile = home.join(".rubocop.yml");
        path == dotfile
            || fs::canonicalize(&dotfile).is_ok_and(|canonical| path == canonical.as_path())
    })
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
    use std::path::Path;

    use super::{base_directory_for, resolve_home_directory};

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

    #[test]
    fn takes_path_parameters_from_the_directory_only_for_a_rubocop_dotfile() {
        let cwd = Path::new("/work/project");
        let home = Some(Path::new("/home/dev"));
        // Nothing loaded: the command's own directory is all there is.
        assert_eq!(base_directory_for(None, cwd, home), cwd);
        // `.rubocop.yml` and its siblings speak about where they sit.
        assert_eq!(
            base_directory_for(Some(Path::new("/work/project/ci/.rubocop.yml")), cwd, home),
            Path::new("/work/project/ci")
        );
        assert_eq!(
            base_directory_for(
                Some(Path::new("/work/project/.rubocop_todo.yml")),
                cwd,
                home
            ),
            Path::new("/work/project")
        );
        // Any other name is read against the directory the command ran in, so a configuration kept
        // outside the project does not drag the target Ruby version out of its own neighbourhood.
        assert_eq!(
            base_directory_for(Some(Path::new("/tmp/checks/all609.yml")), cwd, home),
            cwd
        );
        // The dotfile in the home directory describes the project, not the home directory.
        assert_eq!(
            base_directory_for(Some(Path::new("/home/dev/.rubocop.yml")), cwd, home),
            cwd
        );
    }
}
