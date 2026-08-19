mod inheritance;
mod loader;
mod paths;
mod plugin;
mod store;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_yaml_ng::{Mapping, Value};

use crate::cop_name;
use crate::ruby_version::{
    ResolvedTargetRuby, RubyVersion, resolve_target_ruby, validate_supported,
};

use inheritance::{load_with_inheritance, merge_config};
use loader::{find_config, path_parameter_base_directory};
use paths::{PathPatterns, compile_excludes, compile_includes, cop_patterns, has_hidden_component};
use plugin::{belongs_to_plugin, configured_plugin_departments};

pub use store::ConfigStore;

const DEFAULT_CONFIG: &str = include_str!("../../config/default.yml");

#[derive(Clone, Debug)]
pub struct Config {
    raw: Value,
    user: Value,
    /// Where the paths a configuration file mentions are taken from: the directory holding a
    /// `.rubocop*` file, or the one the run was launched from for any other name. RuboCop calls this
    /// `base_dir_for_path_parameters`, and every `Include` and `Exclude` is resolved against it.
    path_base: PathBuf,
    /// Where the run was launched from. RuboCop shortens paths it prints inside offense messages
    /// against `Dir.pwd`, not against the project root, so the two cannot be collapsed.
    cwd: PathBuf,
    config_path: Option<PathBuf>,
    target_ruby: ResolvedTargetRuby,
    known_cops: HashSet<String>,
    unrecognized_cops: Vec<String>,
    includes: PathPatterns,
    excludes: HashMap<String, PathPatterns>,
    cop_includes: HashMap<String, PathPatterns>,
}

impl Config {
    pub fn load(explicit: Option<&Path>, cwd: &Path) -> Result<Self> {
        Self::load_with_options(explicit, cwd, false)
    }

