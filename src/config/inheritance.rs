use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_yaml_ng::{Mapping, Value};

pub(super) fn load_with_inheritance(path: &Path, visited: &mut HashSet<PathBuf>) -> Result<Value> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("configuration file not found: {}", path.display()))?;
    if !visited.insert(canonical.clone()) {
        bail!("circular inherit_from detected at {}", canonical.display());
    }

    let contents = fs::read_to_string(&canonical)
        .with_context(|| format!("failed to read configuration: {}", canonical.display()))?;
    let mut current: Value = serde_yaml_ng::from_str(&contents)
        .with_context(|| format!("invalid YAML in {}", canonical.display()))?;
    let inherit = take_mapping_key(&mut current, "inherit_from");
    let inherit_gem = take_mapping_key(&mut current, "inherit_gem");
    let parent = canonical.parent().unwrap_or(Path::new("."));
    let mut inherited_paths = resolve_inherit_gems(inherit_gem)?;
    inherited_paths.extend(parse_inherit_paths(inherit)?);
    let mut merged = Value::Mapping(Mapping::new());

    for inherited in inherited_paths {
        if inherited.starts_with("http://") || inherited.starts_with("https://") {
            let mut remote_visited = HashSet::new();
            merged = merge_config(
                merged,
                load_remote_with_inheritance(&inherited, &mut remote_visited)?,
            );
            continue;
        }
        let inherited = PathBuf::from(inherited);
        let inherited = if inherited.is_absolute() {
            inherited
        } else {
            parent.join(inherited)
        };
        merged = merge_config(merged, load_with_inheritance(&inherited, visited)?);
    }

    visited.remove(&canonical);
    Ok(merge_config(merged, current))
}

fn load_remote_with_inheritance(url: &str, visited: &mut HashSet<String>) -> Result<Value> {
    if !visited.insert(url.to_owned()) {
        bail!("circular remote inherit_from detected at {url}");
    }
    let contents = fetch_remote_config(url)?;
    let mut current: Value =
        serde_yaml_ng::from_str(&contents).with_context(|| format!("invalid YAML from {url}"))?;
    let inherit = take_mapping_key(&mut current, "inherit_from");
    let mut merged = Value::Mapping(Mapping::new());
    for inherited in parse_inherit_paths(inherit)? {
        let inherited_url = if inherited.starts_with("http://") || inherited.starts_with("https://")
        {
            inherited
        } else {
            join_remote_url(url, &inherited)?
        };
        merged = merge_config(
            merged,
            load_remote_with_inheritance(&inherited_url, visited)?,
        );
    }
    visited.remove(url);
    Ok(merge_config(merged, current))
}

fn fetch_remote_config(url: &str) -> Result<String> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("failed to fetch remote configuration: {url}"))?;
    response
        .body_mut()
        .with_config()
        .limit(5 * 1024 * 1024)
        .read_to_string()
        .with_context(|| format!("failed to read remote configuration: {url}"))
}

fn join_remote_url(base: &str, relative: &str) -> Result<String> {
    if relative.starts_with('/') {
        let scheme_end = base
            .find("://")
            .context("remote configuration URL has no scheme")?
            + 3;
        let host_end = base[scheme_end..]
            .find('/')
            .map_or(base.len(), |offset| scheme_end + offset);
        return Ok(format!("{}{}", &base[..host_end], relative));
    }
    let directory_end = base
        .rfind('/')
        .context("remote configuration URL has no directory")?
        + 1;
    Ok(format!("{}{}", &base[..directory_end], relative))
}

fn parse_inherit_paths(value: Option<Value>) -> Result<Vec<String>> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::String(path)) => Ok(vec![path]),
        Some(Value::Sequence(values)) => values
            .into_iter()
            .map(|value| match value {
                Value::String(path) => Ok(path),
                _ => bail!("inherit_from entries must be paths"),
            })
            .collect(),
        Some(_) => bail!("inherit_from must be a path or a list of paths"),
    }
}

