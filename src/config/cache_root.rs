use std::path::Path;

use anyhow::{Context, Result};

use super::{Config, ConfigWarnings};

#[cfg(all(test, windows))]
#[path = "../../tests/support/cache_root_paths.rs"]
mod cache_root_paths;

pub(crate) fn report(
    override_root: Option<&Path>,
    cwd: &Path,
    desired: Option<&str>,
    warnings: &ConfigWarnings,
) -> Result<()> {
    // RemoteConfigが先にrootを固定した場合、CLI上書き前の値を比較へ残す。
    let cached = warnings
        .remote_cache_root
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if cached.is_none()
        && (override_root.is_some() || std::env::var_os("RUBOCOP_CACHE_ROOT").is_some())
    {
        return Ok(());
    }
    let early = if cached.is_none() {
        Config::toplevel_cache_root(cwd)?
    } else {
        None
    };
    let environment = std::env::var("RUBOCOP_CACHE_ROOT").ok();
    let override_root = override_root.map(|path| path.to_string_lossy().into_owned());
    let desired = environment
        .as_deref()
        .or(override_root.as_deref())
        .or(desired);
    if cached.is_none() && early.as_deref() == desired {
        return Ok(());
    }
    let xdg = std::env::var("XDG_CACHE_HOME").ok();
    let home = if xdg.is_none() && ((cached.is_none() && early.is_none()) || desired.is_none()) {
        let home = Config::cache_home_directory().context("could not find home directory")?;
        Some(join(&realpath(&home.to_string_lossy())?, ".cache"))
    } else {
        None
    };
    let root = |value| root_dir(value, xdg.as_deref(), home.as_deref(), user_id());
    let early = cached.unwrap_or_else(|| root(early.as_deref()));
    let desired = root(desired);
    // act_on_optionsは比較前に有効rootを再設定し、後の入れ子設定にも引き継ぐ。
    *warnings
        .remote_cache_root
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(desired.clone());
    if early == desired || same_real_path(&early, &desired) {
        return Ok(());
    }
    warnings.emit_cache_root(format!(
        "Warning: Remote configuration cache files were stored in `{early}` because a desired cache root (`{desired}`) was not set at the top level. Consider setting `AllCops: CacheRootDirectory` in your toplevel configuration file, using the `RUBOCOP_CACHE_ROOT` environment variable, or using CLI options."
    ))
}

pub(super) fn early_root(override_root: Option<&Path>, cwd: &Path) -> Result<String> {
    // RemoteConfigは有効設定の解決が終わる前にrootを要求するため、同じ軽量lookupを使う。
    let configured = std::env::var("RUBOCOP_CACHE_ROOT")
        .ok()
        .or_else(|| override_root.map(|path| path.to_string_lossy().into_owned()));
    let configured = match configured {
        Some(root) => Some(root),
        None => Config::toplevel_cache_root(cwd)?,
    };
    let xdg = std::env::var("XDG_CACHE_HOME").ok();
    let home = if configured.is_none() && xdg.is_none() {
        let home = Config::cache_home_directory().context("could not find home directory")?;
        Some(join(&realpath(&home.to_string_lossy())?, ".cache"))
    } else {
        None
    };
    Ok(root_dir(
        configured.as_deref(),
        xdg.as_deref(),
        home.as_deref(),
        user_id(),
    ))
}

pub(super) fn for_remote(warnings: Option<&ConfigWarnings>, cwd: &Path) -> Result<String> {
    let Some(warnings) = warnings else {
        return early_root(None, cwd);
    };
    if let Some(root) = warnings
        .remote_cache_root
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return Ok(root);
    }
    let root = early_root(warnings.cache_root_override.as_deref(), cwd)?;
    let mut cached = warnings
        .remote_cache_root
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(cached.get_or_insert(root).clone())
}

fn root_dir(configured: Option<&str>, xdg: Option<&str>, home: Option<&str>, uid: u32) -> String {
    let root = configured.map(str::to_owned).unwrap_or_else(|| {
        xdg.map_or_else(
            || home.unwrap().to_owned(),
            |root| join(root, &uid.to_string()),
        )
    });
    join(&root, "rubocop_cache")
}

fn join(root: &str, child: &str) -> String {
    // File.joinはrootの末尾separatorを残し、空文字でも先頭にslashを置く。
    if root.ends_with('/') || cfg!(windows) && root.ends_with('\\') {
        format!("{root}{child}")
    } else {
        format!("{root}/{child}")
    }
}

fn same_real_path(early: &str, desired: &str) -> bool {
    matches!((realpath(early), realpath(desired)), (Ok(early), Ok(desired)) if early == desired)
}

