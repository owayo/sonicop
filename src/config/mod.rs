use std::borrow::Cow;
mod inheritance;
mod loader;
mod namespaces;
mod obsoletion;
mod paths;
mod runtime;
mod store;
mod unrecognized;
mod validation;
mod yaml_booleans;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_yaml_ng::{Mapping, Value};

use crate::cop_name;
use crate::ruby_version::{
    ResolvedTargetRuby, RubyVersion, resolve_target_ruby, validate_supported,
};

use inheritance::{load_recorded_in_context, merge_config};
use loader::{
    absolute_config_origin, canonicalize_config_path, find_config_with_origin,
    path_parameter_base_directory,
};
use paths::{PathPatterns, compile_excludes, compile_includes, cop_patterns, has_hidden_component};

pub(crate) use loader::MissingConfigError;
pub(crate) use namespaces::AmbiguousConfigCopError;
pub(crate) use obsoletion::ObsoleteConfigError;
pub(crate) use obsoletion::TargetRubyGetterError;
pub(crate) use runtime::{RuntimeError, RuntimeSession};
pub use store::ConfigStore;
pub(crate) use unrecognized::UnknownConfigError;
use validation::ValidationBundle;
pub(crate) use validation::{ConfigWarning, ConfigWarnings, WarningOutputFailure};

const DEFAULT_CONFIG: &str = include_str!("../../config/default.yml");

#[derive(Clone, Debug)]
pub struct Config {
    raw: Value,
    runtime: Arc<RuntimeSession>,
    runtime_snapshot: Arc<runtime::Snapshot>,
    user: Value,
    renamed_overrides: HashMap<String, Mapping>,
    renamed_sources: Vec<(&'static str, &'static str)>,
    runtime_origin: String,
    using_warned: Arc<Mutex<HashSet<&'static str>>>,
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
    /// The result cache's digest of this configuration, folded on first use.
    ///
    /// Nothing here changes after the configuration is built, and every file in a run mixes the
    /// digest into its own cache key -- so serializing the merged configuration again per file, all
    /// several hundred kilobytes of it once the default set is in, cost a cached run more than
    /// reading the files did. The `Arc` is what lets a cloned `Config` share the answer instead of
    /// working it out again.
    cache_digest: Arc<OnceLock<Option<[u8; 32]>>>,
    // 検証履歴は設定値ではなく、診断キャッシュの digest へ含めない。
    validation: Arc<ValidationBundle>,
}

impl Config {
    /// Stable digest of the on-disk result cache's view of this configuration. The merged raw
    /// configuration determines cop behaviour, while the resolved paths and Ruby version cover
    /// behaviour derived from where the configuration was loaded and from the surrounding project.
    pub(crate) fn cache_digest(&self) -> Result<&[u8; 32]> {
        self.cache_digest
            .get_or_init(|| {
                let material = serde_json::to_vec(&(
                    &self.raw,
                    self.path_base.to_string_lossy(),
                    self.cwd.to_string_lossy(),
                    self.config_path
                        .as_deref()
                        .map(|path| path.to_string_lossy()),
                    self.target_ruby.version.to_string(),
                ))
                .ok()?;
                Some(*blake3::hash(&material).as_bytes())
            })
            .as_ref()
            .context("failed to fingerprint the resolved configuration")
    }

    pub fn load(explicit: Option<&Path>, cwd: &Path) -> Result<Self> {
        Self::load_with_options(explicit, cwd, false)
    }

    pub fn load_with_options(
        explicit: Option<&Path>,
        cwd: &Path,
        force_default: bool,
    ) -> Result<Self> {
        Self::load_internal(explicit, cwd, force_default, None, false)
    }

