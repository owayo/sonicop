use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use super::Config;
use super::loader::find_config;

type ConfigCache = Mutex<HashMap<PathBuf, Arc<Config>>>;

#[derive(Debug)]
pub struct ConfigStore {
    root: Arc<Config>,
    discover_per_path: bool,
    ignore_unrecognized: bool,
    directories: ConfigCache,
    cache: ConfigCache,
}

impl ConfigStore {
    pub fn new(config: Config, discover_per_path: bool, ignore_unrecognized: bool) -> Self {
        Self {
            root: Arc::new(config),
            discover_per_path,
            ignore_unrecognized,
            directories: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Config {
        &self.root
    }

    pub fn for_path(&self, path: &Path) -> Result<Arc<Config>> {
        if !self.discover_per_path {
            return Ok(Arc::clone(&self.root));
        }

        let start = if path.is_dir() {
            path
        } else {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
        };
        // `find_config` canonicalizes and stats every ancestor directory, so keying only
        // the resolved configuration path made every file pay that cost again. Directories
        // are far fewer than the files below them, so memoize on the directory itself.
        if let Some(config) = cached(&self.directories, start)? {
            return Ok(config);
        }

        let discovered = find_config(start);
        let config = if discovered.as_deref() == self.root.config_path() {
            Arc::clone(&self.root)
        } else {
            let key = discovered
                .clone()
                .unwrap_or_else(|| fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf()));
            match cached(&self.cache, &key)? {
                Some(config) => config,
                None => {
                    let config = Arc::new(Config::load(discovered.as_deref(), start)?);
                    if !self.ignore_unrecognized && !config.unrecognized_cop_names().is_empty() {
                        bail!(
                            "unrecognized cop(s): {}",
                            config.unrecognized_cop_names().join(", ")
                        );
                    }
                    store(&self.cache, key, &config)?;
                    config
                }
            }
        };
        store(&self.directories, start.to_path_buf(), &config)?;
        Ok(config)
    }
}

fn cached(cache: &ConfigCache, key: &Path) -> Result<Option<Arc<Config>>> {
    Ok(lock(cache)?.get(key).cloned())
}

fn store(cache: &ConfigCache, key: PathBuf, config: &Arc<Config>) -> Result<()> {
    lock(cache)?.insert(key, Arc::clone(config));
    Ok(())
}

fn lock(cache: &ConfigCache) -> Result<std::sync::MutexGuard<'_, HashMap<PathBuf, Arc<Config>>>> {
    cache
        .lock()
        .map_err(|_| anyhow::anyhow!("configuration cache lock is poisoned"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{Config, ConfigStore};

    #[test]
    fn store_resolves_nested_configuration_from_target_path() {
        let directory = tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "AllCops:\n  DisabledByDefault: true\nLayout/TrailingWhitespace:\n  Enabled: true\n",
        )
        .unwrap();
        fs::write(
            nested.join(".rubocop.yml"),
            "inherit_from: ../.rubocop.yml\nLayout/TrailingWhitespace:\n  Enabled: false\n",
        )
        .unwrap();

        let root = Config::load(None, directory.path()).unwrap();
        let store = ConfigStore::new(root, true, false);
        let root_config = store.for_path(&directory.path().join("root.rb")).unwrap();
        let nested_config = store.for_path(&nested.join("nested.rb")).unwrap();

        assert!(root_config.rule_enabled("Layout/TrailingWhitespace"));
        assert!(!nested_config.rule_enabled("Layout/TrailingWhitespace"));
        assert_eq!(
            nested_config.config_path(),
            Some(
                nested
                    .join(".rubocop.yml")
                    .canonicalize()
                    .unwrap()
                    .as_path()
            )
        );
    }
}