#[cfg(not(windows))]
fn realpath(path: &str) -> Result<String> {
    Ok(std::fs::canonicalize(path)?.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn realpath(path: &str) -> Result<String> {
    let cwd = std::env::current_dir()?
        .to_string_lossy()
        .replace('\\', "/");
    realpath_from(path, &cwd)
}

#[cfg(windows)]
fn realpath_from(path: &str, cwd: &str) -> Result<String> {
    use std::collections::VecDeque;

    // MRIのFile.realpathは通常componentのcase/8.3/drive/prefixを保持する。
    // canonicalizeはそれらを消す上、junction/..をリンク解決前に畳むので使えない。
    let path = path.replace('\\', "/");
    let absolute = if path.starts_with("//") || path.as_bytes().get(1) == Some(&b':') {
        path
    } else if path.starts_with('/') {
        format!("{}{path}", &cwd[..2])
    } else {
        format!("{cwd}/{path}")
    };
    let (mut root, rest) = split_root(&absolute)?;
    let mut parts = Vec::<String>::new();
    let mut pending: VecDeque<String> = rest.split('/').map(str::to_owned).collect();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                parts.pop();
                continue;
            }
            _ => {}
        }
        let candidate = format!(
            "{root}{}",
            parts
                .iter()
                .chain(std::iter::once(&part))
                .cloned()
                .collect::<Vec<_>>()
                .join("/")
        );
        let native = candidate.replace('/', "\\");
        let metadata = std::fs::symlink_metadata(&native)?;
        if metadata.file_type().is_symlink() {
            links += 1;
            anyhow::ensure!(links <= 40, "too many symbolic links");
            let target = std::fs::read_link(&native)?
                .to_string_lossy()
                .replace('\\', "/");
            let target_parts =
                if target.starts_with("//") || target.as_bytes().get(1) == Some(&b':') {
                    let (target_root, rest) = split_root(&target)?;
                    root = target_root;
                    parts.clear();
                    rest.to_owned()
                } else {
                    target
                };
            for component in target_parts.split('/').rev() {
                pending.push_front(component.to_owned());
            }
        } else {
            anyhow::ensure!(metadata.is_dir() || pending.is_empty(), "not a directory");
            parts.push(part);
        }
    }
    let result = format!("{root}{}", parts.join("/"));
    std::fs::metadata(result.replace('/', "\\"))?;
    Ok(result)
}

#[cfg(windows)]
fn split_root(path: &str) -> Result<(String, &str)> {
    let drive = if path.starts_with("//?/") { 4 } else { 0 };
    if path.as_bytes().get(drive + 1) == Some(&b':')
        && path.as_bytes().get(drive + 2) == Some(&b'/')
    {
        return Ok((path[..drive + 3].to_owned(), &path[drive + 3..]));
    }
    if let Some(unc) = path.strip_prefix("//") {
        let mut separators = unc.match_indices('/');
        if separators.next().is_some()
            && let Some((end, _)) = separators.next()
        {
            return Ok((path[..end + 3].to_owned(), &path[end + 3..]));
        }
    }
    anyhow::bail!("unsupported absolute cache path: {path}")
}

#[cfg(windows)]
fn user_id() -> u32 {
    0
}

#[cfg(unix)]
fn user_id() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // Process.uidは実ユーザーIDを用いる。
    unsafe { getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_realpath_matches_sixteen_actual_ruby_paths_and_eleven_pairs() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/cache_root_warning.json"))
                .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let bindings = cache_root_paths::setup(folder.path());
        let cwd = &bindings["WORK_FWD"];
        let mut paths = 0;
        let mut pairs = 0;
        for group in fixture["realpath_api"].as_object().unwrap().values() {
            let rows = group["paths"].as_object().unwrap();
            for (name, row) in rows {
                let path = cache_root_paths::bind(row["path"].as_str().unwrap(), &bindings);
                let expected = cache_root_paths::bind(row["realpath"].as_str().unwrap(), &bindings);
                assert_eq!(
                    realpath_from(&path, cwd).unwrap(),
                    expected,
                    "{name}: {path}"
                );
                paths += 1;
            }
            for pair in group["pairs"].as_array().unwrap() {
                let resolve = |key: &str| {
                    let path = rows[pair[key].as_str().unwrap()]["path"].as_str().unwrap();
                    realpath_from(&cache_root_paths::bind(path, &bindings), cwd).unwrap()
                };
                assert_eq!(
                    resolve("early") == resolve("desired"),
                    pair["warning_string"].as_str().unwrap().is_empty(),
                    "{pair}"
                );
                pairs += 1;
            }
        }
        assert_eq!(cache_root_paths::root("long", &bindings), bindings["LONG"]);
        assert_eq!((paths, pairs), (16, 11));
    }

    #[test]
    fn cache_root_join_preserves_ruby_separator_spelling() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/cache_root_warning.json"))
                .unwrap();
        for case in fixture["path_cases"].as_array().unwrap() {
            let input = case["input"].as_str().unwrap();
            if !cfg!(windows) && input.contains('\\') {
                continue;
            }
            assert_eq!(
                root_dir(Some(input), None, None, 0),
                case["configured"].as_str().unwrap()
            );
            assert_eq!(
                root_dir(None, Some(input), None, 0),
                case["xdg"].as_str().unwrap()
            );
        }
    }
}