    /// ConfigStore.apply_options!のload_fileだけを観測し、merge-default後のTargetRuby解決を早めない。
    pub(crate) fn observe_file_for_cli(path: &Path, warnings: &ConfigWarnings) -> Result<()> {
        match load_recorded_in_context(
            path,
            path,
            &mut HashSet::new(),
            &warnings.runtime,
            Some(warnings),
        ) {
            Ok(loaded) => warnings.emit(&loaded.validation.trace),
            Err(failure) => {
                warnings.emit(&failure.prefix)?;
                Err(failure.error)
            }
        }
    }

    pub(crate) fn load_for_cli(
        explicit: Option<&Path>,
        cwd: &Path,
        force_default: bool,
        warnings: &ConfigWarnings,
    ) -> Result<Self> {
        let config = Self::load_internal(explicit, cwd, force_default, Some(warnings), true)?;
        config.emit_parent_validation(warnings, None)?;
        Ok(config)
    }

    pub(super) fn load_deferred(
        explicit: Option<&Path>,
        cwd: &Path,
        force_default: bool,
        warnings: &ConfigWarnings,
    ) -> Result<Self> {
        Self::load_internal(explicit, cwd, force_default, Some(warnings), false)
    }

    fn load_internal(
        explicit: Option<&Path>,
        cwd: &Path,
        force_default: bool,
        warnings: Option<&ConfigWarnings>,
        immediate: bool,
    ) -> Result<Self> {
        let runtime = warnings.map_or_else(
            || Arc::new(RuntimeSession::default()),
            |warnings| Arc::clone(&warnings.runtime),
        );
        Self::load_internal_in_runtime(explicit, cwd, force_default, warnings, immediate, runtime)
    }