fn resolve_inherit_gems(value: Option<Value>) -> Result<Vec<String>> {
    let Some(Value::Mapping(gems)) = value else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    for (gem, values) in gems {
        let Some(gem) = gem.as_str() else {
            bail!("inherit_gem keys must be gem names");
        };
        let relative_paths = parse_inherit_paths(Some(values))?;
        for relative in relative_paths {
            let script = "spec = Gem::Specification.find_by_name(ARGV.shift); puts File.join(spec.full_gem_path, ARGV.shift)";
            let output = Command::new("ruby")
                .args(["-rrubygems", "-e", script, gem, &relative])
                .output()
                .with_context(|| format!("failed to resolve inherit_gem {gem}"))?;
            if !output.status.success() {
                bail!(
                    "failed to resolve inherit_gem {gem}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            let resolved = String::from_utf8(output.stdout)
                .with_context(|| format!("inherit_gem {gem} resolved to a non-UTF-8 path"))?;
            paths.push(resolved.trim().to_owned());
        }
    }
    Ok(paths)
}

fn take_mapping_key(value: &mut Value, key: &str) -> Option<Value> {
    value
        .as_mapping_mut()?
        .remove(Value::String(key.to_owned()))
}

pub(super) fn merge_config(base: Value, overlay: Value) -> Value {
    let global_merge = inherit_merge_keys(&overlay);
    match (base, overlay) {
        (Value::Mapping(mut base), Value::Mapping(overlay)) => {
            for (key, value) in overlay {
                let local_merge = inherit_merge_keys(&value);
                let merge_keys = if local_merge.is_empty() {
                    &global_merge
                } else {
                    &local_merge
                };
                // `map_or` evaluates its default eagerly, so every key paid for a deep
                // clone even though most keys are absent from `base`.
                let merged = match base.remove(&key) {
                    Some(old) => deep_merge(old, value, merge_keys),
                    None => value,
                };
                base.insert(key, merged);
            }
            Value::Mapping(base)
        }
        (_, overlay) => overlay,
    }
}

fn deep_merge(base: Value, overlay: Value, merge_keys: &HashSet<String>) -> Value {
    match (base, overlay) {
        (Value::Mapping(mut base), Value::Mapping(overlay)) => {
            for (key, value) in overlay {
                let should_merge_sequence =
                    key.as_str().is_some_and(|name| merge_keys.contains(name));
                let merged = match (base.remove(&key), value) {
                    (Some(Value::Sequence(mut old)), Value::Sequence(new))
                        if should_merge_sequence =>
                    {
                        old.extend(new);
                        Value::Sequence(old)
                    }
                    (Some(old), new) => deep_merge(old, new, merge_keys),
                    (None, new) => new,
                };
                base.insert(key, merged);
            }
            Value::Mapping(base)
        }
        (_, overlay) => overlay,
    }
}

fn inherit_merge_keys(value: &Value) -> HashSet<String> {
    let Some(mode) = value
        .as_mapping()
        .and_then(|mapping| mapping.get("inherit_mode"))
        .and_then(Value::as_mapping)
        .and_then(|mapping| mapping.get("merge"))
        .and_then(Value::as_sequence)
    else {
        return HashSet::new();
    };
    mode.iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_yaml_ng::Value;
    use tempfile::tempdir;

    use super::{deep_merge, join_remote_url, merge_config};
    use crate::config::Config;

    fn yaml(text: &str) -> Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    #[test]
    fn joins_remote_inherit_urls() {
        assert_eq!(
            join_remote_url("https://example.com/team/base.yml", "shared.yml").unwrap(),
            "https://example.com/team/shared.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com/team/base.yml", "/root.yml").unwrap(),
            "https://example.com/root.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com", "/root.yml").unwrap(),
            "https://example.com/root.yml"
        );
        assert!(join_remote_url("example.com/base.yml", "/root.yml").is_err());
        assert!(join_remote_url("no-directory", "shared.yml").is_err());
    }

    #[test]
    fn merges_configurations_without_losing_untouched_keys() {
        let merged = merge_config(
            yaml("Style/A:\n  Max: 1\nStyle/B:\n  Max: 2\n"),
            yaml("Style/A:\n  Enabled: false\nStyle/C:\n  Max: 3\n"),
        );
        assert_eq!(
            merged,
            yaml("Style/A:\n  Max: 1\n  Enabled: false\nStyle/B:\n  Max: 2\nStyle/C:\n  Max: 3\n")
        );
    }

    #[test]
    fn merge_replaces_sequences_unless_inherit_mode_requests_merging() {
        let replaced = merge_config(
            yaml("Style/A:\n  Exclude: [a.rb]\n"),
            yaml("Style/A:\n  Exclude: [b.rb]\n"),
        );
        assert_eq!(replaced, yaml("Style/A:\n  Exclude: [b.rb]\n"));

        let appended = merge_config(
            yaml("Style/A:\n  Exclude: [a.rb]\n"),
            yaml("inherit_mode:\n  merge:\n    - Exclude\nStyle/A:\n  Exclude: [b.rb]\n"),
        );
        assert_eq!(
            appended.as_mapping().unwrap().get("Style/A").unwrap(),
            &yaml("Exclude: [a.rb, b.rb]\n")
        );
    }

    #[test]
    fn merge_prefers_scalar_overlays_over_mappings() {
        assert_eq!(merge_config(yaml("A:\n  x: 1\n"), yaml("3")), yaml("3"));
        let keys = std::collections::HashSet::new();
        assert_eq!(deep_merge(yaml("[1, 2]"), yaml("[3]"), &keys), yaml("[3]"));
        assert_eq!(
            deep_merge(yaml("a: 1\n"), yaml("b: 2\n"), &keys),
            yaml("a: 1\nb: 2\n")
        );
    }

    #[test]
    fn loads_inherited_configuration_and_merges_selected_arrays() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join("base.yml"),
            "Layout/LineLength:\n  Max: 90\n  Exclude: [a.rb]\n",
        )
        .unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "inherit_from: base.yml\ninherit_mode:\n  merge:\n    - Exclude\nLayout/LineLength:\n  Enabled: false\n  Exclude: [b.rb]\n",
        )
        .unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        assert_eq!(
            config.cop_value::<usize>("Layout/LineLength", "Max"),
            Some(90)
        );
        assert_eq!(
            config.cop_value::<Vec<String>>("Layout/LineLength", "Exclude"),
            Some(vec!["a.rb".to_owned(), "b.rb".to_owned()])
        );
        assert!(!config.rule_enabled("Layout/LineLength"));
    }
}