    pub fn load_with_options(
        explicit: Option<&Path>,
        cwd: &Path,
        force_default: bool,
    ) -> Result<Self> {
        let default: Value = serde_yaml_ng::from_str(DEFAULT_CONFIG)
            .context("embedded RuboCop default configuration is invalid")?;
        let mut known_cops = cop_names(&default);
        let config_path = if force_default {
            None
        } else {
            match explicit {
                Some(path) => Some(fs::canonicalize(path).with_context(|| {
                    format!("configuration file not found: {}", path.display())
                })?),
                None => find_config(cwd),
            }
        };

        let (raw, user, unrecognized_cops) = if let Some(path) = &config_path {
            let mut visited = HashSet::new();
            let user = load_with_inheritance(path, &mut visited)?;
            let configured_cops = cop_names(&user);
            let plugin_departments = configured_plugin_departments(&user);
            let plugin_cops = configured_cops
                .iter()
                .filter(|name| belongs_to_plugin(name, &plugin_departments))
                .cloned()
                .collect::<HashSet<_>>();
            let mut unknown = configured_cops
                .difference(&known_cops)
                .filter(|name| !plugin_cops.contains(*name))
                .cloned()
                .collect::<Vec<_>>();
            unknown.sort();
            known_cops.extend(plugin_cops);
            (merge_config(default, user.clone()), user, unknown)
        } else {
            (default, Value::Mapping(Mapping::new()), Vec::new())
        };

        if all_cops_bool(&raw, "EnabledByDefault") && all_cops_bool(&raw, "DisabledByDefault") {
            bail!("AllCops/EnabledByDefault and AllCops/DisabledByDefault cannot both be true");
        }

        let path_base = path_parameter_base_directory(config_path.as_deref(), cwd);
        let configured_target = configured_target_ruby(&raw)?;
        let target_ruby = resolve_target_ruby(configured_target, path_base)?;
        validate_supported(target_ruby.version)?;

        let includes = cop_patterns(&raw, "AllCops", "Include").unwrap_or_default();
        let excludes = compile_excludes(&raw);
        let cop_includes = compile_includes(&raw);

        Ok(Self {
            raw,
            user,
            path_base: path_base.to_path_buf(),
            cwd: cwd.to_path_buf(),
            config_path,
            target_ruby,
            known_cops,
            unrecognized_cops,
            includes,
            excludes,
            cop_includes,
        })
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    pub fn target_ruby_version(&self) -> RubyVersion {
        self.target_ruby.version
    }

    pub fn display_cop_names(&self) -> bool {
        self.all_cops_value("DisplayCopNames").unwrap_or(true)
    }

    pub fn rule_enabled(&self, name: &str) -> bool {
        self.rule_enabled_with_pending(name, false, false)
    }

    pub fn rule_enabled_with_pending(
        &self,
        name: &str,
        enable_pending: bool,
        disable_pending: bool,
    ) -> bool {
        if name == "Lint/Syntax" {
            return true;
        }

        let configured = self.user_cop_mapping(name);
        let configured_enabled = configured.and_then(|cop| cop.get("Enabled"));
        let department = self.user_department_mapping(name);
        let department_enabled = department.and_then(|cop| cop.get("Enabled"));

        // An explicitly enabled cop overrides a disabled department.
        if configured_enabled == Some(&Value::Bool(true)) {
            return true;
        }
        if department_enabled == Some(&Value::Bool(false)) {
            return false;
        }

        if self.all_cops_bool_value("DisabledByDefault") {
            if let Some(configured) = configured {
                return configured.get("Enabled").is_none_or(|enabled| {
                    self.resolve_enabled_value(enabled, name, enable_pending, disable_pending)
                });
            }
            if department_enabled == Some(&Value::Bool(true)) {
                return self.default_enabled(name, enable_pending, disable_pending);
            }
            return false;
        }

        if self.all_cops_bool_value("EnabledByDefault") {
            return configured_enabled.is_none_or(|enabled| {
                self.resolve_enabled_value(enabled, name, enable_pending, disable_pending)
            });
        }

        configured_enabled.map_or_else(
            || self.default_enabled(name, enable_pending, disable_pending),
            |enabled| self.resolve_enabled_value(enabled, name, enable_pending, disable_pending),
        )
    }

    pub fn rule_safe(&self, name: &str) -> bool {
        self.cop_raw_value(name, "Safe")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn rule_safe_autocorrect(&self, name: &str) -> bool {
        self.cop_raw_value(name, "SafeAutoCorrect")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn cop_value<T: DeserializeOwned>(&self, name: &str, key: &str) -> Option<T> {
        // Deserialized from the borrowed value rather than a clone of it. Every cop reads its
        // settings once per file, so a clone here is one deep copy of the value per cop per file
        // -- and the values are not all small: `Lint/Debugger`'s method list is a nested map.
        //
        // Measured 2026-08-19: this cuts the profiler's per-cop CPU total from 44.4s to 17.7s on
        // rubocop/rubocop, but an A/B of the two binaries showed **no wall-clock difference**
        // outside the noise. The saving is real and the copy was pointless, so it stays -- but it
        // is not what the run is waiting on.
        T::deserialize(self.cop_raw_value(name, key)?).ok()
    }

    pub fn all_cops_value<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.cop_value("AllCops", key)
    }

    /// `prevent_directive_disabling?`: whether the configuration explicitly turns on the cop that
    /// forbids directive comments, which is what puts that cop out of their reach.
    pub fn prevents_directive_disabling(&self) -> bool {
        self.cop_value::<bool>("Style/DisableCopsWithinSourceCodeDirective", "Enabled")
            == Some(true)
    }

    pub fn path_included(&self, path: &Path) -> bool {
        // RuboCop shortcuts a path whose *first* component is hidden: no `Include` pattern can reach
        // it unless one of them mentions a dot. Only the leading component counts, so a dot
        // directory nested under a visible one -- `docs/.mdl_style.rb` -- is still matched normally.
        if self.top_level_hidden(path) && !self.possibly_include_hidden() {
            return false;
        }
        self.includes.is_empty() || self.includes.matches_includes(path, &self.path_base)
    }

    fn top_level_hidden(&self, path: &Path) -> bool {
        paths::project_relative(path, &self.path_base).is_some_and(|relative| {
            relative
                .components()
                .next()
                .and_then(|component| component.as_os_str().to_str())
                .is_some_and(|first| first.starts_with('.') && first != "..")
        })
    }

    /// Whether a *directory* is one the configuration excludes wholesale, which is how RuboCop
    /// prunes `.git` and `node_modules` before descending rather than filtering their contents one
    /// file at a time.
    pub fn directory_excluded(&self, path: &Path) -> bool {
        let patterns: Vec<String> = self.all_cops_value("Exclude").unwrap_or_default();
        let Some(relative) = paths::project_relative(path, &self.path_base) else {
            return false;
        };
        patterns.iter().any(|pattern| {
            pattern
                .strip_suffix("/**/*")
                .is_some_and(|directory| relative == Path::new(directory))
        })
    }

    pub fn path_excluded(&self, path: &Path) -> bool {
        self.excluded_by("AllCops", path)
    }

    pub fn possibly_include_hidden(&self) -> bool {
        let patterns: Vec<String> = self.all_cops_value("Include").unwrap_or_default();
        patterns
            .iter()
            .any(|pattern| pattern.starts_with('.') || pattern.contains("/."))
    }

    pub fn path_hidden(&self, path: &Path) -> bool {
        let relative =
            paths::project_relative(path, &self.path_base).unwrap_or_else(|| path.to_path_buf());
        let relative = relative.as_path();
        has_hidden_component(relative)
    }

    pub fn rule_excluded(&self, name: &str, path: &Path) -> bool {
        self.excluded_by(name, path)
    }

    /// Whether `name`'s own `Include` list reaches `path`.
    ///
    /// This is the `Include` half of RuboCop's `Cop::Base#relevant_file?`, which is what keeps the
    /// `Bundler` and `Gemspec` cops on the files their configuration names -- `**/Gemfile`,
    /// `**/*.gemspec` -- rather than on every Ruby file the run targets. A cop that names no
    /// `Include` applies to all of them, and one whose list is empty to none.
    pub fn rule_included(&self, name: &str, path: &Path) -> bool {
        self.cop_includes
            .get(name)
            .is_none_or(|patterns| patterns.matches_includes(path, &self.path_base))
    }

    fn excluded_by(&self, name: &str, path: &Path) -> bool {
        self.excludes
            .get(name)
            .is_some_and(|patterns| patterns.matches_any(path, &self.path_base))
    }

    pub fn known_cop_names(&self) -> impl Iterator<Item = &str> {
        self.known_cops.iter().map(String::as_str)
    }

    pub fn unrecognized_cop_names(&self) -> &[String] {
        &self.unrecognized_cops
    }

    pub fn description(&self, name: &str) -> Option<String> {
        self.cop_value(name, "Description")
    }

    fn cop_mapping(&self, name: &str) -> Option<&Mapping> {
        self.raw.as_mapping()?.get(name)?.as_mapping()
    }

    fn cop_raw_value(&self, name: &str, key: &str) -> Option<&Value> {
        self.cop_mapping(name)?.get(key)
    }

    fn user_cop_mapping(&self, name: &str) -> Option<&Mapping> {
        self.user.as_mapping()?.get(name)?.as_mapping()
    }

    fn user_department_mapping(&self, name: &str) -> Option<&Mapping> {
        self.user
            .as_mapping()?
            .get(cop_name::department(name))?
            .as_mapping()
    }

    fn default_enabled(&self, name: &str, enable_pending: bool, disable_pending: bool) -> bool {
        self.cop_raw_value(name, "Enabled").is_none_or(|enabled| {
            self.resolve_enabled_value(enabled, name, enable_pending, disable_pending)
        })
    }

    fn resolve_enabled_value(
        &self,
        enabled: &Value,
        name: &str,
        enable_pending: bool,
        disable_pending: bool,
    ) -> bool {
        match enabled {
            Value::Bool(value) => *value,
            Value::String(value) if value == "pending" => {
                if enable_pending {
                    true
                } else if disable_pending {
                    false
                } else {
                    let department_new_cops = self
                        .cop_raw_value(cop_name::department(name), "NewCops")
                        .and_then(Value::as_str);
                    department_new_cops.map_or_else(
                        || {
                            self.cop_raw_value("AllCops", "NewCops")
                                .and_then(Value::as_str)
                                == Some("enable")
                        },
                        |setting| setting == "enable",
                    )
                }
            }
            _ => true,
        }
    }

    fn all_cops_bool_value(&self, key: &str) -> bool {
        self.cop_raw_value("AllCops", key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

fn all_cops_bool(config: &Value, key: &str) -> bool {
    all_cops_mapping(config)
        .and_then(|mapping| mapping.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn all_cops_mapping(config: &Value) -> Option<&Mapping> {
    config.as_mapping()?.get("AllCops")?.as_mapping()
}

fn configured_target_ruby(config: &Value) -> Result<Option<RubyVersion>> {
    let value = all_cops_mapping(config).and_then(|mapping| mapping.get("TargetRubyVersion"));
    let Some(value) = value else {
        return Ok(None);
    };
    let text = match value {
        Value::Null => return Ok(None),
        Value::Number(number) => number.to_string(),
        Value::String(string) => string.clone(),
        _ => bail!("AllCops/TargetRubyVersion must be a major.minor version"),
    };
    RubyVersion::parse(&text)
        .map(Some)
        .with_context(|| format!("invalid AllCops/TargetRubyVersion: {text}"))
}

fn cop_names(value: &Value) -> HashSet<String> {
    value
        .as_mapping()
        .into_iter()
        .flat_map(Mapping::keys)
        .filter_map(Value::as_str)
        .filter(|name| name.contains('/'))
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::Config;

    #[test]
    fn recognizes_all_upstream_cops() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        assert_eq!(config.known_cop_names().count(), 609);
        assert!(!config.rule_enabled("Style/ArrayFirstLast"));
    }

    #[test]
    fn disabled_by_default_enables_only_configured_cops() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "AllCops:\n  DisabledByDefault: true\nLayout/TrailingWhitespace:\n  Enabled: true\nStyle/StringLiterals:\n  EnforcedStyle: double_quotes\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        assert!(config.rule_enabled("Lint/Syntax"));
        assert!(config.rule_enabled("Layout/TrailingWhitespace"));
        assert!(config.rule_enabled("Style/StringLiterals"));
        assert!(!config.rule_enabled("Layout/SpaceAfterComma"));
    }

    #[test]
    fn explicitly_enabled_cop_overrides_disabled_department() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "Layout:\n  Enabled: false\nLayout/TrailingWhitespace:\n  Enabled: true\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        assert!(config.rule_enabled("Layout/TrailingWhitespace"));
        assert!(!config.rule_enabled("Layout/SpaceAfterComma"));
    }

    #[test]
    fn enabled_by_default_preserves_explicit_disables() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "AllCops:\n  EnabledByDefault: true\nStyle/ArrayFirstLast:\n  Enabled: false\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        assert!(config.rule_enabled("Style/HashSyntax"));
        assert!(!config.rule_enabled("Style/ArrayFirstLast"));
    }

    #[test]
    fn still_reports_unknown_core_cops() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "Style/DefinitelyNotACop:\n  Enabled: true\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        assert_eq!(
            config.unrecognized_cop_names(),
            &["Style/DefinitelyNotACop"]
        );
    }

    #[test]
    fn rejects_conflicting_default_modes() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "AllCops:\n  EnabledByDefault: true\n  DisabledByDefault: true\n",
        )
        .unwrap();

        assert!(Config::load(None, directory.path()).is_err());
    }

    #[test]
    fn relative_excludes_do_not_match_paths_outside_the_project_root() {
        let project = tempdir().unwrap();
        let external = tempdir().unwrap();
        let config = Config::load(None, project.path()).unwrap();
        let local_gemspec = project.path().join("local.gemspec");
        let external_gemspec = external.path().join("external.gemspec");

        assert!(config.rule_excluded("Metrics/BlockLength", &local_gemspec));
        assert!(!config.rule_excluded("Metrics/BlockLength", &external_gemspec));
    }
}