    fn load_internal_in_runtime(
        explicit: Option<&Path>,
        cwd: &Path,
        force_default: bool,
        warnings: Option<&ConfigWarnings>,
        immediate: bool,
        runtime: Arc<RuntimeSession>,
    ) -> Result<Self> {
        let mut default: Value = serde_yaml_ng::from_str(DEFAULT_CONFIG)
            .context("embedded RuboCop default configuration is invalid")?;
        let mut known_cops = cop_names(&default);
        let location = if force_default {
            None
        } else {
            match explicit {
                Some(path) => Some((
                    canonicalize_config_path(path, path).with_context(|| {
                        format!("configuration file not found: {}", path.display())
                    })?,
                    absolute_config_origin(path)?,
                )),
                None => find_config_with_origin(cwd),
            }
        };
        let config_path = location.as_ref().map(|(path, _)| path.clone());

        let mut validation = Arc::new(ValidationBundle::default());
        let (raw, user, unrecognized_cops) = if let Some((_, origin)) = &location {
            let loaded = match load_recorded_in_context(
                origin,
                explicit.unwrap_or(origin),
                &mut HashSet::new(),
                &runtime,
                warnings,
            ) {
                Ok(loaded) => loaded,
                Err(failure) => {
                    if let Some(warnings) = warnings {
                        warnings.emit(&failure.prefix)?;
                    }
                    return Err(failure.error);
                }
            };
            if immediate && let Some(warnings) = warnings {
                warnings.emit(&loaded.validation.trace)?;
            }
            validation = Arc::new(loaded.validation);
            let user = loaded.value;
            let registry = runtime.snapshot();
            registry.apply_defaults(&mut default)?;
            known_cops.extend(cop_names(&default));
            known_cops.extend(registry.names.iter().cloned());
            let configured_cops = cop_names(&user);
            let mut unknown = configured_cops
                .difference(&known_cops)
                .cloned()
                .collect::<Vec<_>>();
            unknown.extend(unrecognized::names_with_registry(&user, &registry));
            unknown.sort();
            unknown.dedup();
            (merge_config(default, user.clone()), user, unknown)
        } else {
            // 設定fileが無いrootも本家のdefault getterを既に使った履歴になる。
            runtime.observe_default_query()?;
            runtime.snapshot().apply_defaults(&mut default)?;
            known_cops.extend(cop_names(&default));
            known_cops.extend(runtime.snapshot().names.iter().cloned());
            (default, Value::Mapping(Mapping::new()), Vec::new())
        };

        let result = (|| -> Result<Self> {
            if all_cops_bool(&raw, "EnabledByDefault") && all_cops_bool(&raw, "DisabledByDefault") {
                bail!("AllCops/EnabledByDefault and AllCops/DisabledByDefault cannot both be true");
            }

            let path_base = path_parameter_base_directory(config_path.as_deref(), cwd);
            let configured_target = configured_target_ruby(&raw)?;
            let target_ruby = resolve_target_ruby(configured_target, path_base)?;
            validate_supported(target_ruby.version)?;
            if all_cops_mapping(&raw)
                .and_then(|mapping| mapping.get(Value::String("ParserEngine".to_owned())))
                .and_then(Value::as_str)
                == Some("parser_prism")
                && target_ruby.version < RubyVersion::new(3, 3)
            {
                bail!(
                    "RuboCop supports target Ruby versions 3.3 and above with Prism. Specified target Ruby version: {}",
                    target_ruby.version
                );
            }

            let includes = cop_patterns(&raw, "AllCops", "Include").unwrap_or_default();
            let mut renamed_overrides = HashMap::<String, Mapping>::new();
            let mut renamed_sources = Vec::new();
            for (old, new, settings) in obsoletion::legacy_settings(&raw) {
                // for_cop(String)はdeprecated_names_for順に浅く重ね、raw/userは変えない。
                renamed_overrides
                    .entry(new.to_owned())
                    .or_default()
                    .extend(settings);
                renamed_sources.push((old, new));
            }
            let overlay = Value::Mapping(Mapping::from_iter(renamed_overrides.iter().map(
                |(name, settings)| {
                    (
                        Value::String(name.clone()),
                        Value::Mapping(settings.clone()),
                    )
                },
            )));
            let mut excludes = compile_excludes(&raw);
            excludes.extend(compile_excludes(&overlay));
            let mut cop_includes = compile_includes(&raw);
            cop_includes.extend(compile_includes(&overlay));
            let runtime_origin = validation
                .origin
                .as_deref()
                .map(|path| crate::formatter::smart_path(path, cwd))
                .unwrap_or_default();

            Ok(Self {
                runtime_snapshot: runtime.snapshot(),
                runtime: Arc::clone(&runtime),
                raw,
                user,
                renamed_overrides,
                renamed_sources,
                runtime_origin,
                using_warned: Arc::new(Mutex::new(HashSet::new())),
                path_base: path_base.to_path_buf(),
                cwd: cwd.to_path_buf(),
                config_path,
                target_ruby,
                known_cops,
                unrecognized_cops,
                includes,
                excludes,
                cop_includes,
                cache_digest: Arc::new(OnceLock::new()),
                validation: Arc::clone(&validation),
            })
        })();
        if result.is_err()
            && !immediate
            && let Some(warnings) = warnings
        {
            // 値の先読みが後続の検証で失敗しても、先に確定した警告は一度だけ出す。
            warnings.emit(&validation.trace)?;
        }
        result
    }

    pub(super) fn emit_validation(&self, warnings: &ConfigWarnings, root: &Config) -> Result<()> {
        warnings.emit(&self.validation.trace)?;
        self.emit_parent_validation(warnings, Some(root.validation.as_ref()))
    }

