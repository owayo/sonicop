use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct MissingConfigError {
    path: String,
    color: bool,
}

impl fmt::Display for MissingConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Configuration file not found: {}", self.path)
    }
}

impl std::error::Error for MissingConfigError {}

impl MissingConfigError {
    pub(crate) fn with_color(mut error: anyhow::Error, color: bool) -> anyhow::Error {
        if let Some(missing) = error.downcast_mut::<Self>() {
            missing.color = color;
        }
        error
    }

    pub(crate) fn color(&self) -> bool {
        self.color
    }
}

#[derive(Debug)]
pub(crate) struct ConfigIoError {
    reason: &'static str,
    path: String,
}

impl fmt::Display for ConfigIoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} @ rb_sysopen - {}", self.reason, self.path)
    }
}

impl std::error::Error for ConfigIoError {}

/// Missing表示も通常のoriginも、File.absolute_pathの入力表記から展開する。
pub(super) fn absolute_config_error_origin(path: &Path) -> io::Result<PathBuf> {
    Ok(absolute_config_spelling(path, &std::env::current_dir()?))
}

fn absolute_config_spelling(path: &Path, cwd: &Path) -> PathBuf {
    let text = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let (input, prefix) = if let Some(unc) = text.strip_prefix("//?/UNC/") {
        (format!("//{unc}"), Some("UNC"))
    } else if let Some(path) = text.strip_prefix("//?/") {
        (path.to_owned(), Some("disk"))
    } else {
        (text, None)
    };
    #[cfg(not(windows))]
    let input = text;
    let full = crate::formatter::absolute_path(Path::new(&input), cwd);
    #[cfg(windows)]
    let full = match prefix {
        Some("UNC") => format!("//?/UNC/{}", full.trim_start_matches('/')),
        Some(_) => format!("//?/{full}"),
        None => full,
    }
    .replace('/', "\\");
    PathBuf::from(full)
}

pub(super) fn config_io_error(path: &Path, error: io::Error) -> anyhow::Error {
    // Windowsの共有違反32はUncategorizedになる。全Uncategorizedや全アクセス拒否は混ぜない。
    let permission = error.kind() == io::ErrorKind::PermissionDenied
        || (cfg!(windows) && error.raw_os_error() == Some(32));
    let directory = error.kind() == io::ErrorKind::IsADirectory
        || (cfg!(windows) && error.raw_os_error() == Some(5));
    if permission || directory {
        let metadata = fs::metadata(path).ok();
        // 他OSのACLによるPermissionDeniedを、Windowsのdirectory readへ読み替えない。
        let reason = if directory && metadata.as_ref().is_some_and(fs::Metadata::is_dir) {
            Some("Is a directory")
        } else if permission && metadata.as_ref().is_some_and(fs::Metadata::is_file) {
            Some("Permission denied")
        } else {
            None
        };
        if let Some(reason) = reason
            && let Ok(origin) = absolute_config_origin(path)
        {
            return ConfigIoError {
                reason,
                path: origin.to_string_lossy().replace('\\', "/"),
            }
            .into();
        }
    }
    if error.kind() != io::ErrorKind::NotFound {
        return error.into();
    }
    match absolute_config_error_origin(path) {
        Ok(path) => MissingConfigError {
            path: path.to_string_lossy().replace('\\', "/"),
            color: false,
        }
        .into(),
        Err(error) => error.into(),
    }
}

pub(super) fn canonicalize_config_path(
    path: &Path,
    error_origin: &Path,
) -> anyhow::Result<PathBuf> {
    // File.absolute_pathは存在しない中間directoryも..で消す。空文字はcwdへのdirectory I/Oになる。
    let path = absolute_config_origin(path)?;
    fs::canonicalize(path).map_err(|error| config_io_error(error_origin, error))
}

/// File.absolute_pathの親の短縮名・symlink表記を保ち、Windowsでは末尾の実名だけを戻す。
pub(super) fn absolute_config_origin(path: &Path) -> std::io::Result<PathBuf> {
    Ok(absolute_config_origin_at(path, &std::env::current_dir()?))
}

fn absolute_config_origin_at(path: &Path, cwd: &Path) -> PathBuf {
    let full = absolute_config_spelling(path, cwd);
    #[cfg(windows)]
    let full = if path
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("//?/")
    {
        // 本家は明示verbatimの末尾の大文字・8.3名も入力通りに残す。
        full
    } else {
        expand_existing_basename(full)
    };
    full
}

#[cfg(windows)]
fn expand_existing_basename(mut path: PathBuf) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;

    let input: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut output = vec![0; 32_768];
    let length =
        unsafe { GetLongPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    if length > 0 && length < output.len() as u32 {
        let expanded = PathBuf::from(OsString::from_wide(&output[..length as usize]));
        if let Some(name) = expanded.file_name() {
            // WindowsのFile.absolute_path/expand_pathは末尾だけ実名へ戻し、親の8.3名を保つ。
            path.set_file_name(name);
        }
    }
    path
}

