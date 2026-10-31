use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_yaml_ng::{Mapping, Value};

use super::loader::{
    absolute_config_error_origin, absolute_config_origin, canonicalize_config_path, config_io_error,
};
use super::namespaces::{self, Origin};
use super::validation::{OrderedKeys, ValidationBundle, ValidationTrace};

#[derive(Clone)]
struct ResolvedDocument {
    value: Value,
    keys: Arc<OrderedKeys>,
    trace: Arc<ValidationTrace>,
    pending: Arc<ValidationTrace>,
}

/// visitedはcanonicalの再帰途中だけを表す。読取/parseは共有し、検証履歴はorigin別・辺ごとに再生する。
#[derive(Default)]
struct ResolvedConfigs {
    // 値とTraceはorigin別。別aliasのrelative inheritや警告を最初の表記へ固定しない。
    local: HashMap<PathBuf, ResolvedDocument>,
    parsed: HashMap<PathBuf, Value>,
    remote: HashMap<String, ResolvedDocument>,
    runtime: Arc<super::RuntimeSession>,
    warnings: Option<super::ConfigWarnings>,
}

#[derive(Debug)]
pub(super) struct RecordedConfig {
    pub(super) value: Value,
    pub(super) validation: ValidationBundle,
}

#[derive(Debug)]
pub(super) struct ReadFailure {
    pub(super) error: anyhow::Error,
    pub(super) prefix: Arc<ValidationTrace>,
}