    fn emit_parent_validation(
        &self,
        warnings: &ConfigWarnings,
        fallback: Option<&ValidationBundle>,
    ) -> Result<()> {
        if !warnings.parent_exclusion {
            return Ok(());
        }
        let Some(path) = self.config_path() else {
            return Ok(());
        };
        let origin = self.validation.origin.as_deref().unwrap_or(path);
        let Some(parent) = loader::parent_exclusion_config(path, origin, self.cwd()) else {
            return Ok(());
        };
        if !self.runtime.active()
            && let Some(trace) = self
                .validation
                .documents
                .get(&parent)
                .or_else(|| fallback.and_then(|bundle| bundle.documents.get(&parent)))
        {
            return warnings.emit(trace);
        }
        // parent Exclude は load_file だけを使う。既定値や対象 Ruby の解決をもう一度行わない。
        match load_recorded_in_context(
            &parent,
            &parent,
            &mut HashSet::new(),
            &self.runtime,
            Some(warnings),
        ) {
            Ok(loaded) => warnings.emit(&loaded.validation.trace),
            Err(failure) => {
                warnings.emit(&failure.prefix)?;
                Err(failure.error)
            }
        }
    }

    pub(super) fn load_in_runtime(
        explicit: Option<&Path>,
        cwd: &Path,
        force_default: bool,
        runtime: Arc<RuntimeSession>,
    ) -> Result<Self> {
        Self::load_internal_in_runtime(explicit, cwd, force_default, None, false, runtime)
    }

    /// Ruby拡張を実行した設定のRegistry snapshot。未起動時にgetterだけでRubyを起動しない。
    pub fn ruby_registered_cop_names(&self) -> &[String] {
        &self.runtime_snapshot.names
    }

    pub fn loaded_features(&self) -> Vec<String> {
        self.runtime.snapshot().loaded_features.clone()
    }

    pub fn loaded_plugin_names(&self) -> Vec<String> {
        self.runtime.snapshot().loaded_plugins.clone()
    }