pub(super) fn find_config_with_origin(start: &Path) -> Option<(PathBuf, PathBuf)> {
    let origin = absolute_config_origin(start).ok()?;
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let project_root = find_project_root(&start);

    for (depth, directory) in start.ancestors().enumerate() {
        let candidate = directory.join(".rubocop.yml");
        if candidate.is_file() {
            let spelling = origin
                .ancestors()
                .nth(depth)
                .map(|path| path.join(".rubocop.yml"));
            return config_location(&candidate, spelling.as_deref());
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
                let depth = start.ancestors().position(|directory| directory == root)?;
                let relative = candidate.strip_prefix(&root).ok()?;
                let spelling = origin
                    .ancestors()
                    .nth(depth)
                    .map(|path| path.join(relative));
                return config_location(&candidate, spelling.as_deref());
            }
        }
    }

    let home = home_directory();
    if let Some(candidate) = home.as_ref().map(|home| home.join(".rubocop.yml"))
        && candidate.is_file()
    {
        return config_location(&candidate, Some(&candidate));
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".config")));
    if let Some(candidate) = xdg.map(|root| root.join("rubocop/config.yml"))
        && candidate.is_file()
    {
        return config_location(&candidate, Some(&candidate));
    }
    None
}

/// 選択する設定は従来のcanonical探索で決め、同じfileを指す入力表記だけを表示用に残す。
fn config_location(path: &Path, origin: Option<&Path>) -> Option<(PathBuf, PathBuf)> {
    let canonical = fs::canonicalize(path).ok()?;
    let origin =
        origin.filter(|origin| fs::canonicalize(origin).is_ok_and(|path| path == canonical));
    let origin = if let Some(origin) = origin {
        absolute_config_origin(origin).ok()?
    } else {
        // APIが内部で足したprefixだけを除き、明示originのprefixと混ぜない。
        let full = canonical.to_string_lossy().replace('\\', "/");
        let full = if let Some(unc) = full.strip_prefix("//?/UNC/") {
            format!("//{unc}")
        } else {
            full.strip_prefix("//?/").unwrap_or(&full).to_owned()
        };
        absolute_config_origin(Path::new(&full)).ok()?
    };
    Some((canonical, origin))
}