#[cfg(test)]
thread_local! {
    // 実際に読んだ文書数を数え、共有継承の memo が効いていることを時間によらず検査する。
    static PARSED_CONFIG_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn load_with_inheritance(path: &Path, visited: &mut HashSet<PathBuf>) -> Result<Value> {
    load_recorded(path, visited)
        .map(|loaded| loaded.value)
        .map_err(|failure| failure.error)
}

#[cfg(test)]
pub(super) fn load_recorded(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<RecordedConfig, ReadFailure> {
    load_recorded_with_error_origin(path, path, visited)
}

#[cfg(test)]
pub(super) fn load_recorded_with_error_origin(
    path: &Path,
    error_origin: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<RecordedConfig, ReadFailure> {
    load_recorded_in_context(
        path,
        error_origin,
        visited,
        &Arc::new(super::RuntimeSession::default()),
        None,
    )
}

pub(super) fn load_recorded_in_context(
    path: &Path,
    error_origin: &Path,
    visited: &mut HashSet<PathBuf>,
    runtime: &Arc<super::RuntimeSession>,
    warnings: Option<&super::ConfigWarnings>,
) -> Result<RecordedConfig, ReadFailure> {
    let mut resolved = ResolvedConfigs {
        runtime: Arc::clone(runtime),
        warnings: warnings.cloned(),
        ..Default::default()
    };
    let mut completed = Vec::new();
    match load_local_with_inheritance(path, error_origin, visited, &mut resolved, &mut completed) {
        Ok(document) => Ok(RecordedConfig {
            value: document.value,
            validation: ValidationBundle {
                origin: absolute_config_origin(path).ok(),
                trace: document.pending,
                documents: resolved
                    .local
                    .into_iter()
                    .map(|(path, doc)| (path, doc.trace))
                    .collect(),
            },
        }),
        Err(error) => Err(ReadFailure {
            error,
            prefix: ValidationTrace::sequence(completed),
        }),
    }
}

fn extensions_before_inheritance(
    value: &Value,
    origin: &str,
    resolved: &ResolvedConfigs,
    completed: &mut [Arc<ValidationTrace>],
) -> Result<()> {
    if !super::RuntimeSession::has_extensions(value) {
        return Ok(());
    }
    if let Some(warnings) = &resolved.warnings {
        // 最初のrequireが後段にあっても、先行fatalより先にそのRubyを実行しない。
        let pending = completed.iter_mut().map(std::mem::take).collect::<Vec<_>>();
        for trace in pending {
            warnings.emit(&trace)?;
        }
    }
    resolved.runtime.config_extensions(value, origin)
}

fn load_local_with_inheritance(
    path: &Path,
    error_origin: &Path,
    visited: &mut HashSet<PathBuf>,
    resolved: &mut ResolvedConfigs,
    completed: &mut Vec<Arc<ValidationTrace>>,
) -> Result<ResolvedDocument> {
    let checkpoint = completed.len();
    let origin = absolute_config_origin(path)?;
    let error_origin = absolute_config_error_origin(error_origin)?;
    let canonical = canonicalize_config_path(path, &error_origin)
        .with_context(|| format!("configuration file not found: {}", path.display()))?;
    if !resolved.runtime.active()
        && let Some(cached) = resolved.local.get(&origin)
    {
        completed.push(Arc::clone(&cached.pending));
        return Ok(cached.clone());
    }
    if !visited.insert(canonical.clone()) {
        bail!("circular inherit_from detected at {}", canonical.display());
    }
    let mut current = if let Some(parsed) = resolved.parsed.get(&canonical) {
        parsed.clone()
    } else {
        let contents = fs::read_to_string(&canonical)
            .map_err(|error| config_io_error(&error_origin, error))
            .with_context(|| format!("failed to read configuration: {}", canonical.display()))?;
        #[cfg(test)]
        PARSED_CONFIG_COUNT.with(|count| count.set(count.get() + 1));
        let parsed = parse_yaml_configuration(&contents, canonical.display())?;
        resolved.parsed.insert(canonical.clone(), parsed.clone());
        parsed
    };
    // meta key の削除や native の値 merge が Ruby Hash の順序を動かす前に記録する。
    let mut keys = OrderedKeys::from_value(&current);
    let target = super::obsoletion::TargetRuby::new(&current, Some(canonical.clone()));
    extensions_before_inheritance(
        &current,
        &origin.to_string_lossy().replace('\\', "/"),
        resolved,
        completed,
    )?;
    let inherit = take_mapping_key(&mut current, "inherit_from");
    let inherit_gem = take_mapping_key(&mut current, "inherit_gem");
    let parent = origin.parent().unwrap_or(Path::new("."));
    let mut inherited_paths = resolve_inherit_gems(inherit_gem)?;
    inherited_paths.extend(parse_inherit_paths(inherit)?);
    let inherited_paths = expand_inherit_paths(inherited_paths)?;
    let mut merged = Value::Mapping(Mapping::new());
    let mut inherited_keys = Vec::new();
    let mut inherited_traces = Vec::new();
    for inherited in inherited_paths {
        let document = if inherited.starts_with("http://") || inherited.starts_with("https://") {
            load_remote_with_inheritance(&inherited, &mut HashSet::new(), resolved, completed)?
        } else {
            let inherited = PathBuf::from(inherited);
            let inherited_error_origin = if inherited.is_absolute() {
                inherited.clone()
            } else {
                error_origin
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(&inherited)
            };
            let inherited = if inherited.is_absolute() {
                inherited
            } else {
                parent.join(inherited)
            };
            load_local_with_inheritance(
                &inherited,
                &inherited_error_origin,
                visited,
                resolved,
                completed,
            )?
        };
        inherited_keys.push(Arc::clone(&document.keys));
        inherited_traces.push(Arc::clone(&document.trace));
        merged = merge_inherited_config(merged, document.value);
    }
    visited.remove(&canonical);
    for base in inherited_keys.iter().rev() {
        keys.inherit(base);
    }
    let mut value = merge_inherited_config(merged, current);
    let mut namespace_warnings = Vec::new();
    let display = match namespaces::normalize(
        &mut value,
        &mut keys,
        Origin::File(&origin),
        &mut namespace_warnings,
        &resolved.runtime,
    ) {
        Ok(display) => display,
        Err(error) => {
            // 曖昧名に達する前の警告も、完了した継承先の後ろへ残す。
            completed.push(ValidationTrace::namespaces(namespace_warnings));
            return Err(error);
        }
    };
    if let Err(error) = resolved.runtime.observe_default_query() {
        completed.push(ValidationTrace::namespaces(namespace_warnings));
        return Err(error);
    }
    // 完了した子を一つの共有履歴へ畳む。失敗時は完了済み部分だけが completed に残る。
    let pending_children = completed.split_off(checkpoint);
    let trace = ValidationTrace::document(
        inherited_traces,
        &keys,
        &value,
        namespace_warnings,
        &display,
        &target,
        &resolved.runtime.snapshot(),
    );
    if resolved.warnings.is_none() {
        // quiet API も、この文書の拒否を次の継承先の I/O より後へ遅らせない。
        trace.check_fatal_settings()?;
    }
    let mut pending = trace.with_children(pending_children);
    if resolved.runtime.active()
        && let Some(warnings) = &resolved.warnings
    {
        // Rubyの副作用が後続文書へ進む前にfatalを確定し、同じ警告を再生しない。
        warnings.emit(&pending)?;
        pending = Arc::default();
    }
    completed.push(Arc::clone(&pending));
    let document = ResolvedDocument {
        value,
        keys: Arc::new(keys),
        trace,
        pending,
    };
    resolved.local.insert(origin, document.clone());
    Ok(document)
}

fn load_remote_with_inheritance(
    url: &str,
    visited: &mut HashSet<String>,
    resolved: &mut ResolvedConfigs,
    completed: &mut Vec<Arc<ValidationTrace>>,
) -> Result<ResolvedDocument> {
    let checkpoint = completed.len();
    if !resolved.runtime.active()
        && let Some(cached) = resolved.remote.get(url)
    {
        completed.push(Arc::clone(&cached.pending));
        return Ok(cached.clone());
    }
    if !visited.insert(url.to_owned()) {
        bail!("circular remote inherit_from detected at {url}");
    }
    // 取得元のURLと、Registryが出典として表示するRemoteConfigの名前を分ける。
    let cwd = std::env::current_dir()?;
    let root = super::cache_root::for_remote(resolved.warnings.as_ref(), &cwd)?;
    let remote_origin = super::remote_origin::path(url, &root, &cwd)?;
    let contents = super::remote_cache::read(url, &remote_origin)?;
    let mut current = parse_yaml_configuration(&contents, url)?;
    let mut keys = OrderedKeys::from_value(&current);
    let target = super::obsoletion::TargetRuby::new(&current, None);
    extensions_before_inheritance(&current, url, resolved, completed)?;
    let inherit = take_mapping_key(&mut current, "inherit_from");
    let mut merged = Value::Mapping(Mapping::new());
    let mut inherited_keys = Vec::new();
    let mut inherited_traces = Vec::new();
    for inherited in expand_inherit_paths(parse_inherit_paths(inherit)?)? {
        let inherited_url = if inherited.starts_with("http://") || inherited.starts_with("https://")
        {
            inherited
        } else {
            join_remote_url(url, &inherited)?
        };
        let document = load_remote_with_inheritance(&inherited_url, visited, resolved, completed)?;
        inherited_keys.push(Arc::clone(&document.keys));
        inherited_traces.push(Arc::clone(&document.trace));
        merged = merge_inherited_config(merged, document.value);
    }
    visited.remove(url);
    for base in inherited_keys.iter().rev() {
        keys.inherit(base);
    }
    let mut value = merge_inherited_config(merged, current);
    let mut namespace_warnings = Vec::new();
    let display = match namespaces::normalize(
        &mut value,
        &mut keys,
        Origin::Remote(&remote_origin),
        &mut namespace_warnings,
        &resolved.runtime,
    ) {
        Ok(display) => display,
        Err(error) => {
            // 曖昧名に達する前の警告も、完了した継承先の後ろへ残す。
            completed.push(ValidationTrace::namespaces(namespace_warnings));
            return Err(error);
        }
    };
    if let Err(error) = resolved.runtime.observe_default_query() {
        completed.push(ValidationTrace::namespaces(namespace_warnings));
        return Err(error);
    }
    let pending_children = completed.split_off(checkpoint);
    let trace = ValidationTrace::document(
        inherited_traces,
        &keys,
        &value,
        namespace_warnings,
        &display,
        &target,
        &resolved.runtime.snapshot(),
    );
    if resolved.warnings.is_none() {
        trace.check_fatal_settings()?;
    }
    let mut pending = trace.with_children(pending_children);
    if resolved.runtime.active()
        && let Some(warnings) = &resolved.warnings
    {
        // Rubyの副作用が後続文書へ進む前にfatalを確定し、同じ警告を再生しない。
        warnings.emit(&pending)?;
        pending = Arc::default();
    }
    completed.push(Arc::clone(&pending));
    let document = ResolvedDocument {
        value,
        keys: Arc::new(keys),
        trace,
        pending,
    };
    resolved.remote.insert(url.to_owned(), document.clone());
    Ok(document)
}

/// Parses one configuration document the way `ConfigLoader#load_yaml_configuration` does.
///
/// 本家の `yaml_tree_to_hash(...) || {}` に合わせ、空の設定と false は既定値を残す。
/// それ以外の scalar を許すと既定設定全体が置換され、Include / Exclude や無効 cop を失う。
pub(super) fn parse_yaml_configuration(contents: &str, origin: impl Display) -> Result<Value> {
    // 本家の ConfigLoader は BOM 付き設定を YAML として拒否する。通常の UTF-8 設定とは区別する。
    if contents.starts_with('\u{feff}') {
        bail!(
            "invalid YAML in {origin}: did not find expected <document start> at line 1 column 1"
        );
    }
    let contents = super::yaml_booleans::normalize(contents);
    let value: Value =
        serde_yaml_ng::from_str(&contents).with_context(|| format!("invalid YAML in {origin}"))?;
    match value {
        // Ruby で偽となる nil / false だけを `|| {}` と同じ空の mapping にする。
        Value::Null | Value::Bool(false) => Ok(Value::Mapping(Mapping::new())),
        Value::Mapping(_) => Ok(value),
        _ => bail!("Malformed configuration in {origin}"),
    }
}

/// Resolves an `inherit_from` entry of a remote configuration against the URL it was found in,
/// the way RFC 3986 resolves a relative reference: the last path segment of the base is replaced,
/// and a query string or fragment of the base is dropped rather than carried over.
///
/// Everything after the authority has to be located explicitly. Searching the whole URL for the
/// last `/` mistakes the second slash of `https://` for a directory separator when the base
/// carries no path at all -- turning `https://example.com` + `shared.yml` into
/// `https://shared.yml` -- and mistakes a slash inside a query string for one as well.
fn join_remote_url(base: &str, relative: &str) -> Result<String> {
    let scheme_end = base
        .find("://")
        .context("remote configuration URL has no scheme")?
        + 3;
    let hierarchical_end = base[scheme_end..]
        .find(['?', '#'])
        .map_or(base.len(), |offset| scheme_end + offset);
    let hierarchical = &base[..hierarchical_end];
    let authority_end = hierarchical[scheme_end..]
        .find('/')
        .map_or(hierarchical.len(), |offset| scheme_end + offset);
    if relative.starts_with('/') {
        return Ok(format!("{}{relative}", &hierarchical[..authority_end]));
    }
    match hierarchical[authority_end..].rfind('/') {
        Some(offset) => Ok(format!(
            "{}{relative}",
            &hierarchical[..authority_end + offset + 1]
        )),
        None => Ok(format!("{hierarchical}/{relative}")),
    }
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

fn expand_inherit_paths(paths: Vec<String>) -> Result<Vec<String>> {
    let cwd = std::env::current_dir()?;
    Ok(paths
        .iter()
        .flat_map(|pattern| super::inherit_glob::expand(pattern, &cwd))
        .collect())
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

/// Where `should_union?` gets its "root mode" from.
///
/// RuboCop merges configurations through two entry points that answer this differently, and
/// nothing else about the two merges differs, so the whole difference lives in this choice.
enum RootMode {
    /// `ConfigLoaderResolver#merge_with_default` passes `config['inherit_mode'] || {}` -- the
    /// file-level directive -- unchanged for every key it merges.
    File,
    /// `ConfigLoaderResolver#resolve_inheritance` passes `determine_inherit_mode(hash, cop)`,
    /// which is `local_inherit || hash['inherit_mode'] || {}`: a cop's own directive *replaces*
    /// the file-level one, and since an empty Hash is truthy in Ruby it replaces it even when it
    /// selects nothing.
    PerCop,
}

/// Merges a user configuration over the `default.yml`-derived one, mirroring
/// `ConfigLoaderResolver#merge_with_default`.
pub(super) fn merge_config(base: Value, overlay: Value) -> Value {
    merge_mappings(base, overlay, &RootMode::File)
}

/// Merges an inheriting configuration over the one it inherits from, mirroring
/// `ConfigLoaderResolver#resolve_inheritance`.
fn merge_inherited_config(base: Value, overlay: Value) -> Value {
    merge_mappings(base, overlay, &RootMode::PerCop)
}

fn merge_mappings(base: Value, overlay: Value, root: &RootMode) -> Value {
    match (base, overlay) {
        (Value::Mapping(mut base), Value::Mapping(overlay)) => {
            let file_mode = inherit_mode(&overlay).unwrap_or_default();
            for (key, value) in overlay {
                let cop_mode = match root {
                    RootMode::File => None,
                    RootMode::PerCop => value.as_mapping().and_then(inherit_mode),
                };
                let mode = cop_mode.as_ref().unwrap_or(&file_mode);
                // `map_or` evaluates its default eagerly, so every key paid for a deep
                // clone even though most keys are absent from `base`.
                let merged = match base.remove(&key) {
                    Some(old) => deep_merge(old, value, mode),
                    None => value,
                };
                base.insert(key, merged);
            }
            Value::Mapping(base)
        }
        (_, overlay) => overlay,
    }
}

fn deep_merge(base: Value, overlay: Value, root_mode: &InheritMode) -> Value {
    match (base, overlay) {
        (Value::Mapping(mut base), Value::Mapping(overlay)) => {
            // `should_union?` reads `inherit_mode` out of *both* hashes at the level it is
            // merging, so an inherited configuration can opt its own parameters into unioning
            // without the inheriting file knowing about it.
            let base_mode = inherit_mode(&base);
            let overlay_mode = inherit_mode(&overlay);
            for (key, value) in overlay {
                let merged = match base.remove(&key) {
                    Some(old) => merge_entry(
                        key.as_str(),
                        old,
                        value,
                        overlay_mode.as_ref(),
                        base_mode.as_ref(),
                        root_mode,
                    ),
                    None => value,
                };
                base.insert(key, merged);
            }
            Value::Mapping(base)
        }
        (_, overlay) => overlay,
    }
}

/// Merges one key of a mapping, mirroring the `merge_hashes?` / `should_union?` / plain-overwrite
/// ladder of `ConfigLoaderResolver#merge`.
fn merge_entry(
    key: Option<&str>,
    base: Value,
    overlay: Value,
    overlay_mode: Option<&InheritMode>,
    base_mode: Option<&InheritMode>,
    root_mode: &InheritMode,
) -> Value {
    // `merge_hashes?` is tested before `should_union?`, so two mappings always recurse.
    if base.is_mapping() && overlay.is_mapping() {
        return deep_merge(base, overlay, root_mode);
    }
    if should_union(key, &base, &overlay, overlay_mode, base_mode, root_mode) {
        return union_sequences(base, overlay);
    }
    overlay
}

/// Mirrors `ConfigLoaderResolver#should_union?`.
///
/// The precedence chain is what makes `override` usable at all: the derived side is consulted
/// first and an `override` entry there stops the search with "replace", which is the only way to
/// cancel a root-level `merge`. The base side gets its turn next, so an inherited file can request
/// unioning for its own parameters, and only then does the root mode decide.
fn should_union(
    key: Option<&str>,
    base: &Value,
    derived: &Value,
    derived_mode: Option<&InheritMode>,
    base_mode: Option<&InheritMode>,
    root_mode: &InheritMode,
) -> bool {
    if !base.is_sequence() && !derived.is_sequence() {
        return false;
    }
    let Some(key) = key else {
        return false;
    };
    if let Some(decision) = derived_mode.and_then(|mode| mode.decide(key)) {
        return decision;
    }
    if let Some(decision) = base_mode.and_then(|mode| mode.decide(key)) {
        return decision;
    }
    root_mode.merge_keys.contains(key)
}

/// Mirrors `Array(base_hash[key]) | Array(derived_hash[key])`, the union `ConfigLoaderResolver#merge`
/// performs once `should_union?` agrees. Ruby's `|` keeps the first occurrence of each value, so a
/// path listed on both sides ends up in the result once.
fn union_sequences(base: Value, derived: Value) -> Value {
    let mut union: Vec<Value> = Vec::new();
    for item in to_sequence(base).into_iter().chain(to_sequence(derived)) {
        if !union.contains(&item) {
            union.push(item);
        }
    }
    Value::Sequence(union)
}

/// Mirrors Ruby's `Array()`, which turns `nil` into `[]` and wraps anything that is not already an
/// array. `should_union?` only requires *one* of the two sides to be an array, so the other one is
/// allowed to be a bare scalar or absent.
fn to_sequence(value: Value) -> Vec<Value> {
    match value {
        Value::Null => Vec::new(),
        Value::Sequence(items) => items,
        other => vec![other],
    }
}

/// The `merge` and `override` lists of one `inherit_mode` directive.
#[derive(Debug, Default)]
struct InheritMode {
    merge_keys: HashSet<String>,
    override_keys: HashSet<String>,
}

impl InheritMode {
    /// `should_override?` and `should_merge?` collapsed into one lookup: `Some(false)` for
    /// "override wins", `Some(true)` for "merge wins", and `None` when this directive says nothing
    /// about the key and the next mode in `should_union?`'s chain gets its turn.
    fn decide(&self, key: &str) -> Option<bool> {
        if self.override_keys.contains(key) {
            Some(false)
        } else if self.merge_keys.contains(key) {
            Some(true)
        } else {
            None
        }
    }
}

/// Reads the `inherit_mode` directive of a single configuration level.
///
/// Returning `None` only for a missing (or explicitly null) key is what lets `RootMode::PerCop`
/// implement `local_inherit || hash['inherit_mode'] || {}` faithfully: a present but empty
/// directive is truthy in Ruby and therefore still replaces the file-level mode.
fn inherit_mode(mapping: &Mapping) -> Option<InheritMode> {
    let mode = mapping.get("inherit_mode")?;
    if mode.is_null() {
        return None;
    }
    Some(InheritMode {
        merge_keys: mode_keys(mode, "merge"),
        override_keys: mode_keys(mode, "override"),
    })
}

fn mode_keys(mode: &Value, name: &str) -> HashSet<String> {
    mode.as_mapping()
        .and_then(|mapping| mapping.get(name))
        .and_then(Value::as_sequence)
        .map(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread;

    use serde_yaml_ng::Value;
    use tempfile::tempdir;

    use super::{
        InheritMode, PARSED_CONFIG_COUNT, deep_merge, join_remote_url, load_with_inheritance,
        merge_config, merge_inherited_config,
    };
    use crate::config::Config;

    fn yaml(text: &str) -> Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    /// Number of configuration documents parsed since the last call, on this thread.
    fn taken_parse_count() -> usize {
        PARSED_CONFIG_COUNT.with(|count| count.replace(0))
    }

    fn resolve(path: &Path) -> anyhow::Result<Value> {
        load_with_inheritance(path, &mut HashSet::new())
    }

    fn serve_once(body: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            assert_ne!(stream.read(&mut request).unwrap(), 0);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        (format!("http://{address}/config.yml"), server)
    }

    #[test]
    fn remote_short_names_resolve_before_validation() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/config_cop_namespaces.json"
        ))
        .unwrap();
        let short = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == "short")
            .unwrap();
        let body = short["files"]["config.yml"].as_str().unwrap();
        let (url, server) = serve_once(body.as_bytes().to_vec());
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.yml");
        fs::write(&path, format!("inherit_from: {url}\n")).unwrap();
        let loaded = super::load_recorded(&path, &mut HashSet::new()).unwrap();
        server.join().unwrap();
        assert_eq!(loaded.value["Layout/SpaceAroundOperators"]["Enabled"], true);
        assert!(loaded.value.get("SpaceAroundOperators").is_none());

        let emitted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = std::sync::Arc::clone(&emitted);
        let warnings = crate::config::ConfigWarnings::new(false, false, move |warning| {
            let crate::config::ConfigWarning::Namespace(text) = warning else {
                panic!("未対応parameterを含まない本家対照である");
            };
            output.lock().unwrap().push(text.clone());
            Ok(())
        });
        warnings.emit(&loaded.validation.trace).unwrap();
        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        // 本家はremoteをcacheへ保存するためoriginが異なる (#201)。値と実測済み本文だけを照合する。
        assert_eq!(
            emitted[0].split_once(": Warning:").unwrap().1,
            short["stderr_lf"]
                .as_str()
                .unwrap()
                .split_once(": Warning:")
                .unwrap()
                .1
        );
    }

    #[test]
    fn aliases_share_parsing_but_keep_their_own_validation_traces() {
        let directory = tempdir().unwrap();
        let real = directory.path().join("configuration-origin-real-directory");
        fs::create_dir(&real).unwrap();
        #[cfg(unix)]
        let alias = {
            let alias = directory.path().join("alias");
            std::os::unix::fs::symlink(&real, &alias).unwrap();
            alias
        };
        #[cfg(windows)]
        let alias = {
            use std::ffi::OsString;
            use std::os::windows::ffi::{OsStrExt, OsStringExt};
            use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
            let input: Vec<_> = real.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut output = vec![0; 32_768];
            let length = unsafe {
                GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32)
            };
            assert!(length > 0 && length < output.len() as u32);
            let alias = std::path::PathBuf::from(OsString::from_wide(&output[..length as usize]));
            let hybrid = alias.parent().unwrap().join(real.file_name().unwrap());
            if alias == real || hybrid == real || hybrid == alias {
                eprintln!(
                    "未計測: long/short/hybridの3表記を作れないvolumeのため別origin対照を省略する"
                );
                return;
            }
            alias
        };
        fs::write(real.join("base.yml"), "inherit_from: leaf.yml\n").unwrap();
        fs::write(
            real.join("leaf.yml"),
            "SpaceAroundOperators:\n  Enabled: true\n",
        )
        .unwrap();
        let path = directory.path().join("root.yml");
        let paths = [real.join("base.yml"), alias.join("base.yml")];
        fs::write(
            &path,
            format!("inherit_from: {}\n", serde_json::to_string(&paths).unwrap()),
        )
        .unwrap();
        taken_parse_count();
        let loaded = super::load_recorded(&path, &mut HashSet::new()).unwrap();
        // 本家は両originで継承を辿るが、原本の読取とparseはcanonicalごとに一度でよい。
        assert_eq!(taken_parse_count(), 3);
        assert_eq!(loaded.validation.documents.len(), 5);
        let real_leaf = super::absolute_config_origin(&real.join("leaf.yml")).unwrap();
        let alias_leaf = super::absolute_config_origin(&alias.join("leaf.yml")).unwrap();
        assert_ne!(real_leaf, alias_leaf);
        assert!(!std::sync::Arc::ptr_eq(
            &loaded.validation.documents[&real_leaf],
            &loaded.validation.documents[&alias_leaf]
        ));
        assert_eq!(loaded.value["Layout/SpaceAroundOperators"]["Enabled"], true);
    }

    fn cop_exclude(merged: &Value, cop: &str) -> Value {
        merged
            .as_mapping()
            .unwrap()
            .get(cop)
            .unwrap()
            .as_mapping()
            .unwrap()
            .get("Exclude")
            .unwrap()
            .clone()
    }

    /// Writes `.rubocop.yml` plus `f0.yml`..`f{depth}.yml`, where every level names the next one
    /// twice. Resolving that costs 2^depth reads without memoisation and depth + 2 with it.
    fn write_diamond_chain(directory: &Path, depth: usize) {
        fs::write(
            directory.join(".rubocop.yml"),
            "inherit_from:\n  - f0.yml\n",
        )
        .unwrap();
        for level in 0..depth {
            fs::write(
                directory.join(format!("f{level}.yml")),
                format!(
                    "inherit_from:\n  - f{next}.yml\n  - f{next}.yml\n",
                    next = level + 1
                ),
            )
            .unwrap();
        }
        fs::write(
            directory.join(format!("f{depth}.yml")),
            "Layout/LineLength:\n  Max: 42\n",
        )
        .unwrap();
    }

    #[test]
    fn joins_remote_inherit_urls() {
        assert_eq!(
            join_remote_url("https://example.com/team/base.yml", "shared.yml").unwrap(),
            "https://example.com/team/shared.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com/team/nested/base.yml", "shared.yml").unwrap(),
            "https://example.com/team/nested/shared.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com/team/base.yml", "/root.yml").unwrap(),
            "https://example.com/root.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com", "/root.yml").unwrap(),
            "https://example.com/root.yml"
        );
        // A base without a path must not have the `//` of its scheme mistaken for a directory.
        assert_eq!(
            join_remote_url("https://example.com", "shared.yml").unwrap(),
            "https://example.com/shared.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com/", "shared.yml").unwrap(),
            "https://example.com/shared.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com/team/", "shared.yml").unwrap(),
            "https://example.com/team/shared.yml"
        );
        // A `/` inside a query string is not a directory separator either.
        assert_eq!(
            join_remote_url(
                "https://example.com/team/base.yml?ref=heads/main",
                "shared.yml"
            )
            .unwrap(),
            "https://example.com/team/shared.yml"
        );
        assert_eq!(
            join_remote_url("https://example.com?ref=heads/main", "shared.yml").unwrap(),
            "https://example.com/shared.yml"
        );
        assert_eq!(
            join_remote_url(
                "https://example.com/team/base.yml?ref=heads/main",
                "/root.yml"
            )
            .unwrap(),
            "https://example.com/root.yml"
        );
        assert!(join_remote_url("example.com/base.yml", "/root.yml").is_err());
        assert!(join_remote_url("no-directory", "shared.yml").is_err());
    }

    #[test]
    fn remote_configuration_enforces_the_response_size_limit() {
        let limit = 5 * 1024 * 1024;
        let directory = tempdir().unwrap();

        let (url, server) = serve_once(vec![b'a'; limit]);
        let accepted =
            crate::config::remote_cache::read(&url, &directory.path().join("accepted.yml"))
                .unwrap();
        server.join().unwrap();
        assert_eq!(accepted.len(), limit);

        let (url, server) = serve_once(vec![b'a'; limit + 1]);
        let rejected = directory.path().join("rejected.yml");
        let error = crate::config::remote_cache::read(&url, &rejected)
            .unwrap_err()
            .to_string();
        server.join().unwrap();
        assert!(!rejected.exists());
        assert!(
            error.starts_with("failed to read remote configuration:"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn remote_response_limit_precedes_text_mode_and_preserves_a_stale_cache_on_failure() {
        use std::fs::{File, FileTimes};
        use std::time::{Duration, SystemTime};

        let limit = 5 * 1024 * 1024;
        let directory = tempdir().unwrap();
        let path = directory.path().join("cache.yml");
        // Windows の実 File.write は改行ごとに CR を足す。保存後のサイズで上限を判定しない。
        let (url, server) = serve_once(vec![b'\n'; limit]);
        let accepted = crate::config::remote_cache::read(&url, &path).unwrap();
        server.join().unwrap();
        assert_eq!(accepted.as_bytes(), vec![b'\n'; limit]);
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            (limit * if cfg!(windows) { 2 } else { 1 }) as u64
        );

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old))
            .unwrap();
        let before = fs::read(&path).unwrap();
        let (url, server) = serve_once(vec![b'a'; limit + 1]);
        assert!(crate::config::remote_cache::read(&url, &path).is_err());
        server.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), old);
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
        let mode = InheritMode::default();
        assert_eq!(deep_merge(yaml("[1, 2]"), yaml("[3]"), &mode), yaml("[3]"));
        assert_eq!(
            deep_merge(yaml("a: 1\n"), yaml("b: 2\n"), &mode),
            yaml("a: 1\nb: 2\n")
        );
    }

    #[test]
    fn per_cop_override_mode_beats_the_root_merge_mode() {
        let merged = merge_inherited_config(
            yaml("Style/A:\n  Exclude: [a.rb]\n"),
            yaml(concat!(
                "inherit_mode:\n  merge:\n    - Exclude\n",
                "Style/A:\n",
                "  inherit_mode:\n    override:\n      - Exclude\n",
                "  Exclude: [b.rb]\n",
            )),
        );
        assert_eq!(cop_exclude(&merged, "Style/A"), yaml("[b.rb]"));
    }

    #[test]
    fn base_side_per_cop_merge_mode_unions_sequences() {
        let merged = merge_inherited_config(
            yaml("Style/A:\n  inherit_mode:\n    merge:\n      - Exclude\n  Exclude: [a.rb]\n"),
            yaml("Style/A:\n  Exclude: [b.rb]\n"),
        );
        assert_eq!(cop_exclude(&merged, "Style/A"), yaml("[a.rb, b.rb]"));
    }

    /// The two upstream entry points read a present-but-empty per-cop `inherit_mode` differently,
    /// and both readings are load-bearing: `resolve_inheritance` lets it cancel the file-level
    /// directive, while `merge_with_default` never consults it for the root mode at all.
    #[test]
    fn empty_per_cop_inherit_mode_replaces_the_root_mode_only_when_inheriting() {
        let overlay = concat!(
            "inherit_mode:\n  merge:\n    - Exclude\n",
            "Style/A:\n  inherit_mode: {}\n  Exclude: [b.rb]\n",
        );
        let inherited =
            merge_inherited_config(yaml("Style/A:\n  Exclude: [a.rb]\n"), yaml(overlay));
        assert_eq!(cop_exclude(&inherited, "Style/A"), yaml("[b.rb]"));

        let defaulted = merge_config(yaml("Style/A:\n  Exclude: [a.rb]\n"), yaml(overlay));
        assert_eq!(cop_exclude(&defaulted, "Style/A"), yaml("[a.rb, b.rb]"));
    }

    #[test]
    fn union_keeps_each_value_once() {
        let merged = merge_config(
            yaml("Style/A:\n  Exclude: [a.rb, b.rb]\n"),
            yaml("inherit_mode:\n  merge:\n    - Exclude\nStyle/A:\n  Exclude: [b.rb, c.rb]\n"),
        );
        assert_eq!(cop_exclude(&merged, "Style/A"), yaml("[a.rb, b.rb, c.rb]"));
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

    #[test]
    fn empty_configuration_parses_as_an_empty_mapping() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(".rubocop.yml");
        fs::write(&path, "").unwrap();
        assert_eq!(resolve(&path).unwrap(), yaml("{}"));

        // A comment-only file is the same story once the comment is stripped.
        fs::write(&path, "# nothing to configure yet\n").unwrap();
        assert_eq!(resolve(&path).unwrap(), yaml("{}"));
    }

    #[test]
    fn empty_configuration_keeps_the_default_configuration() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(directory.path().join(".rubocop.yml"), "").unwrap();
        fs::write(directory.path().join("a.rb"), "x = 1\n").unwrap();
        fs::create_dir_all(directory.path().join("node_modules/pkg")).unwrap();
        fs::write(directory.path().join("node_modules/pkg/v.rb"), "y = 2\n").unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        // `AllCops/Exclude` from `default.yml` has to survive, or `node_modules` gets linted.
        assert!(config.path_excluded(&directory.path().join("node_modules/pkg/v.rb")));
        assert!(!config.path_excluded(&directory.path().join("a.rb")));
        // And so do the per-cop defaults.
        assert_eq!(
            config.cop_value::<usize>("Layout/LineLength", "Max"),
            Some(120)
        );
    }

    #[test]
    fn non_mapping_configuration_is_rejected_as_malformed() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(".rubocop.yml");
        for contents in ["hello\n", "- Style/A\n", "42\n"] {
            fs::write(&path, contents).unwrap();
            let error = resolve(&path).unwrap_err().to_string();
            assert!(
                error.starts_with("Malformed configuration in"),
                "unexpected error for {contents:?}: {error}"
            );
        }
    }

    #[test]
    fn diamond_inheritance_reads_every_file_once() {
        let directory = tempdir().unwrap();
        write_diamond_chain(directory.path(), 8);
        taken_parse_count();

        let resolved = resolve(&directory.path().join(".rubocop.yml")).unwrap();

        // `.rubocop.yml` plus `f0.yml`..`f8.yml`; without memoisation this would be 2^9 - 1 + 1.
        assert_eq!(taken_parse_count(), 10);
        assert_eq!(
            resolved
                .as_mapping()
                .unwrap()
                .get("Layout/LineLength")
                .unwrap(),
            &yaml("Max: 42\n")
        );
    }

    #[test]
    fn detects_circular_inheritance() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "inherit_from: a.yml\n",
        )
        .unwrap();
        fs::write(directory.path().join("a.yml"), "inherit_from: b.yml\n").unwrap();
        fs::write(directory.path().join("b.yml"), "inherit_from: a.yml\n").unwrap();
        let error = resolve(&directory.path().join(".rubocop.yml"))
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("circular inherit_from detected at"),
            "unexpected error: {error}"
        );

        // A file inheriting from itself is the degenerate case of the same check.
        fs::write(directory.path().join("b.yml"), "inherit_from: b.yml\n").unwrap();
        let error = resolve(&directory.path().join(".rubocop.yml"))
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("circular inherit_from detected at"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn override_mode_wins_over_an_inherited_merge_mode_end_to_end() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join("base.yml"),
            "Layout/LineLength:\n  Exclude: [a.rb]\n",
        )
        .unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            concat!(
                "inherit_from: base.yml\n",
                "inherit_mode:\n  merge:\n    - Exclude\n",
                "Layout/LineLength:\n",
                "  inherit_mode:\n    override:\n      - Exclude\n",
                "  Exclude: [b.rb]\n",
            ),
        )
        .unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        assert_eq!(
            config.cop_value::<Vec<String>>("Layout/LineLength", "Exclude"),
            Some(vec!["b.rb".to_owned()])
        );
    }

    #[test]
    fn inherited_per_cop_merge_mode_wins_end_to_end() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join("base.yml"),
            "Layout/LineLength:\n  inherit_mode:\n    merge:\n      - Exclude\n  Exclude: [a.rb]\n",
        )
        .unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "inherit_from: base.yml\nLayout/LineLength:\n  Exclude: [b.rb]\n",
        )
        .unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        assert_eq!(
            config.cop_value::<Vec<String>>("Layout/LineLength", "Exclude"),
            Some(vec!["a.rb".to_owned(), "b.rb".to_owned()])
        );
    }

    /// `merge_with_default` never derives its root mode per cop, so an empty per-cop directive
    /// does not stop a file-level `merge` from unioning with `default.yml` -- and `node_modules`
    /// stays excluded.
    #[test]
    fn empty_per_cop_inherit_mode_still_unions_with_the_default_configuration() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            concat!(
                "inherit_mode:\n  merge:\n    - Exclude\n",
                "AllCops:\n  inherit_mode: {}\n  Exclude:\n    - custom/**/*\n",
            ),
        )
        .unwrap();
        fs::create_dir_all(directory.path().join("node_modules/pkg")).unwrap();
        fs::write(directory.path().join("node_modules/pkg/v.rb"), "y = 2\n").unwrap();
        fs::create_dir_all(directory.path().join("custom")).unwrap();
        fs::write(directory.path().join("custom/c.rb"), "z = 3\n").unwrap();
        fs::write(directory.path().join("a.rb"), "x = 1\n").unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        assert!(config.path_excluded(&directory.path().join("node_modules/pkg/v.rb")));
        assert!(config.path_excluded(&directory.path().join("custom/c.rb")));
        assert!(!config.path_excluded(&directory.path().join("a.rb")));
    }
}