    pub(crate) fn ruby_runtime(&self) -> &Arc<RuntimeSession> {
        &self.runtime
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Project boundary used by the result cache.
    ///
    /// RuboCop resolves every configured path against this directory. Keeping it in the cache
    /// identity prevents unrelated projects that happen to use the same cop selection from
    /// accumulating in one ever-growing index.
    pub(crate) fn path_base(&self) -> &Path {
        &self.path_base
    }

    pub(super) fn config_origin(&self) -> Option<&Path> {
        self.validation.origin.as_deref()
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
        self.rule_enabled_with_defaults(name, enable_pending, disable_pending, None)
    }

    pub(crate) fn rule_enabled_with_defaults(
        &self,
        name: &str,
        enable_pending: bool,
        disable_pending: bool,
        default_enabled: Option<bool>,
    ) -> bool {
        // Registryの実行資格はConfigの!!値と異なり、true又は資格のあるpendingだけ。
        self.resolve_enabled_value(
            &self.cop_enabled_value_with_defaults(name, default_enabled),
            name,
            enable_pending,
            disable_pending,
        )
    }

    pub(crate) fn cop_enabled_with_defaults(&self, name: &str, mode: Option<bool>) -> bool {
        !matches!(
            &*self.cop_enabled_value_with_defaults(name, mode),
            Value::Null | Value::Bool(false)
        )
    }

    pub(crate) fn cop_enabled_value_with_defaults(
        &self,
        name: &str,
        mode: Option<bool>,
    ) -> Cow<'_, Value> {
        // 旧Hashはfor_copの有効値計算後に重なる。null/pendingも生の型を保つ。
        if let Some(value) = self
            .renamed_overrides
            .get(name)
            .and_then(|cop| cop.get("Enabled"))
        {
            return Cow::Borrowed(value);
        }
        if name == "Lint/Syntax" {
            return Cow::Owned(Value::Bool(true));
        }
        let configured = self.user_cop_mapping(name);
        let enabled = configured.and_then(|cop| cop.get("Enabled"));
        let department = self
            .user_department_mapping(name)
            .and_then(|cop| cop.get("Enabled"));
        let disabled = mode.map_or_else(
            || self.all_cops_bool_value("DisabledByDefault"),
            |value| !value,
        );
        let all_enabled = mode.unwrap_or_else(|| self.all_cops_bool_value("EnabledByDefault"));
        // disable既定では明示空HashにもEnabled:trueを補う。CLIは明示値を消さない。
        if enabled == Some(&Value::Bool(true))
            || enabled.and_then(Value::as_str) == Some("override_department")
            || (disabled && configured.is_some() && enabled.is_none())
        {
            return Cow::Owned(Value::Bool(true));
        }
        if department == Some(&Value::Bool(false)) {
            return Cow::Owned(Value::Bool(false));
        }
        if let Some(value) = enabled {
            // merge_with_default(unset_nil:true)で消えたキーのfallbackは元AllCopsの値。
            return if value.is_null() {
                Cow::Owned(Value::Bool(!self.all_cops_bool_value("DisabledByDefault")))
            } else {
                Cow::Borrowed(value)
            };
        }
        let original = || {
            self.cop_raw_value(name, "Enabled").map_or(
                Cow::Owned(Value::Bool(!self.all_cops_bool_value("DisabledByDefault"))),
                Cow::Borrowed,
            )
        };
        if disabled {
            // departmentを有効にすると、そのcop群は一律trueでなく元の既定値へ戻る。
            return if department
                .is_some_and(|value| !matches!(value, Value::Null | Value::Bool(false)))
            {
                original()
            } else {
                Cow::Owned(Value::Bool(false))
            };
        }
        if all_enabled {
            return Cow::Owned(Value::Bool(true));
        }
        original()
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

    /// Whether the run may use this cop's corrector at all, which is the cop's own `AutoCorrect`
    /// setting rather than anything the command line asked for.
    ///
    /// This answers two upstream questions at once, because for every value RuboCop accepts they
    /// have the same answer. `AutocorrectLogic#autocorrect_enabled?`
    /// (`cop/autocorrect_logic.rb:31-46`) refuses `'disabled'` and `false` outright, and refuses
    /// `'contextual'` while `LSP.enabled?` -- which `--editor-mode` and `--lsp` set. `Base#use_corrector`
    /// (`cop/base.rb:445-453`) then decides what the offense is reported as, and its condition
    /// `always_autocorrect? || (contextual_autocorrect? && !LSP.enabled?)` is false in exactly
    /// those cases: the offense keeps status `:unsupported`, so it is **not** `correctable?` and no
    /// `-a`/`-A` run rewrites it. Answering `false` here therefore has to do both -- withhold the
    /// edits and clear the correctable marker -- or a user who opted out of a cop's autocorrection
    /// still gets their code rewritten.
    ///
    /// `editor_mode` is `LSP.enabled?`. `AutoCorrect: contextual` is not an oddity to guard
    /// against: `config/default.yml` ships it on 19 cops, `Lint/UselessAssignment` among them, and
    /// deleting a half-typed assignment is precisely what an editor must not do.
    ///
    /// An absent key means `'always'` (`cop_config.fetch('AutoCorrect', 'always')`). A boolean is
    /// the backward-compatible spelling of `'always'`/`'disabled'`, and
    /// `ConfigValidator::CONFIG_CHECK_AUTOCORRECTS` refuses any other string, so the values below
    /// are the whole reachable set. Anything else -- a number, a list -- is read as `'always'`:
    /// upstream's `autocorrect_enabled?` says the same, and folding it the other way would let a
    /// value RuboCop rejects outright silently switch a cop's corrections off.
    pub fn rule_autocorrect_enabled(&self, name: &str, editor_mode: bool) -> bool {
        match self.cop_raw_value(name, "AutoCorrect") {
            Some(Value::Bool(value)) => *value,
            Some(Value::String(value)) => match value.as_str() {
                "disabled" => false,
                "contextual" => !editor_mode,
                _ => true,
            },
            _ => true,
        }
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

    /// `Config#allowed_camel_case_file?`: a gemspec may use dashes to spell a namespace, and a file
    /// that only reaches the run through an `Include` pattern written with capitals -- `Rakefile`,
    /// `Gemfile` -- is named after that pattern rather than after Ruby's conventions.
    ///
    /// This belongs to the configuration rather than to `Naming/FileName`, exactly as upstream has
    /// it, and the reason is not only tidiness: the cop is put to every file in the run, so
    /// answering it from the cop meant re-reading `AllCops.Include` and recompiling its globs once
    /// per file. Measured on rubocop/rubocop, that was 1.0 of the run's 19.0 seconds of cop time.
    pub fn allowed_camel_case_file(&self, path: &Path) -> bool {
        if path
            .extension()
            .is_some_and(|extension| extension == "gemspec")
        {
            return true;
        }
        self.includes
            .matches_uppercase_includes(path, &self.path_base)
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

    pub(crate) fn unknown_config_cop_names(&self) -> Vec<&str> {
        // libraryのmetadataは残し、CLIで今回扱う旧Hashだけを未知名拒否から外す。
        self.unrecognized_cops
            .iter()
            .filter(|name| !obsoletion::warning_name(name) || self.cop_mapping(name).is_none())
            .map(String::as_str)
            .collect()
    }

    pub(crate) fn catalog_view(&self) -> Self {
        // Class lookupとString runtimeの値を分け、Rubyのobject-key cache状態は持ち込まない。
        let mut config = self.clone();
        config.renamed_overrides.clear();
        config.renamed_sources.clear();
        config.excludes = compile_excludes(&config.raw);
        config.cop_includes = compile_includes(&config.raw);
        config
    }

    pub(crate) fn warn_renamed_cops(
        &self,
        selected: &dyn Fn(&str) -> bool,
        output: &dyn Fn(&str) -> Result<()>,
    ) -> Result<()> {
        for &(old, new) in &self.renamed_sources {
            if !selected(new) {
                continue;
            }
            let claimed = self
                .using_warned
                .lock()
                .map_err(|_| anyhow::anyhow!("configuration warning lock is poisoned"))?
                .insert(old);
            // clone間のclaimだけをlockで守り、I/Oや再入callbackの間はlockを保持しない。
            if claimed {
                output(&format!(
                    "Warning: Using `{old}` configuration in {} for `{new}`.\n",
                    self.runtime_origin
                ))?;
            }
        }
        Ok(())
    }

    pub fn description(&self, name: &str) -> Option<String> {
        self.cop_value(name, "Description")
    }

    /// 表示用の既定値も Symbol/Regexp の型を保つ。検査用の既存の射影は変更しない。
    pub(crate) fn displayed_cop_settings(&self) -> Value {
        static DEFAULT: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
            serde_yaml_ng::from_str(&yaml_booleans::normalize(DEFAULT_CONFIG))
                .expect("embedded configuration must be valid")
        });
        let merged = merge_config(DEFAULT.clone(), self.user.clone());
        let mut settings = ordered_settings(&DEFAULT, &merged);
        for name in self.known_cop_names() {
            let Some(cop) = settings.get_mut(name).and_then(Value::as_mapping_mut) else {
                continue;
            };
            let enabled = match (
                self.rule_enabled_with_pending(name, true, false),
                self.rule_enabled_with_pending(name, false, true),
            ) {
                (true, false) => Value::String("pending".to_owned()),
                (true, true) => self
                    .user_cop_mapping(name)
                    .and_then(|cop| cop.get("Enabled"))
                    .filter(|value| {
                        !matches!(value, Value::Bool(_) | Value::Null)
                            && value.as_str() != Some("override_department")
                    })
                    .cloned()
                    .unwrap_or(Value::Bool(true)),
                _ => Value::Bool(false),
            };
            cop.insert(Value::String("Enabled".to_owned()), enabled);
            if let Some(Value::Sequence(excludes)) = cop.get_mut("Exclude") {
                for exclude in excludes {
                    if let Value::String(pattern) = exclude
                        && !Path::new(pattern).is_absolute()
                    {
                        *pattern = self
                            .path_base
                            .join(&*pattern)
                            .to_string_lossy()
                            .replace('\\', "/");
                    }
                }
            }
        }
        settings
    }

    fn cop_mapping(&self, name: &str) -> Option<&Mapping> {
        self.raw.as_mapping()?.get(name)?.as_mapping()
    }

    fn cop_raw_value(&self, name: &str, key: &str) -> Option<&Value> {
        self.renamed_overrides
            .get(name)
            .and_then(|cop| cop.get(key))
            .or_else(|| self.cop_mapping(name)?.get(key))
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
                if disable_pending {
                    false
                } else if enable_pending {
                    true
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
            _ => false,
        }
    }

    fn all_cops_bool_value(&self, key: &str) -> bool {
        self.cop_raw_value("AllCops", key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// Hash の上書きは既存キーの位置を変えない。本家の既定順を merge 後にも保つ。
fn ordered_settings(default: &Value, merged: &Value) -> Value {
    let (Some(default), Some(merged)) = (default.as_mapping(), merged.as_mapping()) else {
        return merged.clone();
    };
    let mut ordered = Mapping::new();
    for key in default.keys().chain(merged.keys()) {
        if !ordered.contains_key(key)
            && let Some(value) = merged.get(key)
        {
            ordered.insert(
                key.clone(),
                ordered_settings(default.get(key).unwrap_or(&Value::Null), value),
            );
        }
    }
    Value::Mapping(ordered)
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
    fn autocorrect_is_read_per_cop_and_contextual_defers_to_the_editor() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "Layout/TrailingWhitespace:\n  AutoCorrect: false\nStyle/StringLiterals:\n  AutoCorrect: disabled\nStyle/HashSyntax:\n  AutoCorrect: always\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        for editor_mode in [false, true] {
            assert!(!config.rule_autocorrect_enabled("Layout/TrailingWhitespace", editor_mode));
            assert!(!config.rule_autocorrect_enabled("Style/StringLiterals", editor_mode));
            assert!(config.rule_autocorrect_enabled("Style/HashSyntax", editor_mode));
            // No `AutoCorrect` key at all, which upstream reads as `'always'`.
            assert!(config.rule_autocorrect_enabled("Layout/SpaceAfterComma", editor_mode));
        }

        // `AutoCorrect: contextual` ships in the default configuration for this cop.
        assert!(config.rule_autocorrect_enabled("Lint/UselessAssignment", false));
        assert!(!config.rule_autocorrect_enabled("Lint/UselessAssignment", true));
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
    #[test]
    fn renamed_runtime_and_catalog_match_four_real_api_conditions_without_mutating_raw() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/renamed_config.json")).unwrap();
        let apis = fixture["runtime_api"].as_array().unwrap();
        assert_eq!(apis.len(), 4);
        for api in apis {
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == api["id"])
                .unwrap();
            let directory = tempfile::tempdir().unwrap();
            for name in ["config.yml", "Gemfile"] {
                std::fs::write(
                    directory.path().join(name),
                    case["files"][name].as_str().unwrap(),
                )
                .unwrap();
            }
            let config =
                Config::load(Some(&directory.path().join("config.yml")), directory.path()).unwrap();
            let before = (
                config.raw.clone(),
                config.user.clone(),
                *config.cache_digest().unwrap(),
            );
            let name = "Naming/PredicatePrefix";
            for key in ["Enabled", "AllowedMethods"] {
                assert_eq!(
                    config.cop_value::<serde_json::Value>(name, key),
                    Some(api["values"]["first"][key].clone()),
                    "{} {key}",
                    api["id"]
                );
            }
            let catalog = config.catalog_view();
            assert_eq!(
                catalog.rule_enabled(name),
                api["values"]["catalog"]["Enabled"].as_bool().unwrap()
            );
            assert_eq!(
                catalog.cop_value::<serde_json::Value>(name, "AllowedMethods"),
                Some(api["values"]["catalog"]["AllowedMethods"].clone())
            );
            let messages = std::sync::Mutex::new(Vec::new());
            catalog
                .warn_renamed_cops(&|_| true, &|message| {
                    messages.lock().unwrap().push(message.to_owned());
                    Ok(())
                })
                .unwrap();
            config
                .warn_renamed_cops(&|_| false, &|_| panic!("未選択copはlookupしない"))
                .unwrap();
            assert!(messages.lock().unwrap().is_empty());
            for copy in [config.clone(), config.clone()] {
                copy.warn_renamed_cops(&|_| true, &|message| {
                    messages.lock().unwrap().push(message.to_owned());
                    // I/O callbackから同じcloneへ再入してもclaim lockを保持していない。
                    config
                        .warn_renamed_cops(&|_| true, &|_| panic!("claimはclone間で一度だけ"))
                        .unwrap();
                    Ok(())
                })
                .unwrap();
            }
            assert_eq!(messages.lock().unwrap().len(), 1);
            assert!(messages.lock().unwrap()[0].ends_with('\n'));
            assert_eq!(
                before,
                (
                    config.raw.clone(),
                    config.user.clone(),
                    *config.cache_digest().unwrap()
                )
            );
            assert!(
                config
                    .unrecognized_cop_names()
                    .iter()
                    .any(|name| name == "Naming/PredicateName")
            );
            assert!(config.unknown_config_cop_names().is_empty());
        }
    }
    #[test]
    fn effective_enabled_values_match_the_fifteen_real_config_api_observations() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/default_cop_queries.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        let queries = fixture["queries"].as_array().unwrap();
        assert_eq!(cases.len(), 15);
        assert_eq!(queries.len(), 15);
        assert_eq!(
            queries
                .iter()
                .map(|row| row["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "ordinary",
                "enable-default",
                "enable-except-neighbor",
                "disable-only-neighbor",
                "pending-none",
                "null-none",
                "false-none",
                "pending-enable",
                "null-enable",
                "pending-disable",
                "null-disable",
                "typed-absent-none",
                "typed-pending-none",
                "typed-null-none",
                "typed-absent-enable"
            ]
        );
        for (case, expected) in cases.iter().zip(queries) {
            assert_eq!(case["id"], expected["id"]);
            let directory = tempfile::tempdir().unwrap();
            for (name, text) in case["files"].as_object().unwrap() {
                std::fs::write(directory.path().join(name), text.as_str().unwrap()).unwrap();
            }
            let config =
                Config::load(Some(&directory.path().join("config.yml")), directory.path()).unwrap();
            let args = case["args"].as_array().unwrap();
            let mode = if args.iter().any(|value| value == "--disable-all-cops") {
                Some(false)
            } else {
                args.iter()
                    .any(|value| value == "--enable-all-cops")
                    .then_some(true)
            };
            let name = expected["query"].as_str().unwrap();
            let before = config.cop_value::<serde_json::Value>(name, "Enabled");
            let digest = *config.cache_digest().unwrap();
            let enabled = config.cop_enabled_value_with_defaults(name, mode);
            assert_eq!(
                serde_json::to_value(&*enabled).unwrap(),
                expected["value"],
                "{}",
                case["id"]
            );
            let class = match &*enabled {
                serde_yaml_ng::Value::Bool(true) => "TrueClass",
                serde_yaml_ng::Value::Bool(false) => "FalseClass",
                serde_yaml_ng::Value::Null => "NilClass",
                serde_yaml_ng::Value::String(_) => "String",
                _ => panic!("参照外のEnabled型"),
            };
            assert_eq!(class, expected["ruby_class"].as_str().unwrap());
            assert_eq!(
                config.cop_enabled_with_defaults(name, mode),
                expected["truthy"].as_bool().unwrap()
            );
            assert_eq!(
                config.cop_value::<serde_json::Value>(name, "Enabled"),
                before
            );
            assert_eq!(*config.cache_digest().unwrap(), digest);
        }
    }
}