pub(super) fn home_directory() -> Option<PathBuf> {
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

/// add_excludes_from_files は最寄りではなく、起動時の境界までで最後の dotfile を読む。
pub(super) fn parent_exclusion_config(
    config_path: &Path,
    origin: &Path,
    cwd: &Path,
) -> Option<PathBuf> {
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let boundary = find_project_root(&cwd);
    let mut last = None;
    for (depth, directory) in config_path.parent()?.ancestors().enumerate() {
        let candidate = directory.join(".rubocop.yml");
        if candidate.is_file() {
            let spelling = origin
                .parent()?
                .ancestors()
                .nth(depth)
                .map(|path| path.join(".rubocop.yml"));
            last = config_location(&candidate, spelling.as_deref());
        }
        if boundary.as_deref() == Some(directory) {
            break;
        }
    }
    last.filter(|(path, _)| path != config_path)
        .map(|(_, origin)| origin)
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

    #[cfg(windows)]
    fn windows_name(path: &Path, short: bool) -> String {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetLongPathNameW, GetShortPathNameW};
        let input: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut output = vec![0; 32_768];
        let length = unsafe {
            if short {
                GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32)
            } else {
                GetLongPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32)
            }
        };
        assert!(length > 0 && length < output.len() as u32);
        String::from_utf16(&output[..length as usize]).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn explicit_origins_match_ten_actual_windows_file_absolute_path_calls() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/verbatim_config_origins.json"
        ))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        for (name, body) in fixture["api_files"].as_object().unwrap() {
            std::fs::write(directory.path().join(name), body.as_str().unwrap()).unwrap();
        }
        let cwd = windows_name(directory.path(), false);
        let short = windows_name(
            &Path::new(&cwd).join("long-configuration-filename.yml"),
            true,
        );
        let short_file = Path::new(&short).file_name().unwrap().to_string_lossy();
        assert_ne!(
            short_file, "long-configuration-filename.yml",
            "実8.3名が無ければ未計測を成功にしない"
        );
        let bind = |text: &str| {
            text.replace("<CWD_FWD>", &cwd.replace('\\', "/"))
                .replace("<CWD>", &cwd)
                .replace("<SHORT_FILE>", &short_file)
        };
        let rows = fixture["api"].as_array().unwrap();
        assert_eq!(rows.len(), 10);
        for row in rows {
            let input = bind(row["input"].as_str().unwrap());
            let base = row["base"]
                .as_str()
                .map(bind)
                .unwrap_or_else(|| cwd.clone());
            let actual = super::absolute_config_origin_at(Path::new(&input), Path::new(&base));
            assert_eq!(
                actual.to_string_lossy().replace('\\', "/"),
                bind(row["absolute_path"].as_str().unwrap()),
                "{}",
                row["id"]
            );
        }
        // canonicalの内部prefixは通常参照へ戻し、同一fileへの明示originだけを残す。
        let config = Path::new(&cwd).join("config.yml");
        let generated = super::config_location(&config, None).unwrap();
        assert_eq!(
            generated.1.to_string_lossy().replace('\\', "/"),
            bind(rows[0]["absolute_path"].as_str().unwrap())
        );
        let explicit = std::path::PathBuf::from(format!(r"\\?\{}\config.yml", cwd));
        let requested = super::config_location(&config, Some(&explicit)).unwrap();
        assert_eq!(generated.0, requested.0);
        let measured = rows.iter().find(|row| row["id"] == "verbatim").unwrap();
        assert_eq!(
            requested.1.to_string_lossy().replace('\\', "/"),
            bind(measured["absolute_path"].as_str().unwrap())
        );
    }

    #[test]
    fn missing_configuration_keeps_its_body_through_context() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("absent/../missing.yml");
        let error = super::canonicalize_config_path(&path, &path)
            .unwrap_err()
            .context("outer resolution");
        let error = super::MissingConfigError::with_color(error, true);
        let missing = error.downcast_ref::<super::MissingConfigError>().unwrap();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/missing_config_errors.json"
        ))
        .unwrap();
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == "parent")
            .unwrap();
        let expected = case["error_message_template"].as_str().unwrap().replace(
            "{cwd}",
            &directory.path().to_string_lossy().replace('\\', "/"),
        );
        assert_eq!(missing.to_string(), expected);
        assert!(missing.color());
    }

    #[cfg(windows)]
    #[test]
    fn real_io_failures_match_five_actual_config_loader_api_results() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/config_io_error_headers.json"
        ))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        for (name, body) in fixture["api_files"].as_object().unwrap() {
            std::fs::write(directory.path().join(name), body.as_str().unwrap()).unwrap();
        }
        std::fs::create_dir(directory.path().join("configdir")).unwrap();
        let cwd = windows_name(directory.path(), false);
        let bind = |text: &str| {
            text.replace("<CWD_FWD>", &cwd.replace('\\', "/"))
                .replace("<CWD>", &cwd)
        };
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(directory.path().join("locked.yml"))
            .unwrap();
        let rows = fixture["api"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        for row in rows {
            let path = bind(row["path"].as_str().unwrap());
            let actual = std::fs::read_to_string(&path);
            if row["kind"] == "value" {
                assert_eq!(actual.unwrap().as_bytes(), b"{}\n");
            } else {
                let error = super::config_io_error(Path::new(&path), actual.unwrap_err())
                    .context("outer reader");
                let message = if row["class"] == "RuboCop::ConfigNotFoundError" {
                    error
                        .downcast_ref::<super::MissingConfigError>()
                        .unwrap()
                        .to_string()
                } else {
                    error
                        .downcast_ref::<super::ConfigIoError>()
                        .unwrap()
                        .to_string()
                };
                assert_eq!(
                    message,
                    bind(row["message"].as_str().unwrap()),
                    "{}",
                    row["id"]
                );
            }
        }
        drop(guard);
        // 属性不明や別のI/O故障を、確認できたdirectory/通常fileの診断へ誤変換しない。
        let path = directory.path().join("readable.yml");
        for kind in [
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::NotADirectory,
        ] {
            let error = super::config_io_error(&path, std::io::Error::new(kind, "kept"));
            assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(), kind);
        }
        let error = super::config_io_error(
            &directory.path().join("configdir"),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "kept"),
        );
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().to_string(),
            "kept"
        );
    }

    #[test]
    fn other_io_errors_and_empty_directory_input_are_not_missing_files() {
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::NotADirectory,
        ] {
            let error = super::config_io_error(
                Path::new("config.yml"),
                std::io::Error::new(kind, "preserved IO detail"),
            );
            assert!(error.downcast_ref::<super::MissingConfigError>().is_none());
            assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(), kind);
            assert_eq!(error.to_string(), "preserved IO detail");
        }
        let directory = super::canonicalize_config_path(Path::new(""), Path::new("")).unwrap();
        assert!(directory.is_dir());
        let error = super::config_io_error(
            Path::new(""),
            std::fs::read_to_string(directory).unwrap_err(),
        );
        assert!(error.downcast_ref::<super::MissingConfigError>().is_none());
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
