use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_long, c_void};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::config::{Config, ConfigStore};
use crate::cop_name::selector_matches;
use crate::diagnostic::{
    CorrectionCharacterSource, CorrectionRejection, Edit, FileReport, InsertionAnchor, Location,
    Offense, OffenseSnapshot, Severity,
};
use crate::directives::{CommentConfig, CopRegistry, DirectiveState};
use crate::magic_comment::MagicComment;
use crate::rules::{AstIndex, DirectiveReview, Rule, RuleContext, rules};
use crate::source::SourceFile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectMode {
    None,
    Safe,
    All,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Selection {
    pub only: Vec<String>,
    pub except: Vec<String>,
    pub disable_all: bool,
    pub enable_all: bool,
    pub enable_pending: bool,
    pub disable_pending: bool,
    pub safe_only: bool,
    pub ignore_disable_comments: bool,
    pub display_suppressed: bool,
    /// Whether the run was asked to correct, which a handful of cops branch on themselves.
    ///
    /// RuboCop exposes this to cops as `autocorrect?`, and `Layout/IndentationWidth` is the one
    /// that needs it here: it withholds the corrector from an offense nested inside one it already
    /// reported, since two overlapping shifts would corrupt the text. Without a correction pass
    /// there is nothing to corrupt, so it does not withhold, and the offense stays correctable.
    pub correcting: bool,
    /// `LSP.enabled?`, which `--editor-mode` and `--lsp` turn on (`lsp.rb:11-13`).
    ///
    /// It says an editor is driving the run rather than a batch job, and the one thing that hangs
    /// off it is `AutoCorrect: contextual`: a cop configured that way keeps its corrections to
    /// itself here. Half-typed code is the normal state of a buffer under an editor, and the 19
    /// cops the default configuration marks contextual are the ones that would read it as dead --
    /// `Lint/UselessAssignment` deleting an assignment whose use has not been typed yet.
    pub editor_mode: bool,
    /// Whether to skip the guard that refuses a correction leaving the file unparsable.
    ///
    /// **For tests, not for a run.** A cop test asks "is this correction the same as upstream's",
    /// which is a question about the cop; the guard answers "should this text be written", which
    /// is a question about the engine and is deliberately not the same as upstream. Mixing them
    /// makes every case where upstream writes broken Ruby look like a cop that lost its
    /// correction -- it cost three people an evening once already.
    ///
    /// The environment variable does the same thing but reaches the whole process, so it cannot
    /// be used by a harness that runs cases in parallel. **This is the per-case form.**
    pub skip_syntax_guard: bool,
    /// 検査対象外のファイルも含め、実行開始時に構築した本家互換の索引を全パスで共有する。
    #[serde(skip)]
    pub project_index: Option<Arc<crate::ProjectIndex>>,
}

impl Selection {
    pub(crate) fn with_project_index(
        &self,
        paths: &[PathBuf],
        configs: &ConfigStore,
    ) -> Result<Self> {
        let mut selection = self.clone();
        if selection.project_index.is_none() && crate::ProjectIndex::enabled(configs.root()) {
            selection.project_index = Some(Arc::new(crate::ProjectIndex::build(paths, configs)?));
        }
        Ok(selection)
    }
}

/// 5: raw message と初回の cop 名表示設定を分けて保存する。本文 digest・権限・位置の
/// 照合は維持し、formatter/debug だけの変更では最初の注釈を再利用する。
const RESULT_CACHE_SCHEMA: u32 = 5;
const RESULT_CACHE_INDEX_SCHEMA: u32 = 1;
const RESULT_CACHE_INDEX_SUMMARY_SCHEMA: u32 = 1;
const RESULT_CACHE_INDEX_PREFIX: &str = "sonicop-result-cache-";
const RESULT_CACHE_LOCK_FILE: &str = ".sonicop-result-cache.lock";
/// Upper bound on index JSON decoded per requested target. Loading more than this is generally
/// slower than inspecting the targets, especially for editor and changed-file runs. A small
/// index remains useful even for one target; a project-wide run naturally earns a larger budget.
const RESULT_CACHE_INDEX_BYTES_PER_TARGET: u64 = 128 * 1024;
/// Baseline disk budget shared by all project/selection indexes under one cache root. An active
/// index larger than this remains usable, but then it becomes the whole budget and older sessions
/// are discarded.
const RESULT_CACHE_ROOT_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// Persistent reports for unchanged files.
///
/// A cache identity includes the build fingerprint, not only the package version. Development
/// builds often keep the same version while their rules change; keying on every build input
/// prevents a freshly rebuilt linter from accepting reports produced by older code without making
/// each process read and hash its entire executable.
pub(crate) struct ResultCache {
    root: PathBuf,
    index_path: PathBuf,
    identity: blake3::Hash,
    max_files: usize,
    /// `None` until [`ResultCache::prepare`] decides that the index is economical for this run.
    /// It stays `None` when a narrow run would have to decode a disproportionally large index;
    /// both reads and writes then become no-ops so shutdown cannot reintroduce the same cost.
    entries: RwLock<Option<HashMap<String, IndexedReport>>>,
    prepared: OnceLock<bool>,
    dirty_keys: Mutex<HashSet<String>>,
    display_cop_names: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CachedReport {
    schema: u32,
    /// 診断時の stat。mtime は復元できるため、値が同じでも本文の digest を検証する。
    size: u64,
    modified: Option<(u64, u32)>,
    /// The permission bits, which `Lint/ScriptPermission` reads from the file itself. `chmod` moves
    /// neither the size nor the modification time nor a single byte of the text, so without this
    /// neither the stat nor the digest can tell that the answer has changed.
    mode: u32,
    /// The digest of the text the report was made from, for when the stat moved but the text did
    /// not -- a checkout that restores a file, a formatter that rewrites it byte for byte.
    text: [u8; 32],
    display_cop_names: Option<bool>,
    offenses: Vec<CachedOffense>,
}

#[derive(Clone, Serialize, Deserialize)]
struct IndexedReport {
    /// Wall-clock time at which this report was produced. It is used only to choose an entry when
    /// `MaxFilesInCache` requires eviction; cache validity never depends on it.
    cached_at: u64,
    report: CachedReport,
}

#[derive(Default, Serialize, Deserialize)]
struct CacheIndex {
    schema: u32,
    entries: HashMap<String, IndexedReport>,
}

#[derive(Serialize, Deserialize)]
struct CacheIndexSummary {
    schema: u32,
    entries: usize,
}

/// The directory entry a cached report is keyed on, as it stood when the run read the file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileStat {
    size: u64,
    modified: Option<(u64, u32)>,
    mode: u32,
}

/// The permission bits, or zero where the platform has none to give. Windows has no mode a cop
/// reads, so every file there compares equal and the field simply never decides anything.
#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    std::os::unix::fs::PermissionsExt::mode(&metadata.permissions())
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

impl CachedReport {
    /// Whether `text` is what this report was made from.
    fn matches(&self, text: &str) -> bool {
        self.text == *blake3::hash(text.as_bytes()).as_bytes()
    }

    /// The report as the rest of the program expects it.
    ///
    /// `text` is what the file holds, when the caller happens to have read it. Without it the
    /// report carries an empty [`SourceFile`]: every offense restored here has its location frozen,
    /// and both [`Offense::location`] and [`Offense::source_line`] answer from that rather than from
    /// the source, so nothing downstream asks the empty text a question. Autocorrect is the one
    /// caller that needs the real text, and it never reaches the cache -- a correcting run does not
    /// consult it.
    fn into_report(self, path: PathBuf, text: Option<&str>) -> Option<FileReport> {
        let source = SourceFile::new(path.clone(), text.unwrap_or_default().to_owned());
        let mut offenses = Vec::with_capacity(self.offenses.len());
        let display_cop_names = self.display_cop_names;
        for cached in self.offenses {
            let cop_name = rules().find(|rule| rule.name == cached.cop_name)?.name;
            let severity = Severity::parse(&cached.severity)?;
            let mut offense =
                Offense::new(cop_name, severity, cached.message, cached.start, cached.end);
            offense.cached_display_cop_names = display_cop_names;
            offense.correctable = cached.correctable;
            offense.suppressed = cached.suppressed;
            offense.justification = cached.justification;
            offense.snapshot = Some(OffenseSnapshot {
                location: cached.location,
                source_line: cached.source_line,
            });
            offenses.push(offense);
        }
        Some(FileReport {
            path,
            source,
            offenses,
        })
    }
}

/// What a lookup in the result cache found.
pub(crate) enum Cached {
    /// The file has not moved since the report was made; nothing else needs checking.
    Fresh(FileReport),
    /// The file's size or modification time moved. The report still stands if the text hashes to
    /// what it was made from, which the caller can only settle once it has read the file.
    Stale(CachedReport),
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedOffense {
    cop_name: String,
    severity: String,
    message: String,
    start: usize,
    end: usize,
    correctable: bool,
    suppressed: bool,
    justification: Option<String>,
    /// Where the offense sits, resolved while the text was still at hand. A run served from a stat
    /// match never reads the file, so the byte offsets above cannot be turned into a position any
    /// more -- the frozen location is what the formatters get.
    location: Location,
    source_line: String,
}

impl ResultCache {
    pub(crate) fn new(
        root: PathBuf,
        path_base: &Path,
        selection: &Selection,
        max_files: usize,
    ) -> Result<Self> {
        let mut identity = blake3::Hasher::new();
        hash_part(&mut identity, b"sonicop-result-cache");
        hash_part(&mut identity, &RESULT_CACHE_SCHEMA.to_le_bytes());
        hash_part(&mut identity, crate::VERSION.as_bytes());
        hash_part(&mut identity, env!("SONICOP_BUILD_FINGERPRINT").as_bytes());
        hash_part(&mut identity, path_base.as_os_str().as_encoded_bytes());
        hash_part(
            &mut identity,
            &serde_json::to_vec(selection).context("failed to fingerprint the cop selection")?,
        );
        let identity = identity.finalize();
        let index_path = root.join(format!(
            "{RESULT_CACHE_INDEX_PREFIX}{}.index",
            identity.to_hex()
        ));
        Ok(Self {
            root,
            index_path,
            identity,
            max_files,
            entries: RwLock::new(None),
            prepared: OnceLock::new(),
            dirty_keys: Mutex::new(HashSet::new()),
            display_cop_names: None,
        })
    }

    pub(crate) fn with_message_options(mut self, options: &str, display_cop_names: bool) -> Self {
        let mut identity = blake3::Hasher::new();
        hash_part(&mut identity, self.identity.as_bytes());
        hash_part(&mut identity, options.as_bytes());
        self.identity = identity.finalize();
        self.index_path = self.root.join(format!(
            "{RESULT_CACHE_INDEX_PREFIX}{}.index",
            self.identity.to_hex()
        ));
        self.display_cop_names = Some(display_cop_names);
        self
    }

    /// Loads the session index only when its size is proportionate to the number of targets.
    ///
    /// This is deliberately separate from [`ResultCache::new`]: target discovery has not happened
    /// when the CLI constructs the cache. A skipped index is also kept read-only for the run. If
    /// `prune` merged even one new report at shutdown it would have to decode the large index that
    /// this method avoided at startup, merely moving the latency to the other end of the command.
    pub(crate) fn prepare(&self, target_count: usize) {
        if self.prepared.get().is_some() {
            return;
        }
        let load = should_load_cache_index(&self.index_path, target_count);
        if load {
            let entries = crate::profile::phase(crate::profile::Phase::CacheLoad, || {
                load_cache_index(&self.index_path).entries
            });
            let Ok(mut slot) = self.entries.write() else {
                let _ = self.prepared.set(false);
                return;
            };
            *slot = Some(entries);
        }
        let _ = self.prepared.set(load);
    }

    /// パスと設定で索引を引き、保存済みの本文 digest と照合する。未登録のファイルは一度だけ読む。
    fn key(&self, path: &Path, config: &Config) -> Option<blake3::Hash> {
        let path = path.to_str()?;
        let config = config.cache_digest().ok()?;
        let mut key = blake3::Hasher::new();
        hash_part(&mut key, self.identity.as_bytes());
        hash_part(&mut key, path.as_bytes());
        hash_part(&mut key, config);
        Some(key.finalize())
    }

    /// The file's size, modification time and permission bits, as the cache records them.
    ///
    /// A modification time the platform will not give us leaves `None`, which never compares equal
    /// to a stored one, so such a file simply falls through to the digest.
    fn stat(metadata: &fs::Metadata) -> FileStat {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| (since.as_secs(), since.subsec_nanos()));
        FileStat {
            size: metadata.len(),
            modified,
            mode: file_mode(metadata),
        }
    }

    /// stat と本文を確認した報告か、再検査の要否を決める保存済み報告を返す。
    pub(crate) fn load(&self, path: &Path, config: &Config) -> Option<Cached> {
        if self.prepared.get() != Some(&true) {
            return None;
        }
        let key = crate::profile::phase(crate::profile::Phase::CacheKey, || {
            self.key(path, config).map(|key| key.to_hex().to_string())
        })?;
        crate::profile::phase(crate::profile::Phase::CacheLoad, || {
            let cached = {
                let entries = self.entries.read().ok()?;
                entries.as_ref()?.get(&key)?.report.clone()
            };
            if cached.schema != RESULT_CACHE_SCHEMA {
                return None;
            }
            let stat = fs::metadata(path).ok().as_ref().map(Self::stat)?;
            // 同じサイズで書き換えて mtime を戻しても、古い診断を返してはいけない。
            if cached.size == stat.size
                && stat.modified.is_some()
                && cached.modified == stat.modified
                && cached.mode == stat.mode
                && let Decoded::Text(text) = decoded_source(path).ok()?
                && cached.matches(&text)
            {
                return Some(Cached::Fresh(cached.into_report(path.to_path_buf(), None)?));
            }
            // The digest answers "are these the same bytes", which a `chmod` leaves true while the
            // answer a cop gives has changed. Only a fresh inspection settles that, so a report
            // whose mode moved is discarded rather than offered as stale.
            if cached.mode != stat.mode {
                return None;
            }
            Some(Cached::Stale(cached))
        })
    }

    /// 読み込み前の stat と、そのとき読んだ本文の digest を記録する。
    ///
    /// 読み込み後の stat は、途中の書き換えによって古い診断と新しい属性を組にする。
    /// 本文も毎回照合するが、再検査の判断に使う属性も診断と同じ入力へ対応させる。
    /// digest は NUL 処理より前の復号済み本文から計算する。処理後の report.source では
    /// 異なる原本が同じ値になり、キャッシュの入力同一性を失う。
    pub(crate) fn store(
        &self,
        report: &FileReport,
        stat: FileStat,
        digest: &[u8; 32],
        config: &Config,
    ) {
        if self.prepared.get() != Some(&true) {
            return;
        }
        let Some(key) = self.key(&report.path, config) else {
            return;
        };
        if self.max_files == 0 {
            return;
        }
        let cached = CachedReport {
            schema: RESULT_CACHE_SCHEMA,
            size: stat.size,
            modified: stat.modified,
            mode: stat.mode,
            text: *digest,
            // mtime だけ動いた同一本文を再保存するときも、読み込んだ初回の設定を保つ。
            display_cop_names: report
                .offenses
                .iter()
                .find_map(Offense::cached_display_cop_names)
                .or(self.display_cop_names),
            offenses: report
                .offenses
                .iter()
                .map(|offense| CachedOffense {
                    cop_name: offense.cop_name.to_owned(),
                    severity: offense.severity.as_str().to_owned(),
                    message: offense.message.clone(),
                    start: offense.start,
                    end: offense.end,
                    correctable: offense.correctable,
                    suppressed: offense.suppressed,
                    justification: offense.justification.clone(),
                    // キャッシュから返す診断は本文を持たないため、位置と行を検査時に確定する。
                    location: offense.location(&report.source),
                    source_line: offense.source_line(&report.source).to_owned(),
                })
                .collect(),
        };
        let key = key.to_hex().to_string();
        let Ok(mut entries_guard) = self.entries.write() else {
            return;
        };
        let Some(entries) = entries_guard.as_mut() else {
            return;
        };
        entries.insert(
            key.clone(),
            IndexedReport {
                cached_at: cache_clock(),
                report: cached,
            },
        );
        drop(entries_guard);
        let Ok(mut dirty_keys) = self.dirty_keys.lock() else {
            return;
        };
        dirty_keys.insert(key);
    }

    /// Persists reports once after the parallel inspection and enforces
    /// `AllCops/MaxFilesInCache`. A process lock and merge keep concurrent Sonicop runs from
    /// replacing one another's entries; the final temp-file rename keeps a killed writer from
    /// corrupting the previous index.
    pub(crate) fn prune(&self) {
        let dirty_keys = match self.dirty_keys.lock() {
            Ok(keys) => keys.iter().cloned().collect::<Vec<_>>(),
            Err(_) => return,
        };
        let local_len = match self.entries.read() {
            Ok(entries) => match entries.as_ref() {
                Some(entries) => entries.len(),
                None => return,
            },
            Err(_) => return,
        };
        if dirty_keys.is_empty() && local_len <= self.max_files {
            return;
        }
        if fs::create_dir_all(&self.root).is_err() {
            return;
        }
        let lock_path = self.root.join(RESULT_CACHE_LOCK_FILE);
        let Ok(lock) = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
        else {
            return;
        };
        if fs2::FileExt::lock_exclusive(&lock).is_err() {
            return;
        }

        let mut index = load_cache_index(&self.index_path);
        index.schema = RESULT_CACHE_INDEX_SCHEMA;
        if let Ok(entries) = self.entries.read() {
            let Some(entries) = entries.as_ref() else {
                return;
            };
            for key in &dirty_keys {
                if let Some(entry) = entries.get(key) {
                    index.entries.insert(key.clone(), entry.clone());
                }
            }
        } else {
            return;
        }
        let evicted = prune_cache_entries(&mut index.entries, self.max_files);
        if dirty_keys.is_empty() && evicted.is_empty() {
            return;
        }
        if !persist_cache_index(&self.index_path, &index) {
            return;
        }

        if let Ok(mut entries) = self.entries.write()
            && let Some(entries) = entries.as_mut()
        {
            for key in &evicted {
                entries.remove(key);
            }
        }
        if let Ok(mut keys) = self.dirty_keys.lock() {
            for key in &dirty_keys {
                keys.remove(key);
            }
        }
        evict_old_cache_indexes(&self.root, &self.index_path, self.max_files);
        remove_legacy_cache_entries(&self.root);
    }
}

impl Drop for ResultCache {
    fn drop(&mut self) {
        self.prune();
    }
}

fn empty_cache_index() -> CacheIndex {
    CacheIndex {
        schema: RESULT_CACHE_INDEX_SCHEMA,
        entries: HashMap::new(),
    }
}

fn should_load_cache_index(path: &Path, target_count: usize) -> bool {
    if target_count == 0 {
        return false;
    }
    let Ok(metadata) = fs::metadata(path) else {
        // A new session starts empty, so even a one-file run should be allowed to populate it.
        return true;
    };
    let budget = (target_count as u64).saturating_mul(RESULT_CACHE_INDEX_BYTES_PER_TARGET);
    metadata.len() <= budget
}

fn load_cache_index(path: &Path) -> CacheIndex {
    let Ok(bytes) = fs::read(path) else {
        return empty_cache_index();
    };
    let Ok(mut index) = serde_json::from_slice::<CacheIndex>(&bytes) else {
        return empty_cache_index();
    };
    if index.schema != RESULT_CACHE_INDEX_SCHEMA {
        return empty_cache_index();
    }
    index
        .entries
        .retain(|_, entry| entry.report.schema == RESULT_CACHE_SCHEMA);
    index
}

fn persist_cache_index(path: &Path, index: &CacheIndex) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    let Ok(mut temporary) = NamedTempFile::new_in(parent) else {
        return false;
    };
    // **Buffered.** `serde_json::to_writer` hands the writer each token as it produces it, so an
    // unbuffered `File` turns one index into millions of `write` calls. The index holds every
    // offense of every file with the source line it was found on, which is 336 MB over `ruby/ruby`:
    // writing it a token at a time cost 115 seconds of a 135-second run, nearly all of it blocked
    // in the kernel rather than serializing.
    let written = {
        let mut writer = std::io::BufWriter::with_capacity(1 << 20, temporary.as_file_mut());
        serde_json::to_writer(&mut writer, index).is_ok() && writer.flush().is_ok()
    };
    if !written
        || temporary.as_file_mut().flush().is_err()
        || temporary.as_file().sync_all().is_err()
    {
        return false;
    }
    if temporary.persist(path).is_err() {
        return false;
    }
    let summary = CacheIndexSummary {
        schema: RESULT_CACHE_INDEX_SUMMARY_SCHEMA,
        entries: index.entries.len(),
    };
    let Ok(mut temporary) = NamedTempFile::new_in(parent) else {
        return false;
    };
    if serde_json::to_writer(temporary.as_file_mut(), &summary).is_err()
        || temporary.as_file_mut().flush().is_err()
        || temporary.as_file().sync_all().is_err()
        || temporary.persist(cache_index_summary_path(path)).is_err()
    {
        return false;
    }
    // The index itself is already synced above. Syncing the directory where the platform permits
    // it also makes the rename durable across a sudden power loss.
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    true
}

fn cache_index_summary_path(path: &Path) -> PathBuf {
    let mut summary = path.as_os_str().to_owned();
    summary.push(".summary");
    PathBuf::from(summary)
}

fn load_cache_index_summary(path: &Path) -> Option<usize> {
    let bytes = fs::read(cache_index_summary_path(path)).ok()?;
    let summary = serde_json::from_slice::<CacheIndexSummary>(&bytes).ok()?;
    (summary.schema == RESULT_CACHE_INDEX_SUMMARY_SCHEMA).then_some(summary.entries)
}

fn remove_cache_index(path: &Path) -> bool {
    let removed = fs::remove_file(path).is_ok();
    let _ = fs::remove_file(cache_index_summary_path(path));
    removed
}

fn cache_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn prune_cache_entries(
    entries: &mut HashMap<String, IndexedReport>,
    max_files: usize,
) -> Vec<String> {
    if entries.len() <= max_files {
        return Vec::new();
    }
    let mut oldest = entries
        .iter()
        .map(|(key, entry)| (entry.cached_at, key.clone()))
        .collect::<Vec<_>>();
    oldest.sort_unstable();
    let excess = entries.len() - max_files;
    let evicted = oldest
        .into_iter()
        .take(excess)
        .map(|(_, key)| key)
        .collect::<Vec<_>>();
    for key in &evicted {
        entries.remove(key);
    }
    evicted
}

fn is_owned_cache_index(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(hash) = name
        .strip_prefix(RESULT_CACHE_INDEX_PREFIX)
        .and_then(|name| name.strip_suffix(".index"))
    else {
        return false;
    };
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Keeps `MaxFilesInCache` and a byte ceiling as global budgets even though projects, selections,
/// and executables live in separate indexes. Whole old sessions are discarded, so startup still
/// reads exactly one index rather than opening every historical session.
///
/// Entry counts come from tiny sidecars. Reading every old index to count it would make a narrow
/// run pay the JSON-decoding cost that [`ResultCache::prepare`] deliberately avoided. Indexes made
/// before sidecars existed are unreachable after this binary change and are removed without being
/// decoded.
fn evict_old_cache_indexes(root: &Path, current: &Path, max_files: usize) {
    let Ok(indexes) = fs::read_dir(root) else {
        return;
    };
    let mut total_entries = 0usize;
    let mut total_bytes = 0u64;
    let mut current_bytes = 0u64;
    let mut old = Vec::new();
    for entry in indexes.filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) || !is_owned_cache_index(&path) {
            continue;
        }
        let Some(count) = load_cache_index_summary(&path) else {
            if path != current {
                remove_cache_index(&path);
            }
            continue;
        };
        let metadata = entry.metadata().ok();
        let size = metadata.as_ref().map_or(0, fs::Metadata::len);
        total_entries = total_entries.saturating_add(count);
        total_bytes = total_bytes.saturating_add(size);
        if path == current {
            current_bytes = size;
        } else {
            let modified = metadata
                .and_then(|metadata| metadata.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            old.push((modified, path, count, size));
        }
    }
    let byte_budget = RESULT_CACHE_ROOT_MAX_BYTES.max(current_bytes);
    if total_entries <= max_files && total_bytes <= byte_budget {
        return;
    }
    old.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    for (_, path, count, size) in old {
        if total_entries <= max_files && total_bytes <= byte_budget {
            break;
        }
        if remove_cache_index(&path) {
            total_entries = total_entries.saturating_sub(count);
            total_bytes = total_bytes.saturating_sub(size);
        }
    }
}

/// Removes only files matching Sonicop's former `<hash-prefix>/<hash>.json` layout. A custom
/// `--cache-root` may be shared, so unrelated files and non-empty directories are never removed.
fn remove_legacy_cache_entries(root: &Path) {
    let Ok(shards) = fs::read_dir(root) else {
        return;
    };
    for shard in shards.filter_map(Result::ok).filter(|entry| {
        let name = entry.file_name();
        entry.file_type().is_ok_and(|kind| kind.is_dir())
            && name.len() == 2
            && name
                .to_str()
                .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_hexdigit()))
    }) {
        let Ok(files) = fs::read_dir(shard.path()) else {
            continue;
        };
        for entry in files.filter_map(Result::ok) {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if entry.file_type().is_ok_and(|kind| kind.is_file())
                && path.extension().and_then(|extension| extension.to_str()) == Some("json")
                && stem.len() == 64
                && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    == Some(&stem[..2])
            {
                let _ = fs::remove_file(path);
            }
        }
        let _ = fs::remove_dir(shard.path());
    }
}

fn hash_part(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// RuboCop refuses to let syntax checking be turned off, so the cop stays on no matter how it is
/// selected away. Both the `--except` guard and cop selection have to agree on the names that
/// denote it, including the legacy `Syntax` spelling RuboCop still accepts.
pub fn is_mandatory_cop(name: &str) -> bool {
    matches!(name, "Lint/Syntax" | "Syntax")
}

/// The cop that reads the file's own `rubocop:disable` comments back to it.
pub const REDUNDANT_COP_DISABLE_DIRECTIVE: &str = "Lint/RedundantCopDisableDirective";

/// `Runner::REDUNDANT_COP_DISABLE_DIRECTIVE_RULES`: the selectors that switch the check off.
const REDUNDANT_COP_DISABLE_DIRECTIVE_RULES: [&str; 3] = [
    "Lint/RedundantCopDisableDirective",
    "RedundantCopDisableDirective",
    "Lint",
];

impl Selection {
    pub fn includes(&self, name: &str, configured_enabled: bool, safe: bool) -> bool {
        if is_mandatory_cop(name) {
            return true;
        }
        let explicitly_selected = self
            .only
            .iter()
            .any(|selection| selector_matches(selection, name));
        let selected = if self.only.is_empty() {
            if self.disable_all {
                false
            } else if self.enable_all {
                true
            } else {
                configured_enabled
            }
        } else {
            explicitly_selected
        };
        selected
            && (!self.safe_only || safe)
            && !self
                .except
                .iter()
                .any(|except| selector_matches(except, name))
    }

    /// `Runner#check_for_redundant_disables?`.
    ///
    /// RuboCop never runs `Lint/RedundantCopDisableDirective` under `--only`: the cop asks whether
    /// the rest of the run had anything to say, and a run narrowed to a few cops cannot answer
    /// that. `-l` and `-x` go through `--only` too, so they switch it off as well. Naming the cop
    /// in `--except`, by department or without one, switches it off outright.
    pub fn checks_redundant_directives(&self) -> bool {
        self.only.is_empty()
            && !self
                .except
                .iter()
                .any(|except| REDUNDANT_COP_DISABLE_DIRECTIVE_RULES.contains(&except.as_str()))
    }
}

/// The cops a run applies, with every configuration decision that does not depend on the file
/// resolved once.
///
/// Resolving `Enabled`, `Severity` and `SafeAutoCorrect` out of YAML costs a lookup per cop per
/// file, which is work that grows with the registry as it fills out RuboCop's full cop set. Only
/// `Exclude` reads the path being inspected, so it is all that stays per-file.
pub(crate) struct RulePlan {
    entries: Vec<PlannedRule>,
    /// The cops the configuration switched off that a file could ask back with an `enable`
    /// directive. Upstream keeps the whole registry on standby for exactly this and mobilizes one
    /// when `CommentConfig#opt_in_cops` names it, so the selection is settled here but the decision
    /// is per file.
    standby: Vec<PlannedRule>,
    /// `registry.disabled_names(config)`: the same set as [`Self::standby`], as the two directive
    /// cops read it -- by name, in registry order.
    standby_names: Vec<&'static str>,
    /// What the cop that reads directives needs to know about the cops that exist. Built once per
    /// configuration because it depends on nothing in the file, and only when that cop will run.
    directive_registry: Option<CopRegistry>,
}

struct PlannedRule {
    rule: &'static Rule,
    /// The cop's position in the registry, which is the slot `crate::profile` tallies it under.
    index: usize,
    /// `rule.severity` unless the configuration overrode it.
    severity: Severity,
    safe_autocorrect: bool,
    /// [`Config::rule_autocorrect_enabled`]: whether the cop's own `AutoCorrect` setting lets the
    /// run use its corrector at all.
    autocorrect: bool,
}

impl PlannedRule {
    /// Settles what `Base#use_corrector` (`cop/base.rb:445-453`) would have made of the offenses a
    /// cop has just reported.
    ///
    /// Two of its three statuses are decided here. A cop the configuration switched autocorrection
    /// off for yields `:unsupported`: the offense is still reported, but it is not `correctable?`
    /// and its edits never reach the run's corrector, so they are dropped rather than merely left
    /// unapplied -- [`correction_candidates`] takes any offense that still carries edits. The
    /// edits a cop scheduled outside `add_offense` go with them, since upstream reaches
    /// `apply_correction` only through the branch this one replaces.
    ///
    /// `SafeAutoCorrect` is the other half and is not the same question: it leaves the offense
    /// correctable and only bars `-a` from taking it, which is why `-A` still applies those edits.
    fn settle_corrections(&self, reported: &mut [Offense]) {
        if !self.autocorrect {
            for offense in reported {
                offense.clear_corrections();
                offense.correctable = false;
            }
            return;
        }
        if !self.safe_autocorrect {
            for offense in reported {
                for correction in offense.planned_corrections_mut() {
                    correction.safe = false;
                }
            }
        }
    }
}

impl RulePlan {
    pub(crate) fn build(config: &Config, selection: &Selection) -> Self {
        let planned = |index: usize, rule: &'static Rule| PlannedRule {
            rule,
            index,
            severity: config
                .cop_value::<String>(rule.name, "Severity")
                .and_then(|value| Severity::parse(&value))
                .unwrap_or(rule.severity),
            // `AutocorrectLogic#safe_autocorrect?` is both halves: a cop whose analysis is
            // unsafe cannot have a safe correction either, however `SafeAutoCorrect` was left.
            safe_autocorrect: config.rule_safe(rule.name)
                && config.rule_safe_autocorrect(rule.name),
            autocorrect: config.rule_autocorrect_enabled(rule.name, selection.editor_mode),
        };
        let mut entries = Vec::new();
        let mut standby = Vec::new();
        for (index, rule) in rules().enumerate() {
            let safe = config.rule_safe(rule.name);
            let enabled = config.rule_enabled_with_pending(
                rule.name,
                selection.enable_pending,
                selection.disable_pending,
            );
            if selection.includes(rule.name, enabled, safe) {
                entries.push(planned(index, rule));
            } else if !enabled && selection.includes(rule.name, true, safe) {
                // Only the configuration stands in the way, which is what an `enable` directive can
                // undo. A cop the run itself left out (`--only`, `--except`, the safety filters)
                // stays out however the file is written.
                standby.push(planned(index, rule));
            }
        }
        let directive_registry = (selection.checks_redundant_directives()
            && entries
                .iter()
                .any(|planned| planned.rule.name == REDUNDANT_COP_DISABLE_DIRECTIVE))
        .then(|| CopRegistry::new(config, selection));
        let standby_names = standby.iter().map(|planned| planned.rule.name).collect();
        Self {
            entries,
            standby,
            standby_names,
            directive_registry,
        }
    }

    /// Whether the run reports redundant directives at all, which decides whether the autocorrect
    /// loop has to leave a pass for that cop.
    fn checks_directives(&self) -> bool {
        self.directive_registry.is_some()
    }
}

/// The cop that reports parse failures. Every file is put to it before any other cop runs, so it
/// is looked up by name rather than taken from the run's plan, which a configuration could have
/// excluded it from without making the file inspectable.
fn syntax_rule() -> &'static Rule {
    static RULE: OnceLock<&'static Rule> = OnceLock::new();
    RULE.get_or_init(|| {
        rules()
            .find(|rule| rule.name == "Lint/Syntax")
            .expect("the registry always carries Lint/Syntax")
    })
}

/// Inspects source read from standard input, decoded exactly as the same bytes on disk would be.
///
/// `Runner#get_processed_source` (`runner.rb:623-633`) builds the same `ProcessedSource` for
/// `--stdin` as for a file; `ProcessedSource.from_file` differs only in doing the `File.read`
/// first. So `--stdin` gets the magic comment's encoding applied, and bytes that are not valid
/// UTF-8 get the fatal `Lint/Syntax` offense a file's would -- rather than aborting the run, which
/// leaves a `--format json` caller with no JSON to read and an exit code it cannot interpret.
pub fn inspect_stdin(
    path: impl Into<PathBuf>,
    bytes: Vec<u8>,
    config: &Config,
    selection: &Selection,
) -> Result<FileReport> {
    let path = path.into();
    match decoded_bytes(bytes) {
        Decoded::Text(text) => inspect_source(path, text, config, selection),
        Decoded::Undecodable(message) => Ok(undecodable_report(&path, &message)),
    }
}

/// 共有 stdin は補正後の LF view と物理 CR の由来を同時に持つ。新しい原本として読み直さない。
#[derive(Clone, Default)]
pub(crate) struct SharedStdinState {
    logical: Option<String>,
    retained_crlf: Option<LineEndingProvenance>,
    preserve_crlf: bool,
}

pub(crate) fn inspect_shared_stdin(
    path: PathBuf,
    bytes: &[u8],
    state: &SharedStdinState,
    config: &Config,
    selection: &Selection,
) -> Result<FileReport> {
    if let Some(logical) = &state.logical {
        // UTF-8 の解析 String を legacy 宣言で二重に復号しない。
        inspect_source(path, logical.clone(), config, selection)
    } else {
        inspect_stdin(path, bytes.to_vec(), config, selection)
    }
}

pub fn inspect_source(
    path: impl Into<PathBuf>,
    text: String,
    config: &Config,
    selection: &Selection,
) -> Result<FileReport> {
    let path = path.into();
    let indexed = if selection.project_index.is_none() && crate::ProjectIndex::enabled(config) {
        let configs = ConfigStore::new(config.clone(), false, false);
        Some(selection.with_project_index(std::slice::from_ref(&path), &configs)?)
    } else {
        None
    };
    let selection = indexed.as_ref().unwrap_or(selection);
    inspect_planned(
        path,
        text,
        config,
        selection,
        &RulePlan::build(config, selection),
        true,
        None,
    )
}

/// Inspects one file against an already-resolved [`RulePlan`], which must have been built from
/// `config` and `selection`.
///
/// `check_directives` is off for the passes of an autocorrect loop that follow the one the
/// directive cop was given: RuboCop mobilizes that cop once and the loop it then runs does not
/// carry it, so a directive that only became redundant afterwards goes unreported.
fn inspect_planned(
    path: impl Into<PathBuf>,
    text: String,
    config: &Config,
    selection: &Selection,
    plan: &RulePlan,
    check_directives: bool,
    raw_source: Option<String>,
) -> Result<FileReport> {
    // NULの代替と打ち切りはtree-sitter用の投影だけに適用する。原本をcopと補正へ渡し、
    // コメントのNULや字句器が読まない後続本文を無関係な補正で変えない。
    // NUL分類と本体parseには同じ対象Ruby・パーサーの束縛を使う。
    let semantics = crate::regexp_ast::Semantics::from_config(config);
    let parsed_text = crate::nul_bytes::as_ruby_reads_it_configured(&text, semantics);
    let source = SourceFile::new(path, text).with_raw_source(raw_source);
    let tree = crate::profile::phase(crate::profile::Phase::Parse, || {
        crate::parser::parse_configured(parsed_text.as_deref().unwrap_or(source.text()), semantics)
    })
    .context("Ruby parser returned no syntax tree")?;
    let ast = crate::profile::phase(crate::profile::Phase::Index, || {
        AstIndex::new(tree.root_node(), source.text())
    });
    // `opted_in_standby_cops`: a cop the configuration switched off is put back on duty for this
    // file when an `enable` directive names it. The names are read once and used twice -- to pick
    // the cops out of the plan's standby list, and to seed the directive state so that only what
    // the `enable` opens is reported.
    let opted_in: Vec<&PlannedRule> = if plan.standby.is_empty() {
        Vec::new()
    } else {
        let names = crate::directives::opted_in_cops(&source, ast.comment_ranges());
        plan.standby
            .iter()
            .filter(|planned| names.contains(planned.rule.name))
            .collect()
    };
    let disabled_by_config: Vec<&str> = opted_in.iter().map(|planned| planned.rule.name).collect();
    let directives = crate::profile::phase(crate::profile::Phase::Directives, || {
        (!selection.ignore_disable_comments).then(|| {
            DirectiveState::parse_opting_in(
                &source,
                ast.comment_ranges(),
                config.prevents_directive_disabling(),
                &disabled_by_config,
            )
        })
    });
    let (mut offenses, valid_syntax) =
        inspect_registered_rules(&source, &ast, config, selection, plan, &opted_in);

    // `Runner#add_redundant_disables`, which happens once the inspection loop is done and is
    // handed the offenses as they were found -- the ones a directive suppressed included, since a
    // directive that suppressed something was plainly needed.
    if valid_syntax
        && check_directives
        && let Some(registry) = &plan.directive_registry
        && let Some(planned) = plan
            .entries
            .iter()
            .find(|planned| planned.rule.name == REDUNDANT_COP_DISABLE_DIRECTIVE)
        && config.rule_included(planned.rule.name, source.path())
        && !config.rule_excluded(planned.rule.name, source.path())
    {
        let comments = CommentConfig::analyze(&source, ast.comment_ranges(), registry);
        if !comments.is_empty() {
            let review = DirectiveReview {
                offenses: &offenses,
                comments: &comments,
                registry,
            };
            let context = RuleContext::new(
                &source,
                &ast,
                config,
                planned.rule,
                planned.severity,
                selection.correcting,
            )
            .reviewing_directives(&review);
            let mut reported = Vec::new();
            (planned.rule.check)(&context, &mut reported);
            planned.settle_corrections(&mut reported);
            offenses.append(&mut reported);
        }
    }

    if let Some(directives) = directives {
        offenses.retain_mut(|offense| {
            let Some(justification) = directives.suppression(offense, &source) else {
                return true;
            };
            offense.suppressed = true;
            offense.justification = justification;
            selection.display_suppressed
        });
    }
    crate::profile::phase(crate::profile::Phase::Sort, || {
        sort_offenses(&mut offenses, &source);
        dedupe_offenses(&mut offenses, &source);
    });

    Ok(FileReport {
        path: source.path().to_path_buf(),
        source,
        offenses,
    })
}

/// Runs the syntax cop and every ordinary cop selected for one file.
///
/// The directive-review cop is deliberately excluded: it needs the complete offense list this
/// function returns and is therefore run by [`inspect_planned`] afterwards.
fn inspect_registered_rules<'a>(
    source: &'a SourceFile,
    ast: &'a AstIndex<'a>,
    config: &'a Config,
    selection: &Selection,
    plan: &'a RulePlan,
    opted_in: &[&'a PlannedRule],
) -> (Vec<Offense>, bool) {
    // RuboCop's `Commissioner#investigate` walks the syntax tree only for a source that parses;
    // otherwise it calls `on_other_file`, which `Lint/Syntax` alone implements. A file that does
    // not parse therefore reports its syntax errors and nothing else, however the run selected its
    // cops -- including when `Lint/Syntax` is itself excluded from the file and reports nothing.
    let syntax_rule = syntax_rule();
    let mut syntax_offenses = Vec::new();
    let syntax_severity = plan
        .entries
        .iter()
        .find(|planned| planned.rule.name == syntax_rule.name)
        .map_or(syntax_rule.severity, |planned| planned.severity);
    crate::profile::phase(crate::profile::Phase::Syntax, || {
        (syntax_rule.check)(
            &RuleContext::new(
                source,
                ast,
                config,
                syntax_rule,
                syntax_severity,
                selection.correcting,
            ),
            &mut syntax_offenses,
        );
    });
    let valid_syntax = syntax_offenses.is_empty();
    let mut offenses = Vec::new();

    // One context for every cop of the file. Beyond saving the construction, it is what lets the
    // analyses a cop asks for -- `VariableForce` above all -- be computed once and reused by the
    // cops that follow, the way upstream's commissioner runs one force for the whole team.
    let mut context = RuleContext::new(
        source,
        ast,
        config,
        syntax_rule,
        syntax_severity,
        selection.correcting,
    )
    // `registry.disabled_names(config)`: the cops the run switches off, which is what an
    // `# rubocop:enable` has to undo. The standby list is that set already.
    .with_disabled_cops(&plan.standby_names)
    .with_project_index(selection.project_index.as_deref());
    for planned in plan.entries.iter().chain(opted_in.iter().copied()) {
        let rule = planned.rule;
        // `Cop::Base#relevant_file?`: a cop applies to a file its own `Include` reaches and its own
        // `Exclude` does not, which is how a `Bundler` cop stays off everything but a Gemfile.
        if !config.rule_included(rule.name, source.path())
            || config.rule_excluded(rule.name, source.path())
        {
            continue;
        }
        if rule.name == syntax_rule.name {
            offenses.append(&mut syntax_offenses);
            continue;
        }
        // The directive cop reads what every other cop found, so it cannot run in the same pass.
        if rule.name == REDUNDANT_COP_DISABLE_DIRECTIVE {
            continue;
        }
        if !valid_syntax {
            continue;
        }
        context.inspecting_with(rule, planned.severity);
        let start = offenses.len();
        crate::profile::rule(planned.index, || (rule.check)(&context, &mut offenses));
        // The cop's name comes from the registry through `RuleContext`, so a mismatch here means
        // an offense was built outside `context.offense` and would be attributed to a cop that
        // never ran -- directives and severity overrides would both consult the wrong entry.
        debug_assert!(
            offenses[start..]
                .iter()
                .all(|offense| offense.cop_name == rule.name),
            "{} reported an offense under another cop's name",
            rule.name
        );
        planned.settle_corrections(&mut offenses[start..]);
    }

    (offenses, valid_syntax)
}

pub fn inspect_files(
    paths: &[PathBuf],
    config: &Config,
    selection: &Selection,
    parallel: bool,
) -> Result<Vec<FileReport>> {
    let configs = ConfigStore::new(config.clone(), false, false);
    inspect_files_with_store(paths, &configs, selection, parallel)
}

pub fn inspect_files_with_store(
    paths: &[PathBuf],
    configs: &ConfigStore,
    selection: &Selection,
    parallel: bool,
) -> Result<Vec<FileReport>> {
    inspect_files_with_store_cached(paths, configs, selection, parallel, None)
}

pub(crate) fn inspect_files_with_store_cached(
    paths: &[PathBuf],
    configs: &ConfigStore,
    selection: &Selection,
    parallel: bool,
    cache: Option<&ResultCache>,
) -> Result<Vec<FileReport>> {
    inspect_file_results_with_store_cached(paths, configs, selection, parallel, cache)?
        .into_iter()
        .collect()
}

// 並列の完了順ではなく入力順の成功/失敗を保持し、CLIが成功prefixを終了reportへ渡す。
pub(crate) fn inspect_file_results_with_store_cached(
    paths: &[PathBuf],
    configs: &ConfigStore,
    selection: &Selection,
    parallel: bool,
    cache: Option<&ResultCache>,
) -> Result<Vec<Result<FileReport>>> {
    let indexed = selection.with_project_index(paths, configs)?;
    let selection = &indexed;
    // 他ファイルの追加・削除も診断を変える。索引の永続化を導入するまでは古い結果を使わない。
    let cache = cache.filter(|_| selection.project_index.is_none());
    // Most runs resolve every file to the store's root configuration, so the plan for it is worth
    // building once. A file that a nested `.rubocop.yml` gives a different configuration falls back
    // to building its own, which costs no more than resolving the cops inline would have.
    let root_plan = RulePlan::build(configs.root(), selection);
    let inspect = |path: &PathBuf| -> Result<FileReport> {
        let config = configs.for_path_observed(path)?;
        // 有効なキャッシュは load が本文を照合済み。その他は読み込んだ本文で再検査する。
        let stale = match cache.and_then(|cache| cache.load(path, &config)) {
            Some(Cached::Fresh(report)) => return Ok(report),
            Some(Cached::Stale(cached)) => Some(cached),
            None => None,
        };
        // Taken before the read, so that a rewrite landing between the two leaves a stat the next
        // run will not accept. See [`ResultCache::store`].
        let stat = cache.and_then(|_| fs::metadata(path).ok().as_ref().map(ResultCache::stat));
        let text =
            match crate::profile::phase(crate::profile::Phase::Read, || decoded_source(path))? {
                Decoded::Text(text) => text,
                Decoded::Undecodable(message) => return Ok(undecodable_report(path, &message)),
            };
        // Both describe the bytes on disk, which `inspect_planned` is about to rewrite. A NUL can
        // make Ruby stop reading before the physical end of the file, and preserving both lengths
        // in the cache buys less than keeping this path unambiguous, so such a file is not cached.
        let digest = *blake3::hash(text.as_bytes()).as_bytes();
        let cacheable = !text.as_bytes().contains(&0);
        if let Some(cached) = stale
            && cached.matches(&text)
            && let Some(report) = cached.into_report(path.clone(), Some(&text))
        {
            // The text is confirmed, so refresh the entry's stat. Without this a file whose stat
            // moved but whose bytes did not -- a `touch`, a `git checkout` that restores it -- is
            // read and digested on every later run, having permanently lost the stat fast path.
            if let Some(cache) = cache
                && cacheable
                && let Some(stat) = stat
            {
                cache.store(&report, stat, &digest, &config);
            }
            return Ok(report);
        }
        let own_plan = (!std::ptr::eq(Arc::as_ptr(&config), configs.root()))
            .then(|| RulePlan::build(&config, selection));
        let report = inspect_planned(
            path.clone(),
            text,
            &config,
            selection,
            own_plan.as_ref().unwrap_or(&root_plan),
            true,
            None,
        )?;
        if let Some(cache) = cache
            && cacheable
            && let Some(stat) = stat
        {
            cache.store(&report, stat, &digest, &config);
        }
        Ok(report)
    };
    // 全結果を入力順に集め、並列処理の完了順ではなく最初の入力のエラーを返す。
    // 本家は引数順を保持する。Path で並べ直すと foo/bar.rb が foo.rb より前になる。
    let inspected: Vec<Result<FileReport>> = if parallel && paths.len() > 1 {
        paths.par_iter().map(inspect).collect()
    } else {
        paths.iter().map(inspect).collect()
    };
    Ok(inspected)
}

/// What reading a file produced.
///
/// RuboCop reports a file it cannot decode as a fatal `Lint/Syntax` offense and inspects the rest,
/// so this must not abort the run; a genuine IO failure still does. The message travels with the
/// failure because RuboCop writes a different one for each way the decoding can fail.
enum Decoded {
    Text(String),
    Undecodable(String),
}

/// `Encoding::InvalidByteSequenceError` as RuboCop capitalizes it for a source that is not UTF-8
/// and does not say what it is.
const INVALID_UTF8: &str = "Invalid byte sequence in utf-8.";

/// Decodes a file the way Ruby does: through the encoding its own magic comment names, and only
/// through UTF-8 when it names none.
///
/// Reaching for UTF-8 first and falling back to the declaration would be more forgiving, but Ruby
/// is not forgiving here, and the difference shows twice. A file declaring a single-byte encoding
/// while holding UTF-8 is read as one character per byte upstream, so every column after the first
/// multibyte character moves. A file naming an encoding that does not exist is a syntax error
/// upstream however well its bytes read as UTF-8.
///
/// Four ways to fail are recorded upstream; three are reproduced here. The fourth -- a multibyte
/// encoding whose lead byte is fine and whose trailing byte is not, spelled
/// `"\x81" followed by " " on windows-31j.` -- needs Ruby's canonical encoding names, which are
/// not the ones `encoding_rs` answers to. It keeps the message it had. See
/// `tests/conformance/known_divergences.yml`.
fn decoded_source(path: &Path) -> Result<Decoded> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(decoded_bytes(bytes))
}

/// [`decoded_source`] once the bytes are in hand, which is also how `--stdin` gets them: upstream
/// hands `ProcessedSource.new` the raw buffer either way and only `ProcessedSource.from_file` adds
/// the `File.read` (`runner.rb:623-633`), so the two must decode identically.
fn decoded_bytes(bytes: Vec<u8>) -> Decoded {
    let Some(label) = declared_label(&bytes) else {
        return utf8_or_invalid(bytes);
    };
    // A file declaring itself binary has to be read that way even when its bytes happen to be valid
    // UTF-8: Ruby measures an `ASCII-8BIT` source one byte at a time, so a cop reporting a length or
    // a column over a multibyte sequence counts each byte separately.
    if is_binary_label(&label) {
        return Decoded::Text(bytes.iter().map(|byte| *byte as char).collect());
    }
    // `encoding_rs` answers to the WHATWG registry, which folds `us-ascii` into windows-1252 and so
    // reads every byte happily. Ruby's `US-ASCII` refuses anything above 7 bits, and that refusal is
    // the whole point of the declaration, so it is checked here rather than delegated.
    if is_seven_bit_label(&label) {
        return match bytes.iter().position(|byte| *byte > 0x7f) {
            Some(index) => {
                Decoded::Undecodable(format!("\"\\x{:02x}\" on us-ascii.", bytes[index]))
            }
            None => Decoded::Text(String::from_utf8(bytes).expect("7-bit bytes are UTF-8")),
        };
    }
    let Some(encoding) = encoding_for_ruby_label(&label) else {
        // The label reaches here already cut at the first `.`, since the magic comment's token
        // pattern holds no dot -- upstream's does not either, so `ANSI_X3.4-1968` is `ansi_x3` to
        // both of us and neither can name it. Cutting it is upstream's behaviour, not a defect to
        // repair: repairing it would resolve an encoding RuboCop reports as unknown.
        return Decoded::Undecodable(format!(
            "Unknown encoding name - {}.",
            label.to_ascii_lowercase()
        ));
    };
    if encoding == encoding_rs::UTF_8 {
        return utf8_or_invalid(bytes);
    }
    let (text, _, malformed) = encoding.decode(&bytes);
    match malformed {
        true => Decoded::Undecodable(INVALID_UTF8.to_owned()),
        false => Decoded::Text(text.into_owned()),
    }
}

/// A source with nothing to say about its own encoding, which Ruby reads as UTF-8.
fn utf8_or_invalid(bytes: Vec<u8>) -> Decoded {
    match String::from_utf8(bytes) {
        Ok(text) => Decoded::Text(text),
        Err(_) => Decoded::Undecodable(INVALID_UTF8.to_owned()),
    }
}

/// The names Ruby spells `US-ASCII`, where a byte above 7 bits is not a character at all.
fn is_seven_bit_label(label: &str) -> bool {
    matches!(
        label.to_ascii_lowercase().as_str(),
        "us-ascii" | "ascii" | "ansi_x3.4-1968" | "646"
    )
}

/// The encoding a source names for itself, read loosely: the magic comment is ASCII in every
/// encoding this can resolve, so the opening lines can be scanned before anything is decoded.
fn declared_label(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(b"\xef\xbb\xbf") {
        // `Parser::Source::Buffer.recognize_encoding` settles a byte order mark as UTF-8 and never
        // looks at the comment, so a declaration under one is only a comment.
        return None;
    }
    encoding_declaration(&String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]))
}

/// The encoding a source declares, read the way `Parser::Source::Buffer.recognize_encoding` reads
/// it: from the first line, or the second when the first is a shebang, and from nowhere else. A
/// `# coding:` comment further down -- Rails has one under a `frozen_string_literal` line -- is
/// just a comment, and reading it would decode the file as something Ruby never would.
fn encoding_declaration(head: &str) -> Option<String> {
    let mut lines = head.lines();
    let first = lines.next()?;
    let line = match first.starts_with("#!") {
        true => lines.next()?,
        false => first,
    };
    // Upstream tests the first byte rather than the first non-blank one, so an indented comment
    // declares nothing.
    if !line.starts_with('#') {
        return None;
    }
    MagicComment::parse(line).encoding()
}

/// How a source's declared encoding reads the bytes of a string literal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LiteralEncoding {
    /// Bytes above ASCII join into the character their UTF-8 spells.
    Text,
    /// Only ASCII is text. A literal written with nothing but ASCII keeps that encoding, so an
    /// escaped byte stays a byte; one holding a character of its own is retagged as text.
    SevenBit,
    /// Every byte is its own character, however its neighbours read.
    Binary,
}

/// The encoding a source declares, as it bears on writing a string literal's value back out.
///
/// A cop that does that needs this: `String#inspect` spells a byte its string's encoding cannot
/// read as `\xNN`, and only where the string is text do `"\xE5\xBE\x8C"`'s three bytes join into
/// the one character they spell.
pub(crate) fn declared_literal_encoding(text: &str) -> LiteralEncoding {
    // `encoding_declaration` reads the opening line or two and stops, so the whole text costs no
    // more than a prefix would -- and slicing one would have to find a character boundary first.
    match encoding_declaration(text) {
        Some(label) if is_binary_label(&label) => LiteralEncoding::Binary,
        Some(label) if is_seven_bit_label(&label) => LiteralEncoding::SevenBit,
        _ => LiteralEncoding::Text,
    }
}

/// 物理 Ruby の識別子の Symbol を比較するため、Source::Buffer の UTF-8 化前の符号化を保つ。
/// 対応しない符号化を UTF-8 と推測して補正の安全性を保証しない。
pub(crate) fn regexp_source_encoding(text: &str) -> Option<sonicop_onigmo_compat::Encoding> {
    use sonicop_onigmo_compat::Encoding;
    let Some(label) = declared_label(text.as_bytes()) else {
        return Some(Encoding::Utf8);
    };
    if is_binary_label(&label) {
        return Some(Encoding::Binary);
    }
    if is_seven_bit_label(&label) {
        return Some(Encoding::UsAscii);
    }
    // Ruby 4.0.8 の Encoding.aliases で同一 index と確認した名前だけをまとめる。
    // Shift_JIS や EUCJP-MS を WHATWG codec の近さだけで同一 Symbol にしない。
    match label.to_ascii_lowercase().as_str() {
        "utf-8" | "cp65001" => Some(Encoding::Utf8),
        "euc-jp" | "eucjp" => Some(Encoding::EucJp),
        "windows-31j" | "cswindows31j" | "sjis" | "pck" | "cp932" => Some(Encoding::Windows31J),
        _ => None,
    }
}

/// Ruby's names for "no encoding at all", where one byte is one character.
fn is_binary_label(label: &str) -> bool {
    matches!(
        label.to_ascii_lowercase().as_str(),
        "binary" | "ascii-8bit" | "ascii8bit"
    )
}

/// Ruby の別名と WHATWG のラベルは一致しないため、Ruby の追加名は明示的に対応させる。
fn encoding_for_ruby_label(label: &str) -> Option<&'static encoding_rs::Encoding> {
    if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
        // 本家は x-euc-jp 等を拒否する。EUC-JP の別名は下の Ruby 用対応表だけで補う。
        return (encoding != encoding_rs::EUC_JP || label.eq_ignore_ascii_case("euc-jp"))
            .then_some(encoding);
    }
    let alias = match label.to_ascii_lowercase().as_str() {
        "eucjp" => "euc-jp",
        "cp932" => "windows-31j",
        "cp51932" | "eucjp-ms" | "euc-jp-ms" => "euc-jp",
        "cp936" => "gbk",
        "cp949" => "euc-kr",
        "cp950" => "big5",
        _ => return None,
    };
    encoding_rs::Encoding::for_label(alias.as_bytes())
}

fn undecodable_report(path: &Path, message: &str) -> FileReport {
    // RuboCop anchors the offense at the head of the file however far in the bad byte sits, since
    // it never got a syntax tree to locate anything against.
    let mut offense = Offense::new("Lint/Syntax", Severity::Fatal, message, 0, 0);
    let source = SourceFile::new(path.to_path_buf(), String::new());
    offense.freeze_location(&source);
    FileReport {
        path: path.to_path_buf(),
        source,
        offenses: vec![offense],
    }
}

pub fn discover_targets(
    arguments: &[PathBuf],
    cwd: &Path,
    config: &Config,
    force_exclusion: bool,
    only_recognized_file_types: bool,
) -> Result<Vec<PathBuf>> {
    let configs = ConfigStore::new(config.clone(), false, false);
    discover_targets_with_store(
        arguments,
        cwd,
        &configs,
        force_exclusion,
        only_recognized_file_types,
    )
}

/// What the `Include`/`Exclude` lists alone can say about a walked path. Only `AskShebang` costs a
/// read, and it is reached solely by an extension-less file no pattern claims.
enum Verdict {
    Keep,
    Drop,
    AskShebang,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DirectoryOrder {
    Lexical,
    FailFast,
}

pub fn discover_targets_with_store(
    arguments: &[PathBuf],
    cwd: &Path,
    configs: &ConfigStore,
    force_exclusion: bool,
    only_recognized_file_types: bool,
) -> Result<Vec<PathBuf>> {
    discover_cli_targets_with_store(
        arguments,
        cwd,
        configs,
        force_exclusion,
        only_recognized_file_types,
        DirectoryOrder::Lexical,
    )
}

pub(crate) fn discover_cli_targets_with_store(
    arguments: &[PathBuf],
    cwd: &Path,
    configs: &ConfigStore,
    force_exclusion: bool,
    only_recognized_file_types: bool,
    order: DirectoryOrder,
) -> Result<Vec<PathBuf>> {
    discover_target_paths(
        arguments,
        cwd,
        configs,
        force_exclusion,
        only_recognized_file_types,
        false,
        order,
    )
}

/// TargetFinder#ruby_file? は stdin 時に拡張子や shebang を調べない。
pub(crate) fn discover_stdin_targets_with_store(
    directory: &Path,
    cwd: &Path,
    configs: &ConfigStore,
    order: DirectoryOrder,
) -> Result<Vec<PathBuf>> {
    discover_target_paths(
        &[directory.to_path_buf()],
        cwd,
        configs,
        false,
        false,
        true,
        order,
    )
}

fn discover_target_paths(
    arguments: &[PathBuf],
    cwd: &Path,
    configs: &ConfigStore,
    force_exclusion: bool,
    only_recognized_file_types: bool,
    stdin: bool,
    order: DirectoryOrder,
) -> Result<Vec<PathBuf>> {
    let roots = if arguments.is_empty() {
        vec![cwd.to_path_buf()]
    } else {
        arguments.to_vec()
    };
    let mut targets = Vec::new();

    for root in roots {
        if !root.exists() {
            bail!("No such file or directory: {}", root.display());
        }
        if root.is_file() {
            // TargetFinderは通常fileをfor_pwdで選び、ignore_parent時だけ個別設定を訪問する。
            let per_file = if force_exclusion && configs.ignores_parent_exclusion() {
                Some(configs.for_path_observed(&root)?)
            } else {
                None
            };
            let config = per_file.as_deref().unwrap_or(configs.root());
            let recognized = config.path_included(&root) || has_ruby_shebang(&root);
            if (!force_exclusion || !config.path_excluded(&root))
                && (!only_recognized_file_types || recognized)
            {
                targets.push(root);
            }
            continue;
        }

        // 本家は対象を `AllCops/Include` と `AllCops/Exclude` だけで決め、`.gitignore` は読まない。
        // これを読むと、`bin/*` が無視されても追跡中の `bin/console` まで落としてしまう。
        // ignore クレートには Git の索引が見えないため、その区別はできない。
        // 本家の glob は `File::FNM_DOTMATCH` を使う。可視ディレクトリ内の隠しファイルは
        // 対象で、最初のパス成分が隠しの場合だけ `Config::path_included` で除く。
        // 探索前の除外ディレクトリ枝刈りは、本家の `wanted_dir_patterns` に合わせる。
        let mut walked = Vec::new();
        // 本家の TargetFinder は探索を始めるディレクトリの設定を全候補に適用する。
        // 配下の別設定は、そこを探索起点に指定したときだけ対象選択へ反映する。
        let discovery_config = crate::profile::phase(crate::profile::Phase::ConfigLookup, || {
            configs.for_path_observed(&root)
        })?;
        let pruned = discovery_config.clone();
        let hidden_stdin_root = stdin
            && root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        let mut builder = WalkBuilder::new(&root);
        builder
            .filter_entry(move |entry| {
                !entry.file_type().is_some_and(|kind| kind.is_dir())
                    || (stdin && entry.depth() == 0)
                    || crate::profile::phase(crate::profile::Phase::DirFilter, || {
                        !pruned.directory_excluded(entry.path())
                    })
            })
            .hidden(false)
            .parents(false)
            .git_ignore(false)
            .git_exclude(false)
            .git_global(false)
            .require_git(false)
            .ignore(false)
            // RuboCop descends through a directory symlink and turns back only when following it
            // would revisit an ancestor, so a vendored gem reachable under both its real name and a
            // versionless link is inspected under both paths.
            .follow_links(true);
        let mut walker = builder.build();
        while let Some(entry) = crate::profile::phase(crate::profile::Phase::Walk, || walker.next())
        {
            // RuboCop globs the tree and keeps whatever `FileTest.file?` accepts, so an entry it
            // cannot resolve -- a symlink that closes a cycle, or one whose target is gone -- drops
            // out silently instead of failing the run.
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let verdict = crate::profile::phase(crate::profile::Phase::PathMatch, || {
                // 明示した隠し起点だけは TargetFinder#hidden_dir? の分岐へ進む。
                if hidden_stdin_root {
                    return Verdict::Keep;
                }
                let included = discovery_config.path_included(path);
                if discovery_config.path_excluded(path) {
                    return Verdict::Drop;
                }
                if included {
                    return Verdict::Keep;
                }
                if stdin {
                    let hidden =
                        path.strip_prefix(&root)
                            .unwrap_or(path)
                            .components()
                            .any(|part| {
                                let name = part.as_os_str().to_string_lossy();
                                name.starts_with('.') && name != "." && name != ".."
                            });
                    return if hidden { Verdict::Drop } else { Verdict::Keep };
                }
                if discovery_config.path_hidden(path) {
                    return Verdict::Drop;
                }
                Verdict::AskShebang
            });
            let keep = match verdict {
                Verdict::Keep => true,
                Verdict::Drop => false,
                Verdict::AskShebang => {
                    crate::profile::phase(crate::profile::Phase::Shebang, || has_ruby_shebang(path))
                }
            };
            if !keep {
                continue;
            }
            walked.push(normalized_target_path(path));
        }
        // TargetFinder#order は各 directory にだけ適用し、明示引数の group 順は保つ。
        order_directory_targets(&mut walked, order)?;
        targets.append(&mut walked);
    }

    let mut seen = HashSet::new();
    targets.retain(|target| seen.insert(target.clone()));
    Ok(targets)
}

fn order_directory_targets(paths: &mut Vec<PathBuf>, order: DirectoryOrder) -> Result<()> {
    if order == DirectoryOrder::Lexical {
        // Windows の区切りのままでは foo1/bar.rb が foo/bar.rb より先になる。
        #[cfg(windows)]
        paths.sort_by_cached_key(|path| path.to_string_lossy().replace('\\', "/"));
        #[cfg(not(windows))]
        paths.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
        return Ok(());
    }
    // wanted_dir_patterns/Dir.glob は root の files、sibling 名順DFSの各 files の順。
    // directory の完成文字列では a-/ が a/ より先になるため、component列を比較する。
    paths.sort_by_cached_key(|path| {
        let parent = path.parent().unwrap_or(Path::new(""));
        let directories = parent
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        (
            directories,
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    });
    let mut entries = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let metadata = fs::metadata(path)
                .with_context(|| format!("failed to read modification time: {}", path.display()))?;
            Ok(MtimeEntry {
                seconds: ruby_mtime_seconds(&metadata)?,
                index,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    sort_entries(&mut entries, c_long::BITS);
    let mut original: Vec<_> = std::mem::take(paths).into_iter().map(Some).collect();
    paths.extend(entries.into_iter().map(|entry| {
        original[entry.index]
            .take()
            .expect("qsort changed the index permutation")
    }));
    Ok(())
}

fn ruby_mtime_seconds(metadata: &fs::Metadata) -> Result<i64> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Ruby 4.0.7 filetime_split と同じ unsigned subtraction を discovery key だけへ使う。
        // 生FILETIME/負のEpoch/実File.mtimeの照合で、cacheの時刻表現とは異なることを確認した。
        Ok((metadata
            .last_write_time()
            .wrapping_sub(116_444_736_000_000_000)
            / 10_000_000) as i64)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(metadata.mtime())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let seconds = match metadata.modified()?.duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => i128::from(duration.as_secs()),
            Err(error) => {
                -i128::from(error.duration().as_secs())
                    - i128::from(error.duration().subsec_nanos() != 0)
            }
        };
        i64::try_from(seconds).context("modification time is outside signed 64-bit seconds")
    }
}

// Ruby enum.c revision 229531a6cfbf07e3caef30dbac24a2a3f3fed482 の uniform sort を移植。
// Copyright (C) 1993-2007 Yukihiro Matsumoto.
// 2-clause BSDLを選択。手順・revisionはNOTICE、全文はlicenses/RUBY_BSDL.txtを参照。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct MtimeEntry {
    seconds: i64,
    index: usize,
}

unsafe extern "C" {
    #[link_name = "qsort"]
    fn crt_qsort(
        base: *mut c_void,
        count: usize,
        width: usize,
        compare: unsafe extern "C" fn(*const c_void, *const c_void) -> c_int,
    );
}

unsafe extern "C" fn compare_mtime(left: *const c_void, right: *const c_void) -> c_int {
    // SAFETY: 呼出し元の qsort は同じ生存中の MtimeEntry 配列要素を渡す。
    // 所有物を持たない Copy record の整列済み pointer を、この呼出し内だけ読む。
    let left = unsafe { &*left.cast::<MtimeEntry>() };
    let right = unsafe { &*right.cast::<MtimeEntry>() };
    match right.seconds.cmp(&left.seconds) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn mtime_key_is_fixnum(seconds: i64, long_bits: u32) -> bool {
    // Ruby FIXNUM_MIN/MAX は pointer 幅ではなく C long の範囲の半分を使う。
    // i64::MIN の negation も先に i128 へ広げてから行う。
    let boundary = 1_i128 << (long_bits - 2);
    (-boundary..boundary).contains(&-i128::from(seconds))
}

fn mtime_less(left: MtimeEntry, right: MtimeEntry) -> bool {
    // TargetFinder#order の -Integer(mtime) と同じ向きで、同値 index を比較しない。
    left.seconds > right.seconds
}

fn insertion_sort(entries: &mut [MtimeEntry]) {
    for index in 1..entries.len() {
        let value = entries[index];
        let mut position = index;
        while position > 0 && mtime_less(value, entries[position - 1]) {
            entries[position] = entries[position - 1];
            position -= 1;
        }
        entries[position] = value;
    }
}

fn heap_down(entries: &mut [MtimeEntry], mut offset: usize, length: usize) {
    let value = entries[offset];
    loop {
        let mut child = offset * 2 + 1;
        if child > length {
            break;
        }
        if child < length && mtime_less(entries[child], entries[child + 1]) {
            child += 1;
        }
        if !mtime_less(value, entries[child]) {
            break;
        }
        entries[offset] = entries[child];
        offset = child;
    }
    entries[offset] = value;
}

fn heap_sort(entries: &mut [MtimeEntry]) {
    for offset in (0..entries.len() / 2).rev() {
        heap_down(entries, offset, entries.len() - 1);
    }
    for offset in (1..entries.len()).rev() {
        entries.swap(0, offset);
        heap_down(entries, 0, offset - 1);
    }
}

fn uniform_quicksort(entries: &mut [MtimeEntry], depth: u32) {
    if entries.len() <= 16 {
        insertion_sort(entries);
        return;
    }
    if depth == 0 {
        heap_sort(entries);
        return;
    }
    let (a, b, c) = (
        entries[0],
        entries[entries.len() / 2],
        entries[entries.len() - 1],
    );
    let pivot = if mtime_less(a, b) {
        if mtime_less(b, c) {
            b
        } else if mtime_less(c, a) {
            a
        } else {
            c
        }
    } else if mtime_less(c, b) {
        b
    } else if mtime_less(a, c) {
        a
    } else {
        c
    };
    let mut left = 0;
    let mut right = entries.len() as isize - 1;
    loop {
        while mtime_less(entries[left], pivot) {
            left += 1;
        }
        while mtime_less(pivot, entries[right as usize]) {
            right -= 1;
        }
        if left as isize <= right {
            entries.swap(left, right as usize);
            left += 1;
            right -= 1;
        }
        if left as isize > right {
            break;
        }
    }
    let right = (right + 1) as usize;
    // Ruby は右側を先に処理する。depth は2*log2(n)で有界なのでAST深さに依存しない。
    if entries.len() - right > 1 {
        uniform_quicksort(&mut entries[right..], depth - 1);
    }
    if left > 1 {
        uniform_quicksort(&mut entries[..left], depth - 1);
    }
}

fn sort_entries(entries: &mut [MtimeEntry], long_bits: u32) {
    if entries.len() < 2 {
        return;
    }
    if entries
        .iter()
        .all(|entry| mtime_key_is_fixnum(entry.seconds, long_bits))
    {
        if entries.windows(2).any(|pair| mtime_less(pair[1], pair[0])) {
            uniform_quicksort(entries, entries.len().ilog2() * 2);
        }
    } else {
        // SAFETY: qsort に渡すのは初期化済みで有効な配列の生存中の範囲だけ。
        // MtimeEntry は drop 対象を持たず Copy でき、byte 単位の交換で所有権は壊れない。
        // comparator はこの配列の要素を読み、整数比較だけを行い、panic/保持/I/Oはない。
        // qsort は同期的に完了し、pointer を保持しない。PathBuf 自体は C へ渡さない。
        unsafe {
            crt_qsort(
                entries.as_mut_ptr().cast(),
                entries.len(),
                std::mem::size_of::<MtimeEntry>(),
                compare_mtime,
            );
        }
    }
}

fn normalized_target_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    path.strip_prefix(".").unwrap_or(path).to_path_buf()
}

fn has_ruby_shebang(path: &Path) -> bool {
    // RuboCop's `ruby_executable?` bails out before reading the file unless the name carries no
    // extension at all, so a shebang only rescues files like `bin/console`. Templates such as
    // `newgem.tt` or `Executable.standalone` start with `#!/usr/bin/env ruby` and would otherwise be
    // linted here while upstream leaves them alone.
    if path.extension().is_some() {
        return false;
    }
    let Ok(contents) = fs::read(path) else {
        return false;
    };
    let first_line = contents
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    first_line.starts_with(b"#!")
        && [b"ruby".as_slice(), b"rake".as_slice(), b"jruby".as_slice()]
            .iter()
            .any(|interpreter| {
                first_line
                    .windows(interpreter.len())
                    .any(|part| part == *interpreter)
            })
}

/// Stands in for the `Parser::ClobberingError` upstream raises when an edit cannot be placed.
#[derive(Clone, Copy, Debug)]
struct Clobbering;

type Combined = Result<Action, Clobbering>;

/// One scheduled edit, in the shape `Parser::Source::TreeRewriter::Action` gives it.
///
/// Upstream arranges every edit of a rewrite into a tree: a child is contained by its parent,
/// siblings are disjoint and ordered, and only an action with no replacement may have children.
/// Combining two edits is therefore not "the first one wins" but a placement in that tree, and
/// what a collision costs depends on the shape of the overlap:
///
/// * two edits on exactly the same range *merge* -- their insertions concatenate, with the later
///   edit wrapping around the earlier, and a replacement fills in for an absent one;
/// * an edit that strictly contains others *swallows* them, which RuboCop's `Corrector` turns into
///   a clobbering error the moment one of the swallowed edits inserts text;
/// * edits that merely cross *fuse* when both are deletions, and clobber otherwise.
///
/// That asymmetry is the whole point: two cops inserting at one offset both land, while two cops
/// replacing the same text cannot, so reproducing RuboCop's output byte for byte means reproducing
/// the tree rather than any single "keep one, drop the other" rule.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Action {
    begin_pos: usize,
    end_pos: usize,
    insert_before: String,
    replacement: Option<String>,
    characters_from: Option<CorrectionCharacterSource>,
    insert_after: String,
    children: Vec<Action>,
}

// 補正の適用範囲と、値の文字が由来する範囲は別々に持つ。
type Replacement<'a> = (usize, usize, &'a str, Option<CorrectionCharacterSource>);

/// LF解析viewに隠した元CRと、まだ見える元bare CRの由来を分ける。
/// CRが連続すると次passでさらに1つ隠れるため、同じLFの復元markerは重複できる。
#[derive(Clone)]
struct LineEndingProvenance {
    hidden_cr: Vec<usize>,
    visible_cr: Vec<usize>,
    physical: bool,
}

impl LineEndingProvenance {
    fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        Self {
            hidden_cr: text
                .match_indices("\r\n")
                .map(|(offset, _)| offset + 1)
                .collect(),
            visible_cr: bytes
                .iter()
                .enumerate()
                .filter_map(|(offset, byte)| {
                    (*byte == b'\r' && bytes.get(offset + 1) != Some(&b'\n')).then_some(offset)
                })
                .collect(),
            physical: true,
        }
    }
}

impl Action {
    /// The root upstream builds by widening the source range at both ends so that it contains every
    /// action, including the empty ranges at either end of the source. An unsigned offset cannot go
    /// below zero, so the root only has to reach past every real edit for no action to ever compare
    /// equal to it -- the one comparison the algorithm makes against a parent's own range.
    fn root() -> Self {
        Self {
            begin_pos: 0,
            end_pos: usize::MAX,
            insert_before: String::new(),
            replacement: None,
            characters_from: None,
            insert_after: String::new(),
            children: Vec::new(),
        }
    }

    /// A cop's edit as the corrector call it stands for: a span carries a replacement, an empty
    /// span is an insertion.
    ///
    /// `anchor` is the range the offense was reported on. Upstream never inserts at a bare offset:
    /// a cop calls `insert_before` or `insert_after` with the range of the thing it is talking
    /// about, and the tree keeps that range, which is what orders two insertions that land on the
    /// same offset. `Edit` carries only the offset, so the offense's own range stands in for the
    /// range the cop would have passed -- the two agree for every cop that inserts at an end of
    /// what it reported, which is what a cop inserting anything at all does.
    fn from_edit(edit: &crate::diagnostic::Edit, anchor: (usize, usize)) -> Self {
        let mut action = Self {
            begin_pos: edit.start,
            end_pos: edit.end,
            insert_before: String::new(),
            replacement: None,
            characters_from: None,
            insert_after: String::new(),
            children: Vec::new(),
        };
        if edit.start != edit.end {
            action.replacement = Some(edit.replacement.clone());
            return action;
        }
        let (begin, end) = anchor;
        if begin < end && (edit.start == begin || edit.start == end) {
            action.begin_pos = begin;
            action.end_pos = end;
        }
        match edit.start == action.end_pos && action.begin_pos != action.end_pos {
            true => action.insert_after = edit.replacement.clone(),
            false => action.insert_before = edit.replacement.clone(),
        }
        action
    }

    /// cop が範囲と向きを指定した挿入。本家の `insert_before(range, text)` /
    /// `insert_after(range, text)` そのもので、空の範囲への `insert_after` も向きを失わない。
    ///
    /// 挿入でない編集と、文字の位置がアンカーの向きの端にない編集は `None` を返す。前者は
    /// 置換なのでアンカーが効かず、後者は cop の指定の誤りなので、どちらも
    /// [`Action::from_edit`] の推定に任せる。
    fn from_anchored_edit(edit: &Edit, anchor: &InsertionAnchor) -> Option<Self> {
        let (range, after) = match anchor {
            InsertionAnchor::Before(range) => (range, false),
            InsertionAnchor::After(range) => (range, true),
        };
        let at = if after { range.end } else { range.start };
        if edit.start != edit.end || edit.start != at || range.start > range.end {
            return None;
        }
        let mut action = Self {
            begin_pos: range.start,
            end_pos: range.end,
            insert_before: String::new(),
            replacement: None,
            characters_from: None,
            insert_after: String::new(),
            children: Vec::new(),
        };
        match after {
            true => action.insert_after = edit.replacement.clone(),
            false => action.insert_before = edit.replacement.clone(),
        }
        Some(action)
    }

    fn is_empty(&self) -> bool {
        self.insert_before.is_empty()
            && self.insert_after.is_empty()
            && self.children.is_empty()
            && match &self.replacement {
                None => true,
                Some(replacement) => replacement.is_empty() && self.begin_pos == self.end_pos,
            }
    }

    /// Whether the action puts text into the source. A deletion does not, which is what lets two
    /// deletions fuse and lets a replacement swallow one silently.
    fn inserts(&self) -> bool {
        !self.insert_before.is_empty()
            || !self.insert_after.is_empty()
            || self
                .replacement
                .as_ref()
                .is_some_and(|replacement| !replacement.is_empty())
    }

    fn combine(&self, action: &Action) -> Combined {
        match action.is_empty() {
            true => Ok(self.clone()),
            false => self.do_combine(action),
        }
    }

    fn do_combine(&self, action: &Action) -> Combined {
        match action.begin_pos == self.begin_pos && action.end_pos == self.end_pos {
            true => self.merge(action),
            false => self.place_in_hierarchy(action),
        }
    }

    /// 本家の `Action#with` は置換に含まれる子を捨てる。削除は黙って失われるが、
    /// 挿入を飲み込む場合は RuboCop の `swallowed_insertions` と同じ衝突にする。
    fn with(&self, children: Vec<Action>, replacement: Option<String>) -> Combined {
        let children = match replacement.is_some() {
            true if children.iter().any(Action::inserts) => return Err(Clobbering),
            true => Vec::new(),
            false => children,
        };
        Ok(Self {
            begin_pos: self.begin_pos,
            end_pos: self.end_pos,
            insert_before: self.insert_before.clone(),
            insert_after: self.insert_after.clone(),
            replacement,
            characters_from: self.characters_from,
            children,
        })
    }

    /// 同じ範囲の挿入は後の文字列が外側へ回る。異なる置換は本家の
    /// different_replacements と同じ衝突にする。値の出典も置換と一緒に保つ。
    fn merge(&self, action: &Action) -> Combined {
        if let (Some(ours), Some(theirs)) = (&self.replacement, &action.replacement)
            && ours != theirs
        {
            return Err(Clobbering);
        }
        let replacement = action
            .replacement
            .clone()
            .or_else(|| self.replacement.clone());
        let characters_from = match (self.characters_from, action.characters_from) {
            (Some(ours), Some(theirs)) => {
                let (a, b) = ours.range();
                let (c, d) = theirs.range();
                let start = a.max(c);
                let end = b.min(d).max(start);
                Some(
                    if matches!(ours, CorrectionCharacterSource::Copied(..))
                        && matches!(theirs, CorrectionCharacterSource::Copied(..))
                    {
                        CorrectionCharacterSource::Copied(start, end)
                    } else {
                        CorrectionCharacterSource::Candidates(start, end)
                    },
                )
            }
            (ours, theirs) => theirs.or(ours),
        };
        let merged = Self {
            characters_from,
            insert_before: format!("{}{}", action.insert_before, self.insert_before),
            insert_after: format!("{}{}", self.insert_after, action.insert_after),
            ..self.clone()
        }
        .with(self.children.clone(), replacement)?;
        merged.combine_children(&action.children)
    }

    /// 既存の子より後ろに離れた補正だけは、階層も衝突判定も変わらず末尾に足せる。
    /// 同じ位置の挿入は後から来た文字列が外側になるため、接する範囲も通常経路へ回す。
    /// 全件を先に調べ、失敗時には途中まで追加しない。
    fn append_disjoint_children(&mut self, children: &[Action]) -> bool {
        if self.replacement.is_some() || children.is_empty() {
            return false;
        }
        let mut previous_end = self.children.last().map(|child| child.end_pos);
        for child in children {
            if child.begin_pos > child.end_pos
                || child.begin_pos < self.begin_pos
                || child.end_pos > self.end_pos
                || previous_end.is_some_and(|end| end >= child.begin_pos)
            {
                return false;
            }
            previous_end = Some(child.end_pos);
        }
        self.children.extend_from_slice(children);
        true
    }

    fn combine_children(mut self, children: &[Action]) -> Combined {
        if self.merge_disjoint_children(children) {
            return Ok(self);
        }
        for child in children {
            if !self.append_disjoint_children(std::slice::from_ref(child)) {
                self = self.place_in_hierarchy(child)?;
            }
        }
        Ok(self)
    }

    /// 交互に並ぶ別 cop の補正も、離れた兄弟だけなら階層を組み直す必要がない。
    /// 入力同士と既存の子との境界を先に確認し、衝突時は親を変更せず通常経路へ返す。
    fn merge_disjoint_children(&mut self, children: &[Action]) -> bool {
        if self.replacement.is_some() || children.is_empty() {
            return false;
        }
        let mut previous_end = None;
        let mut existing = 0;
        for child in children {
            if child.is_empty()
                || child.begin_pos > child.end_pos
                || child.begin_pos < self.begin_pos
                || child.end_pos > self.end_pos
                || (child.begin_pos == self.begin_pos && child.end_pos == self.end_pos)
                || previous_end.is_some_and(|end| end >= child.begin_pos)
            {
                return false;
            }
            while self
                .children
                .get(existing)
                .is_some_and(|old| old.end_pos < child.begin_pos)
            {
                existing += 1;
            }
            if self
                .children
                .get(existing)
                .is_some_and(|old| old.begin_pos <= child.end_pos)
            {
                return false;
            }
            previous_end = Some(child.end_pos);
        }

        let count = self.children.len() + children.len();
        let mut old = std::mem::take(&mut self.children).into_iter().peekable();
        let mut merged = Vec::with_capacity(count);
        for child in children {
            while old.peek().is_some_and(|old| old.end_pos < child.begin_pos) {
                merged.push(old.next().expect("直前に存在を確認した子"));
            }
            merged.push(child.clone());
        }
        merged.extend(old);
        self.children = merged;
        true
    }

    fn place_in_hierarchy(&self, action: &Action) -> Combined {
        let family = self.analyse_hierarchy(action)?;
        let Family {
            left_index,
            right_index,
            ..
        } = family;
        if let Some(fusible) = family.fusible {
            // Both sides are deletions, which RuboCop accepts: they collapse into the one deletion
            // that spans them all, and the fused children leave the tree.
            let mut kept = self.children[..left_index].to_vec();
            kept.extend(family.contained.unwrap_or_default());
            kept.extend_from_slice(&self.children[right_index..]);
            let without_fusible = self.with(kept, self.replacement.clone())?;
            let fused = Self {
                begin_pos: fusible
                    .iter()
                    .fold(action.begin_pos, |begin, child| begin.min(child.begin_pos)),
                end_pos: fusible
                    .iter()
                    .fold(action.end_pos, |end, child| end.max(child.end_pos)),
                ..action.clone()
            }
            .with(action.children.clone(), action.replacement.clone())?;
            return without_fusible.do_combine(&fused);
        }
        let placed = if let Some(parent) = family.parent {
            self.children[parent].do_combine(action)?
        } else if let Some(contained) = family.contained {
            action
                .with(contained, action.replacement.clone())?
                .combine_children(&action.children)?
        } else {
            action.clone()
        };
        let mut children = self.children[..left_index].to_vec();
        children.push(placed);
        children.extend_from_slice(&self.children[right_index..]);
        self.with(children, self.replacement.clone())
    }

    /// `Action#analyse_hierarchy`: where `action` sits among the children of this node.
    fn analyse_hierarchy(&self, action: &Action) -> Result<Family, Clobbering> {
        // The first child that is not wholly to the left of the action, and the first that is
        // wholly to its right. Everything between the two touches the action somehow.
        let mut left_index = self.find_child(0, |child| child.end_pos > action.begin_pos);
        let mut right_index = self.find_child(left_index.saturating_sub(1), |child| {
            child.begin_pos >= action.end_pos
        });

        // An empty range is disjoint from every range it merely touches, which leaves a child whose
        // range is empty and equal to the action's looking both left of it and right of it. The
        // ranges are equal, so that child is the action's parent.
        if right_index + 1 == left_index {
            left_index -= 1;
            right_index += 1;
            return Ok(Family {
                parent: Some(left_index),
                left_index,
                right_index,
                fusible: None,
                contained: None,
            });
        }
        if right_index == left_index {
            return Ok(Family {
                parent: None,
                left_index,
                right_index,
                fusible: None,
                contained: None,
            });
        }

        let overlap_left = self.children[left_index].begin_pos.cmp(&action.begin_pos);
        let overlap_right = self.children[right_index - 1].end_pos.cmp(&action.end_pos);
        if right_index - left_index == 1
            && overlap_left != Ordering::Greater
            && overlap_right != Ordering::Less
        {
            return Ok(Family {
                parent: Some(left_index),
                left_index,
                right_index,
                fusible: None,
                contained: None,
            });
        }

        // Everything the action reaches is contained by it, bar a first and a last child it only
        // partly covers. Those two cross the action rather than nest inside it.
        let mut contained = self.children[left_index..right_index].to_vec();
        let mut fusible = Vec::new();
        if overlap_left == Ordering::Less {
            fusible.push(contained.remove(0));
        }
        if overlap_right == Ordering::Greater
            && let Some(last) = contained.pop()
        {
            fusible.push(last);
        }
        // `crossing_deletions` is the one clobbering RuboCop accepts; a crossing that puts text
        // anywhere -- upstream's `crossing_insertions` -- has no policy and always raises.
        if fusible
            .iter()
            .any(|child| action.inserts() || child.inserts())
        {
            return Err(Clobbering);
        }
        Ok(Family {
            parent: None,
            left_index,
            right_index,
            fusible: (!fusible.is_empty()).then_some(fusible),
            contained: Some(contained),
        })
    }

    /// The first child from `from` on that the predicate accepts, or the number of children when
    /// none does. Children are ordered and disjoint, so the predicates upstream passes are
    /// monotonic and this answers what its `bsearch_child_index` answers.
    fn find_child(&self, from: usize, accepts: impl Fn(&Action) -> bool) -> usize {
        (from..self.children.len())
            .find(|index| accepts(&self.children[*index]))
            .unwrap_or(self.children.len())
    }

    fn ordered_replacements<'a>(&'a self, replacements: &mut Vec<Replacement<'a>>) {
        if !self.insert_before.is_empty() {
            replacements.push((self.begin_pos, self.begin_pos, &self.insert_before, None));
        }
        if let Some(replacement) = &self.replacement {
            replacements.push((
                self.begin_pos,
                self.end_pos,
                replacement,
                self.characters_from,
            ));
        }
        for child in &self.children {
            child.ordered_replacements(replacements);
        }
        if !self.insert_after.is_empty() {
            replacements.push((self.end_pos, self.end_pos, &self.insert_after, None));
        }
    }

    fn rewrite(&self, source: &str, introduced: Option<&[usize]>) -> (String, Option<Vec<usize>>) {
        self.rewrite_with_line_endings(source, introduced, None)
    }

    fn rewrite_with_line_endings(
        &self,
        source: &str,
        introduced: Option<&[usize]>,
        retained_crlf: Option<&mut LineEndingProvenance>,
    ) -> (String, Option<Vec<usize>>) {
        let mut replacements = Vec::new();
        self.ordered_replacements(&mut replacements);
        let mut text = String::with_capacity(source.len());
        let mut provenance = introduced.map(|_| Vec::new());
        let mut next_crlf = retained_crlf.as_ref().map(|before| LineEndingProvenance {
            hidden_cr: Vec::new(),
            visible_cr: Vec::new(),
            physical: before.physical,
        });
        let mut last_end = 0;
        for (begin, end, replacement, characters_from) in replacements {
            debug_assert!(
                begin >= last_end,
                "the action tree yielded overlapping edits"
            );
            if let (Some(before), Some(after)) = (introduced, &mut provenance) {
                copy_provenance(before, last_end, begin, text.len(), after);
            }
            if let (Some(before), Some(after)) = (retained_crlf.as_deref(), &mut next_crlf) {
                copy_line_endings(before, source, last_end, begin, text.len(), after);
            }
            text.push_str(&source[last_end..begin.max(last_end)]);
            if let (Some(before), Some(after)) = (introduced, &mut provenance) {
                // 削除された別範囲の文字で、追加された文字の由来を相殺させない。
                let (origin_begin, origin_end) =
                    characters_from.map_or((begin, end), CorrectionCharacterSource::range);
                let original = &source[origin_begin..origin_end];
                let copied = matches!(characters_from, Some(CorrectionCharacterSource::Copied(..)))
                    && original
                        .chars()
                        .filter(|character| !character.is_ascii())
                        .eq(replacement
                            .chars()
                            .filter(|character| !character.is_ascii()));
                if copied {
                    // 同じ文字が複数あっても、範囲全体の複写は位置ごとの由来を保つ。
                    let mut originals = original
                        .char_indices()
                        .filter(|(_, character)| !character.is_ascii());
                    for (offset, character) in replacement.char_indices() {
                        if !character.is_ascii() {
                            let (old_offset, _) = originals.next().unwrap();
                            if before.binary_search(&(origin_begin + old_offset)).is_ok() {
                                after.push(text.len() + offset);
                            }
                        }
                    }
                } else {
                    let mut existing: HashMap<char, Vec<usize>> = HashMap::new();
                    let mut generated = HashSet::new();
                    for (offset, character) in source[origin_begin..origin_end].char_indices().rev()
                    {
                        if !character.is_ascii() {
                            existing
                                .entry(character)
                                .or_default()
                                .push(origin_begin + offset);
                            if before.binary_search(&(origin_begin + offset)).is_ok() {
                                generated.insert(character);
                            }
                        }
                    }
                    for (offset, character) in replacement.char_indices() {
                        if character.is_ascii() {
                            continue;
                        }
                        let matched = existing.get_mut(&character).and_then(Vec::pop);
                        // 同じ文字が原本と追加分にあれば、由来が曖昧なコピーで追加分を消さない。
                        if matched.is_none() || generated.contains(&character) {
                            after.push(text.len() + offset);
                        }
                    }
                }
            }
            text.push_str(replacement);
            last_end = end.max(last_end);
        }
        if let (Some(before), Some(after)) = (introduced, &mut provenance) {
            copy_provenance(before, last_end, source.len(), text.len(), after);
        }
        if let (Some(before), Some(after)) = (retained_crlf.as_deref(), &mut next_crlf) {
            copy_line_endings(before, source, last_end, source.len(), text.len(), after);
        }
        text.push_str(&source[last_end..]);
        if let (Some(before), Some(after)) = (retained_crlf, next_crlf) {
            *before = after;
        }
        (text, provenance)
    }
}

/// 未変更のcopy区間だけが元の改行形式を引き継ぐ。置換の同じCR/LFから由来を推測しない。
fn copy_line_endings(
    before: &LineEndingProvenance,
    source: &str,
    begin: usize,
    end: usize,
    output_begin: usize,
    after: &mut LineEndingProvenance,
) {
    let first = before
        .hidden_cr
        .partition_point(|position| *position < begin);
    for &position in before.hidden_cr[first..]
        .iter()
        .take_while(|position| **position < end)
    {
        // 初回physicalのpairだけ両byteを要求する。LF-viewの前のCRは別の元bare CRである。
        if source.as_bytes().get(position) == Some(&b'\n')
            && (!before.physical
                || position == 0
                || source.as_bytes()[position - 1] != b'\r'
                || position > begin)
        {
            after.hidden_cr.push(output_begin + position - begin);
        }
    }
    copy_provenance(
        &before.visible_cr,
        begin,
        end,
        output_begin,
        &mut after.visible_cr,
    );
}

/// 変更していない範囲に残る文字の由来を、補正後のバイト位置へ移す。
fn copy_provenance(
    before: &[usize],
    begin: usize,
    end: usize,
    output_begin: usize,
    after: &mut Vec<usize>,
) {
    let first = before.partition_point(|offset| *offset < begin);
    after.extend(
        before[first..]
            .iter()
            .take_while(|offset| **offset < end)
            .map(|offset| output_begin + offset - begin),
    );
}

/// Where one action sits among the children of a node: which of them it nests inside, which of them
/// nest inside it, which of them it crosses, and which it leaves alone on either side.
struct Family {
    /// Index of the child that contains the action, if one does.
    parent: Option<usize>,
    /// Children before `left_index` and from `right_index` on are disjoint from the action.
    left_index: usize,
    right_index: usize,
    /// Children the action only partly covers, which fuse with it into a single deletion.
    fusible: Option<Vec<Action>>,
    /// Children the action strictly contains, which become its own children.
    contained: Option<Vec<Action>>,
}

/// Team#each_corrector は名前順ではなく、依存クラスの先行読込も含む実際の登録順でマージする。
/// 無限ループの原因一覧と同じ記録を使い、補正の競合と非互換宣言の優先順を揃える。
fn cop_merge_order(cop_name: &str) -> (usize, &str) {
    if let Some((rank, _)) = cop_autocorrect_metadata(cop_name) {
        return (rank, cop_name);
    }
    // テスト用の架空名は組込み cop の後へ置き、部門と名前による順序を保つ。
    const DEPARTMENTS: [&str; 9] = [
        "Bundler",
        "Gemspec",
        "Layout",
        "Lint",
        "Metrics",
        "Migration",
        "Naming",
        "Security",
        "Style",
    ];
    let department = cop_name.split_once('/').map_or(cop_name, |(head, _)| head);
    let index = DEPARTMENTS
        .iter()
        .position(|name| *name == department)
        .unwrap_or(DEPARTMENTS.len());
    (crate::formatter::COP_REGISTRY_ORDER.len() + index, cop_name)
}

/// `Cop::Base.autocorrect_incompatible_with`: the cops whose corrections are dropped for the rest
/// of the pass once this one has corrected something.
///
/// `Team#each_corrector` walks the cops in registry order and collects these into a skip set, so a
/// pair listed here never corrects the same file in the same pass however disjoint their edits
/// look. The correction they were denied is not lost: the next pass re-inspects the text the first
/// cop produced, where the second usually no longer has anything to say. `Style/IfUnlessModifier`
/// and `Style/Next` are the pair this matters most for -- folding an inner conditional into a
/// modifier shortens the body the outer one was going to rewrite.
///
/// Transcribed from the 1.89.0 sources; the unqualified names upstream writes (`RedundantSelf`,
/// `ColonMethodCall`) resolve to the declaring cop's own department.
fn autocorrect_incompatible_with(cop_name: &str) -> &'static [&'static str] {
    match cop_name {
        "Layout/DotPosition" => &["Style/RedundantSelf"],
        "Layout/EmptyLineBetweenDefs" => &["Layout/EmptyLines"],
        "Layout/HeredocArgumentClosingParenthesis" => &["Style/TrailingCommaInArguments"],
        "Layout/LineContinuationLeadingSpace" => &["Style/StringLiterals"],
        "Layout/SingleLineBlockChain" => &["Style/MapToHash"],
        "Layout/SpaceAroundOperators" => &["Style/SelfAssignment"],
        "Layout/SpaceBeforeBlockBraces" => &["Style/SymbolProc"],
        "Layout/SpaceBeforeFirstArg" => &["Style/MethodCallWithArgsParentheses"],
        "Layout/SpaceInsideBlockBraces" => &["Style/BlockDelimiters"],
        "Lint/AmbiguousOperator" => &["Naming/BlockForwarding"],
        "Lint/ConstantOverwrittenInRescue" => &[
            "Naming/RescuedExceptionsVariableName",
            "Style/RescueStandardError",
        ],
        "Lint/UnusedMethodArgument" => &["Style/ExplicitBlockArgument"],
        "Naming/BlockForwarding" => &[
            "Lint/AmbiguousOperator",
            "Style/ArgumentsForwarding",
            "Style/ExplicitBlockArgument",
        ],
        "Style/ArgumentsForwarding" => &["Naming/BlockForwarding", "Style/MethodDefParentheses"],
        "Style/BlockDelimiters" => &["Style/RedundantBegin"],
        "Style/ColonMethodCall" => &["Style/RedundantSelf"],
        "Style/ExplicitBlockArgument" => &["Lint/UnusedMethodArgument"],
        "Style/IfUnlessModifier" => &["Style/Next", "Style/SoleNestedConditional"],
        "Style/InverseMethods" => &["Style/Not", "Style/SymbolProc"],
        "Style/Lambda" => &["Style/SymbolProc"],
        "Style/LineEndConcatenation" => &["Style/RedundantInterpolation"],
        "Style/MapToHash" => &["Layout/SingleLineBlockChain"],
        "Style/MethodCallWithArgsParentheses" => {
            &["Style/NestedParenthesizedCalls", "Style/RescueModifier"]
        }
        "Style/MethodDefParentheses" => &["Style/ArgumentsForwarding"],
        "Style/NegatedIfElseCondition" => &["Style/InverseMethods", "Style/Not"],
        "Style/NestedParenthesizedCalls" => &["Style/MethodCallWithArgsParentheses"],
        "Style/Next" => &["Style/SafeNavigation"],
        "Style/RedundantBegin" => &["Style/BlockDelimiters"],
        "Style/RedundantInterpolation" => &["Style/LineEndConcatenation"],
        "Style/RedundantSelf" => &["Style/ColonMethodCall", "Layout/DotPosition"],
        "Style/RescueModifier" => &["Style/MethodCallWithArgsParentheses"],
        "Style/SelfAssignment" => &["Layout/SpaceAroundOperators"],
        "Style/Semicolon" => &["Style/SingleLineMethods"],
        "Style/SoleNestedConditional" => &["Style/NegatedIf", "Style/NegatedUnless"],
        "Style/SymbolProc" => &["Layout/SpaceBeforeBlockBraces"],
        "Style/TrailingCommaInArguments" => &["Layout/HeredocArgumentClosingParenthesis"],
        _ => &[],
    }
}

/// Whether the span addresses text that is actually there. Nothing a cop reports should fail this;
/// an offense whose edits do is dropped whole rather than half-applied.
fn is_addressable(start: usize, end: usize, source: &str) -> bool {
    start <= end
        && end <= source.len()
        && source.is_char_boundary(start)
        && source.is_char_boundary(end)
}

fn edits_are_addressable(offense: &Offense, source: &str) -> bool {
    offense
        .planned_corrections()
        .iter()
        .all(|edit| is_addressable(edit.start, edit.end, source))
}

/// `index` 番目の編集を、cop が corrector に渡した形の action にする。編集ごとのアンカーを
/// 先に使い、無い編集と使えない編集は offense の範囲から推す。See [`Offense::edit_anchor`].
fn edit_action(offense: &Offense, index: usize, edit: &Edit, source: &str) -> Action {
    offense
        .edit_anchor(index)
        .filter(|anchor| {
            let range = anchor.range();
            is_addressable(range.start, range.end, source)
        })
        .and_then(|anchor| Action::from_anchored_edit(edit, anchor))
        .unwrap_or_else(|| Action::from_edit(edit, anchor_range(offense, source)))
}

/// The range an insertion of this offense is taken to hang off. See [`Action::from_edit`].
fn anchor_range(offense: &Offense, source: &str) -> (usize, usize) {
    let (start, end) = offense
        .correction_anchor
        .unwrap_or((offense.start, offense.end));
    match is_addressable(start, end, source) {
        true => (start, end),
        false => (0, 0),
    }
}

/// Writes the corrector of every cop of one pass, and what the merge did with it, to stderr under
/// `SONICOP_TRACE_CORRECTORS`.
///
/// The output is the same shape upstream's correctors can be dumped in -- one line per scheduled
/// edit, `line:column-line:column` then the replacement -- so that the two can be read side by side.
/// Comparing text alone cannot say *why* a pass landed where it did: a cop whose corrections are
/// discarded whole still reports every offense as corrected, so the report is identical either way.
mod trace {
    pub(super) fn enabled() -> bool {
        static ENABLED: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| std::env::var_os("SONICOP_TRACE_CORRECTORS").is_some());
        *ENABLED
    }

    /// The 1-based line and column of an offset, counted the way an offense is reported.
    ///
    /// Slices with `get` rather than `[..]`: the offsets this walks are the ones under suspicion
    /// whenever the trace is worth reading, and an offset landing inside a multi-byte character
    /// would panic exactly when it is being investigated. A malformed edit should print oddly, not
    /// take the run down.
    fn position(source: &str, offset: usize) -> (usize, usize) {
        let mut offset = offset.min(source.len());
        while offset > 0 && !source.is_char_boundary(offset) {
            offset -= 1;
        }
        let head = &source[..offset];
        let line = head.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let start = head.rfind('\n').map_or(0, |index| index + 1);
        (line, head[start..].chars().count())
    }

    pub(super) fn span(source: &str, start: usize, end: usize) -> String {
        let (first_line, first_column) = position(source, start);
        let (last_line, last_column) = position(source, end);
        format!("{first_line}:{first_column}-{last_line}:{last_column}")
    }

    /// cop の corrector を、結合に渡す木の形で書き出す。
    ///
    /// 編集の範囲と文字だけでは、空白を `(` に置換する補正と、空白を消してその前に `(` を
    /// 挿す補正が同じに見える。結合で衝突するかどうかを決めるのはその違いなので、置換の
    /// `None` と `Some("")` を分け、挿入がどの範囲に掛かったかを子の字下げで出す。
    pub(super) fn tree(source: &str, root: &super::Action) {
        let mut stack: Vec<(&super::Action, usize)> =
            root.children.iter().rev().map(|child| (child, 0)).collect();
        while let Some((action, depth)) = stack.pop() {
            eprintln!(
                "      {}[{}..{}) {} before={:?} replace={:?} after={:?}",
                "  ".repeat(depth),
                action.begin_pos,
                action.end_pos,
                span(source, action.begin_pos, action.end_pos),
                action.insert_before,
                action.replacement,
                action.insert_after,
            );
            stack.extend(action.children.iter().rev().map(|child| (child, depth + 1)));
        }
    }

    /// Names a cop whose own edits cannot stand together, under `SONICOP_TRACE_OVERLAP`.
    ///
    /// The cop-side guard only covers the four cops that reparse their own correction; this end sees
    /// every cop, because every offense's edits pass through the correction tree. See
    /// [`crate::rules::support::report_overlap`] for what the stages mean.
    pub(super) fn overlap(report: &super::FileReport, index: usize, stage: &str) {
        if !crate::rules::support::overlap_trace_enabled() {
            return;
        }
        let offense = &report.offenses[index];
        let (line, column) = report.source.line_column(offense.start);
        crate::rules::support::report_overlap(
            offense.cop_name,
            &report.path.display().to_string(),
            line,
            column,
            stage,
        );
    }
}

/// Which cops a correction pass takes edits from.
///
/// RuboCop runs the cop that reads directives on a team of its own, after the inspection loop has
/// stopped changing the file, and then lets the loop run once more. Its removals therefore never
/// share a corrector with another cop's rewrite -- and since removing a directive comment shifts
/// every line under it, sharing one would land the other cop's edit a column out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Correcting {
    /// Every cop, which is what a caller correcting a single file in isolation wants.
    Everything,
    /// The inspection loop: everything but the cop that reads directives.
    ExceptDirectives,
    /// The pass that cop gets to itself once the loop has settled.
    DirectivesOnly,
}

impl Correcting {
    fn takes(self, cop_name: &str) -> bool {
        let directives = cop_name == REDUNDANT_COP_DISABLE_DIRECTIVE;
        match self {
            Self::Everything => true,
            Self::ExceptDirectives => !directives,
            Self::DirectivesOnly => directives,
        }
    }
}

pub fn corrected_text(
    report: &mut FileReport,
    mode: CorrectMode,
    correcting: Correcting,
) -> (String, usize) {
    match corrected_text_with_provenance(report, mode, correcting, None, None) {
        Ok((text, count, _, _)) => (text, count),
        Err(_) => (report.source.text().to_owned(), 0),
    }
}

type CorrectedPass = (String, usize, Option<Vec<usize>>, Vec<usize>);

fn corrected_text_with_provenance(
    report: &mut FileReport,
    mode: CorrectMode,
    correcting: Correcting,
    introduced: Option<&[usize]>,
    retained_crlf: Option<&mut LineEndingProvenance>,
) -> Result<CorrectedPass, CorrectionRejection> {
    if mode == CorrectMode::None {
        return Ok((
            report.source.text().to_owned(),
            0,
            introduced.map(<[usize]>::to_vec),
            Vec::new(),
        ));
    }
    let source = report.source.text();

    let candidates = correction_candidates(report, mode, correcting, source);
    let parenthesizing: HashSet<_> = candidates
        .iter()
        .filter_map(|&index| {
            let offense = &report.offenses[index];
            matches!(
                offense.cop_name,
                "Lint/AmbiguousOperator" | "Lint/AmbiguousRegexpLiteral"
            )
            .then_some(offense.planned_corrections())
        })
        .flatten()
        .filter(|edit| edit.end == edit.start + 1 && edit.replacement == "(")
        .map(|edit| (edit.start, edit.end))
        .collect();

    let mut run = Action::root();
    // 本家は cop が編集を積んだ時点で corrected を立てるため、後で skip や衝突で
    // 補正を捨てても corrected の報告は残る。実際の適用数・文字の由来とは区別する。
    let mut corrected: Vec<usize> = Vec::new();
    // 実行全体の corrector に届いた編集だけが、次のパスを走らせる根拠になる。
    let mut applied = 0;
    let mut rejection = None;
    // `Team#each_corrector` の互換性宣言による skip。autocorrect_incompatible_with を参照。
    let mut skips: HashSet<&'static str> = HashSet::new();
    let mut rest = candidates.as_slice();
    if trace::enabled() {
        eprintln!("=== cop ごとの corrector (マージ順)");
    }
    while let Some(&first) = rest.first() {
        let cop_name = report.offenses[first].cop_name;
        let taken = rest
            .iter()
            .take_while(|index| report.offenses[**index].cop_name == cop_name)
            .count();
        let (group, remainder) = rest.split_at(taken);
        rest = remainder;

        let skipped = skips.contains(cop_name);
        if trace::enabled() {
            eprintln!("  {cop_name}{}", if skipped { "  (skip 済み)" } else { "" });
        }

        let (cop, placed) = build_cop_corrector(report, group, source);

        if cop.children.is_empty() {
            continue;
        }
        if trace::enabled() {
            eprintln!("    木:");
            trace::tree(source, &cop);
        }
        corrected.extend(&placed);
        // 本家はマージの前に互換性宣言を読み、編集が後で衝突しても非互換 cop を見送る。
        skips.extend(autocorrect_incompatible_with(cop_name));

        // 先行 cop が非互換と宣言した補正は、次のパスへ回す。
        if skipped {
            trace_outcome("skip ", cop_name);
            if trace::enabled() {
                eprintln!("    見送り (非互換の宣言)");
            }
            continue;
        }
        // Team#merge_corrector! と同じく、衝突した cop の補正はこのファイルで全件捨てる。
        let merged = if run.merge_disjoint_children(&cop.children) {
            Ok(())
        } else {
            run.clone().combine_children(&cop.children).map(|merged| {
                run = merged;
            })
        };
        match merged {
            Ok(()) => {
                applied += placed.len();
                // WordArrayの拒否予定は非emptyな配列置換1件で、採用後に親へ飲まれない。
                // skip・clashで捨てた予定を理由に、無関係な安全補正まで拒否しない。
                rejection = rejection.or_else(|| {
                    placed
                        .iter()
                        .find_map(|&index| report.offenses[index].correction_rejection())
                });
                trace_outcome("apply", cop_name);
                if trace::enabled() {
                    eprintln!("    取り込み");
                }
            }
            Err(_) => {
                trace_outcome("clash", cop_name);
                if trace::enabled() {
                    eprintln!("    ★ 丸ごと捨てた");
                }
            }
        }
    }

    // Actionの配置だけを調べ、危険な置換をrewriteにもcorrected表示にも渡さない。
    if let Some(reason) = rejection {
        return Err(reason);
    }
    for index in &corrected {
        // add_offense の外で積まれた編集は診断の corrected を変えない。
        // Offense::corrected_without_status を参照。
        if !report.offenses[*index].corrections_detached {
            report.offenses[*index].corrected = true;
        }
    }
    // corrected は本家の報告状態であり、skip・clash で捨てた編集にも立つ。
    // 最終 Action 木に残った同一の opening 置換だけを、安全性の判定へ渡す。
    let mut replacements = Vec::new();
    run.ordered_replacements(&mut replacements);
    let openings = replacements
        .into_iter()
        .filter_map(|(begin, end, replacement, _)| {
            (replacement == "(" && parenthesizing.contains(&(begin, end))).then_some(begin)
        })
        .collect();
    let (text, provenance) = match retained_crlf {
        Some(retained) => run.rewrite_with_line_endings(source, introduced, Some(retained)),
        None => run.rewrite(source, introduced),
    };
    Ok((text, applied, provenance, openings))
}

fn correction_candidates(
    report: &FileReport,
    mode: CorrectMode,
    correcting: Correcting,
    source: &str,
) -> Vec<usize> {
    // An offense is corrected whole or not at all: its edits are one rewrite the cop asked for, and
    // applying half of them would leave source the cop never intended to produce.
    let mut candidates: Vec<usize> = report
        .offenses
        .iter()
        .enumerate()
        .filter(|(_, offense)| {
            correcting.takes(offense.cop_name)
                && !offense.planned_corrections().is_empty()
                && (mode == CorrectMode::All
                    || offense.planned_corrections().iter().all(|edit| edit.safe))
                && edits_are_addressable(offense, source)
        })
        .map(|(index, _)| index)
        .collect();
    // Offenses arrive ordered by position; a stable sort by cop leaves them that way within a cop.
    candidates.sort_by_key(|index| cop_merge_order(report.offenses[*index].cop_name));
    candidates
}

/// cop の corrector を組み立て、補正を受け入れた offense を返す。
fn build_cop_corrector(report: &FileReport, group: &[usize], source: &str) -> (Action, Vec<usize>) {
    // 本家は配置できない offense だけを捨て、同じ cop の残りの補正は保つ。
    let mut cop = Action::root();
    let mut placed = Vec::new();
    for &index in group {
        if trace::enabled() {
            for edit in report.offenses[index].planned_corrections() {
                eprintln!(
                    "      {:16} {:?}",
                    trace::span(source, edit.start, edit.end),
                    edit.replacement
                );
            }
        }
        // `Corrector#replace` と同様、何もしない編集を除く入口の `combine` を使う。
        let reported = &report.offenses[index];
        let offense = reported.planned_corrections().iter().enumerate().try_fold(
            Action::root(),
            |tree, (position, edit)| {
                let mut action = edit_action(reported, position, edit, source);
                action.characters_from = reported.correction_characters_from;
                tree.combine(&action)
            },
        );
        let Ok(offense) = offense else {
            if trace::enabled() {
                eprintln!("      ★ この offense の中で衝突");
            }
            // 同じ corrector への二重書き込みを、再解析する cop に限らずここで監視する。
            trace::overlap(report, index, "offense-tree");
            continue;
        };
        if offense.children.is_empty() {
            continue;
        }
        if !cop.append_disjoint_children(&offense.children) {
            let Ok(merged) = cop.clone().combine_children(&offense.children) else {
                if trace::enabled() {
                    eprintln!("      ★ cop の corrector に入らなかった (この offense だけ捨てた)");
                }
                trace::overlap(report, index, "cop-tree");
                continue;
            };
            cop = merged;
        }
        placed.push(index);
        trace_edits(
            report.offenses[index].cop_name,
            &report.offenses[index],
            source,
        );
    }
    (cop, placed)
}

/// 一時的ではない計装。`SONICOP_TRACE_EDITS` が立っているときだけ、1 パスの中で
/// **どの cop が何を書き換えようとし、それが採用されたか**を stderr へ出す。
///
/// 出力を比べるだけでは「誰の Edit がどう混ざったか」が見えない。`%q{...}` が `%(...)` になる
/// ように、**どの cop も単独では出さない字面**が出ることがあり、そこは出力からは遡れない。
///
/// 既定の実行には 1 バイトも出さないので、測定に使うバイナリのまま有効にできる。計装版と
/// 測定版を分けると、測るたびにビルドが要り、そのビルドが測定を壊す。
fn trace_edits(cop_name: &str, offense: &Offense, source: &str) {
    if std::env::var_os("SONICOP_TRACE_EDITS").is_none() {
        return;
    }
    for edit in &offense.corrections {
        let before = source.get(edit.start..edit.end).unwrap_or("<範囲外>");
        eprintln!(
            "TRACE edit  {cop_name} [{}..{}] {before:?} -> {:?}",
            edit.start, edit.end, edit.replacement
        );
    }
}

/// 同上。cop 単位の採否 (`Team#merge_corrector!` の結果) を出す。
fn trace_outcome(outcome: &str, cop_name: &str) {
    if std::env::var_os("SONICOP_TRACE_EDITS").is_some() {
        eprintln!("TRACE {outcome} {cop_name}");
    }
}

const MAX_CORRECTION_PASSES: usize = 200;

/// What a pass's text is remembered by, so that the loop can tell it has come round again.
///
/// `Runner#check_for_infinite_loop` keeps `processed_source.checksum` rather than the text, and the
/// difference shows on a file a cop keeps adding to. `Regexp.new("a\\d]b")` grows about 1.5x per
/// pass under `Lint/UnescapedBracketInRegexp`, measured identical to upstream pass for pass, so
/// holding every pass costs the sum of that series -- roughly three times the current text -- and
/// comparing against every pass re-reads all of it. Neither form bounds the growth: that is
/// upstream's defect, reproduced here rather than fixed.
///
/// The length rides along with the hash because it is free and makes a collision take two
/// coincidences instead of one. A collision would report a loop that is not there, which is a
/// worse failure than the one being avoided.
type SourceDigest = (u64, usize);

fn digest(text: &str) -> SourceDigest {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    (hasher.finish(), text.len())
}

type OffenseKey = (usize, usize, &'static str, String, Severity);

fn offense_key(offense: &Offense, source: &SourceFile) -> OffenseKey {
    let (line, column) = offense.start_position(source);
    (
        line,
        column,
        offense.cop_name,
        offense.message.clone(),
        offense.severity,
    )
}

/// `Runner#inspect_file`'s `offenses_by_iteration.flatten.uniq`.
///
/// `Offense#eql?` compares `COMPARISON_ATTRIBUTES` -- the line, the column, the cop, the message and
/// the severity -- rather than the range, so two offenses a cop reported over different spans of the
/// same starting point are one offense in the report. `Style/CombinableDefined` is where this shows:
/// it reports on every `and` of a chain, and the nested ones all begin where the outermost does.
///
/// The list is sorted on exactly those attributes, so duplicates are already adjacent, and the sort
/// is stable, so the one kept is the one the cop reported first -- which is the one upstream keeps.
fn dedupe_offenses(offenses: &mut Vec<Offense>, source: &SourceFile) {
    offenses.dedup_by(|later, earlier| offense_key(later, source) == offense_key(earlier, source));
}

fn sort_offenses(offenses: &mut [Offense], source: &SourceFile) {
    offenses.sort_by(|left, right| {
        let (left_line, left_column) = left.start_position(source);
        let (right_line, right_column) = right.start_position(source);
        (
            left_line,
            left_column,
            left.cop_name,
            &left.message,
            left.severity,
        )
            .cmp(&(
                right_line,
                right_column,
                right.cop_name,
                &right.message,
                right.severity,
            ))
    });
}

/// Offenses an earlier autocorrect pass already fixed. Re-inspecting the rewritten text cannot
/// rediscover them, so without this ledger every `[Corrected]` marker and every corrected count
/// would vanish the moment the fix landed.
#[derive(Default)]
struct CorrectionLog {
    offenses: Vec<Offense>,
    keys: HashSet<OffenseKey>,
    /// The cops credited with each pass's corrections, used to name the culprits of a loop.
    cops_by_pass: Vec<Vec<&'static str>>,
}

impl CorrectionLog {
    /// `keep_uncorrected` for the pass RuboCop gives the directive cop to itself. The inspection
    /// loop had already settled by then, so `Runner#add_redundant_disables` unions that whole
    /// offense list -- corrected or not -- with what the loop finds afterwards. Removing a
    /// directive comment moves every line under it, so the same offense turns up in both lists at
    /// two different line numbers and both are reported.
    fn record_pass(&mut self, report: &mut FileReport, keep_uncorrected: bool) {
        let source = &report.source;
        let mut cops: Vec<&'static str> = Vec::new();
        for offense in &mut report.offenses {
            if !(offense.corrected || keep_uncorrected) {
                continue;
            }
            offense.freeze_location(source);
            if !offense.corrected {
                if self.keys.insert(offense_key(offense, source)) {
                    self.offenses.push(offense.clone());
                }
                continue;
            }
            if !cops.contains(&offense.cop_name) {
                cops.push(offense.cop_name);
            }
            if self.keys.insert(offense_key(offense, source)) {
                self.offenses.push(offense.clone());
            }
        }
        self.cops_by_pass.push(cops);
    }

    fn root_cause(
        &self,
        loop_start: usize,
        last: &[Offense],
        mode: CorrectMode,
        config: &Config,
        selection: &Selection,
    ) -> String {
        // 本家は次の反復へ入るときに前回の未補正 offense を落とす。
        // 上限・循環で止まった最後の反復だけは、補正できなかったものも原因に残る。
        let last_cops: Vec<_> = last
            .iter()
            .filter(|offense| {
                cop_autocorrect_metadata(offense.cop_name).is_some_and(|(_, support)| support)
                    && config.rule_autocorrect_enabled(offense.cop_name, selection.editor_mode)
                    && (mode == CorrectMode::All
                        || (config.rule_safe(offense.cop_name)
                            && config.rule_safe_autocorrect(offense.cop_name)))
            })
            .map(|offense| offense.cop_name)
            .collect();
        self.cops_by_pass
            .get(loop_start..)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(offset, cops)| {
                let mut cops = if loop_start + offset + 1 == self.cops_by_pass.len() {
                    last_cops.clone()
                } else {
                    cops.clone()
                };
                // Commissioner の結果は位置順ではなく、cop の登録順に連結される。
                cops.sort_unstable_by_key(|cop| {
                    cop_autocorrect_metadata(cop).map_or(usize::MAX, |(rank, _)| rank)
                });
                cops.dedup();
                cops.join(", ")
            })
            .collect::<Vec<_>>()
            .join(" -> ")
    }

    /// Union the ledger with the last pass the way RuboCop does: an offense a later pass
    /// rediscovered at the same place loses to the corrected entry already on file.
    fn merge_into(self, mut report: FileReport) -> (FileReport, usize) {
        let Self {
            mut offenses, keys, ..
        } = self;
        let source = &report.source;
        offenses.extend(
            report
                .offenses
                .drain(..)
                .filter(|offense| !keys.contains(&offense_key(offense, source))),
        );
        sort_offenses(&mut offenses, source);
        dedupe_offenses(&mut offenses, source);
        let corrected_count = offenses.iter().filter(|offense| offense.corrected).count();
        report.offenses = offenses;
        (report, corrected_count)
    }
}

pub(crate) fn cop_autocorrect_metadata(cop: &str) -> Option<(usize, bool)> {
    // offense ごとの correctable とは別の、本家クラスの補正機能と実際の登録順。
    static METADATA: std::sync::LazyLock<std::collections::BTreeMap<&'static str, (usize, bool)>> =
        std::sync::LazyLock::new(|| {
            let cops: Vec<(&str, bool)> =
                serde_json::from_str(include_str!("../config/autocorrect_support.json"))
                    .expect("vendored autocorrect metadata must be valid");
            cops.into_iter()
                .enumerate()
                .map(|(rank, (name, support))| (name, (rank, support)))
                .collect()
        });
    METADATA.get(cop).copied()
}

/// 補正が収束するまで反復した結果と、書き戻しの成否。
pub struct CorrectionOutcome {
    pub report: FileReport,
    pub text: String,
    pub corrected_count: usize,
    /// Team#updated_source_file? と同じく、実際のテキスト変更が書き戻しを決める。
    /// corrected_without_status の編集は診断の修正件数に入らないため、件数では代用できない。
    pub rewritten: bool,
    /// 最終出力に残る、原本からの複写と確認できない非 ASCII 文字。
    pub(crate) introduced_non_ascii: bool,
    /// 本家は非収束をファイルごとに報告し、最後の補正を残して残りの検査を続ける。
    pub infinite_loop: Option<String>,
    /// 有効な原本を構文エラーにする補正を取り消したときに設定する。
    /// ディスクには有効な原本が残るため、存在しない Lint/Syntax は報告しない。
    /// 未補正の診断を返し、補正失敗として終了する。
    pub rollback: Option<String>,
}

/// 下の構文保護を外す測定専用の設定。`withhold_unparsable` を参照。
///
/// 共有される .rubocop.yml で全利用者の保護を無効にできないよう、環境変数に限る。
pub const NO_SYNTAX_GUARD: &str = "SONICOP_NO_SYNTAX_GUARD";

/// 解析不能を示す fatal の Lint/Syntax が報告されているか。
fn holds_fatal_syntax(report: &FileReport) -> bool {
    report
        .offenses
        .iter()
        .any(|offense| offense.severity == Severity::Fatal && offense.cop_name == "Lint/Syntax")
}

/// 有効だった原本を構文エラーにする補正は、ファイル単位で全パスを取り消す。
///
/// 本家の Layout/LineLength は heredoc を開く行を折って構文エラーを作ることがある。
/// バイト一致だけでは両者が同じように壊れた場合を見逃すため、補正後も検査する。
/// 最初から無効だった入力には発火せず、警告だけでも補正を止めない。
///
/// Lint/Syntax の判定に加え、ネイティブ Prism でスクリプトとしての構文も比較する。
/// 旧版の診断には tree-sitter が受理しすぎる構文があるため、診断の互換性とは分ける。
/// 構文が通ったまま値や制御の意味が変わる補正までは検出できない。
///
/// ディスクには原本が残るため、適用されていない補正を corrected として報告しない。
/// SONICOP_NO_SYNTAX_GUARD は、保護が止めた出力を本家と比較して誤拒否を測るためだけに
/// 用意した。共有設定から全利用者の保護を外せないよう環境変数に限定している。
fn withhold_unparsable(
    outcome: CorrectionOutcome,
    original: &str,
    original_bytes: Option<&[u8]>,
    started_valid: bool,
    config: &Config,
    selection: &Selection,
    plan: &RulePlan,
) -> Result<CorrectionOutcome> {
    if !started_valid || !outcome.rewritten {
        return Ok(outcome);
    }
    if selection.skip_syntax_guard || std::env::var_os(NO_SYNTAX_GUARD).is_some() {
        return Ok(outcome);
    }
    // CLI は入力バイト列を渡す。メモリ上の公開 API では宣言に従って原本を再構成する。
    let reconstructed = original_bytes.is_none().then(|| {
        output_bytes(original, declared_label(original.as_bytes()).as_deref())
            .unwrap_or_else(|_| original.as_bytes().to_vec())
    });
    let original_bytes = original_bytes.unwrap_or_else(|| reconstructed.as_deref().unwrap());
    // 符号化できない出力は書き戻し処理が別の理由で拒否する。構文破壊とは取り違えない。
    let invalid_script = correction_bytes(
        &outcome.text,
        original_bytes,
        Some(outcome.introduced_non_ascii),
    )
    .is_ok_and(|bytes| {
        crate::prism::would_invalidate(original_bytes, &bytes, config.target_ruby_version())
    });
    if !holds_fatal_syntax(&outcome.report) && !invalid_script {
        return Ok(outcome);
    }
    let path = outcome.report.path.clone();
    let reason = format!(
        "Autocorrection was not written to {} because it introduced a syntax error.",
        path.display()
    );
    rollback_correction(
        path,
        original,
        outcome.infinite_loop,
        reason,
        config,
        selection,
        plan,
    )
}

fn rollback_correction(
    path: PathBuf,
    original: &str,
    infinite_loop: Option<String>,
    reason: String,
    config: &Config,
    selection: &Selection,
    plan: &RulePlan,
) -> Result<CorrectionOutcome> {
    // 書かない補正後のソースではなく、ディスクに残る原本の診断を返す。
    let report = inspect_planned(
        path,
        original.to_owned(),
        config,
        selection,
        plan,
        true,
        None,
    )?;
    Ok(CorrectionOutcome {
        report,
        text: original.to_owned(),
        corrected_count: 0,
        rewritten: false,
        introduced_non_ascii: false,
        infinite_loop,
        rollback: Some(reason),
    })
}

/// 元の改行がすべて CRLF なら、書き戻しでもその形式を保つ。
fn uses_crlf_only(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut found = false;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            if index == 0 || bytes[index - 1] != b'\r' {
                return false;
            }
            found = true;
        }
    }
    found
}

/// LF の解析結果へ原本の保護と Windows disk の native 改行を適用する。
fn restore_crlf(
    mut outcome: CorrectionOutcome,
    preserve_crlf: bool,
    retained_crlf: Option<&LineEndingProvenance>,
    native_newlines: bool,
    config: &Config,
) -> CorrectionOutcome {
    if outcome.rewritten && outcome.rollback.is_none() {
        outcome.text = physical_corrected_text(
            &outcome.text,
            preserve_crlf,
            retained_crlf,
            native_newlines,
            config,
        );
    }
    outcome
}

/// DATA と code NUL 以降を、Windows のテキスト書込みによる CR 挿入から保護する。
/// 同名の文字列・コメントは境界ではないため、文字列検索だけで切り分けない。
fn unparsed_tail_start(text: &str, config: &Config) -> Option<usize> {
    unparsed_tail(text, config).map(|(start, _)| start)
}

/// tail 開始に加え、実際の DATA marker の終端 LF を返す。
/// marker の CRLF を CRCRLF にすると、構文が通ったまま DATA が未定義になる。
fn unparsed_tail(text: &str, config: &Config) -> Option<(usize, Option<usize>)> {
    let semantics = crate::regexp_ast::Semantics::from_config(config);
    let projected = crate::nul_bytes::as_ruby_reads_it_configured(text, semantics);
    let parsed = projected.as_deref().unwrap_or(text);
    let nul_end = (parsed.len() < text.len()).then_some(parsed.len());
    if !parsed.contains("__END__") {
        return nul_end.map(|start| (start, None));
    }
    let tree = crate::parser::parse_configured(parsed, semantics)?;
    let root = tree.root_node();
    for index in 0..root.child_count() {
        let Some(node) = root.child(index) else {
            continue;
        };
        if node.kind() == "uninterpreted" {
            // uninterpreted は marker 行の終端 LF から始まる。DATA.read はその次から。
            let start = node.start_byte();
            let newline = text[start..].find('\n').map(|offset| start + offset);
            let end = newline.map_or(start, |offset| offset + 1);
            return Some((nul_end.map_or(end, |nul| nul.min(end)), newline));
        }
    }
    nul_end.map(|start| (start, None))
}

/// Windows Ruby の通常 IO と同じく、既存の CR に関係なく LF の前へ CR を足す。
/// compact JSON のように生の LF が無い出力では、本文を複写しない。
pub(crate) fn native_textmode_bytes(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !cfg!(windows) || !bytes.contains(&b'\n') {
        return Cow::Borrowed(bytes);
    }
    let mut output =
        Vec::with_capacity(bytes.len() + bytes.iter().filter(|b| **b == b'\n').count());
    for &byte in bytes {
        if byte == b'\n' {
            output.push(b'\r');
        }
        output.push(byte);
    }
    Cow::Owned(output)
}

/// stdin の保存予定 bytes を確定する。呼出側はこの結果を構文保護へ渡してから出力する。
/// DATA/code NUL 以降と、値を変える既存 CRLF literal には新しい CR を挿入しない。
pub(crate) fn stdin_output_bytes<'a>(
    bytes: &'a [u8],
    config: &Config,
    rewritten: bool,
) -> Result<Cow<'a, [u8]>> {
    if !cfg!(windows) || !bytes.contains(&b'\n') {
        return Ok(Cow::Borrowed(bytes));
    }
    if !bytes.contains(&b'\r')
        && !bytes.contains(&0)
        && !bytes.windows(7).any(|part| part == b"__END__")
    {
        return Ok(native_textmode_bytes(bytes));
    }
    let Decoded::Text(text) = decoded_bytes(bytes.to_vec()) else {
        // 通常の無効な Ruby は、従来どおり構文診断と native echo を返す。
        if !crate::prism::script_is_valid(bytes, config.target_ruby_version()) {
            return Ok(native_textmode_bytes(bytes));
        }
        bail!("cannot determine literal boundaries in undecodable stdin bytes");
    };
    let unparsed = unparsed_tail(&text, config);
    let tail = unparsed.map_or(text.len(), |(start, _)| start);
    let mut protected_newlines: Vec<_> = unparsed
        .and_then(|(_, newline)| newline)
        .filter(|&offset| offset > 0 && text.as_bytes()[offset - 1] == b'\r')
        .into_iter()
        .collect();
    if !rewritten
        && text.contains("\r\n")
        && crate::prism::script_is_valid(bytes, config.target_ruby_version())
    {
        // Prism token の所有範囲を使い、escape_sequence・語境界・補間コードを区別する。
        let raw_positions =
            crate::prism::literal_crlf_positions(bytes, config.target_ruby_version())
                .map_err(|reason| anyhow::anyhow!(reason))?;
        let mut raw_positions = raw_positions;
        raw_positions.sort_unstable();
        raw_positions.dedup();
        let raw_newlines = bytes.iter().enumerate().filter(|(_, byte)| **byte == b'\n');
        for ((raw, _), (decoded, _)) in raw_newlines.zip(text.match_indices('\n')) {
            if raw_positions.binary_search(&raw).is_ok() {
                protected_newlines.push(decoded);
            }
        }
        protected_newlines.sort_unstable();
        protected_newlines.dedup();
    }
    // 復号後の UTF-8 offset を raw bytes の offset と混ぜない。Ruby の対応する
    // ASCII 互換符号化では LF の順序が不変なので、改行の序数だけを受け渡す。
    let convert: Vec<_> = text
        .match_indices('\n')
        .map(|(offset, _)| offset < tail && protected_newlines.binary_search(&offset).is_err())
        .collect();
    if convert.len() != bytes.iter().filter(|byte| **byte == b'\n').count() {
        if !crate::prism::script_is_valid(bytes, config.target_ruby_version()) {
            return Ok(native_textmode_bytes(bytes));
        }
        bail!("cannot map decoded stdin newlines to the original bytes");
    }
    let mut output = Vec::with_capacity(bytes.len() + convert.len());
    let mut newline = 0;
    for (offset, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            // Team が更新した stdin buffer は LF。既存の物理 CRLF 復元と二重に足さない。
            if convert[newline] && !(rewritten && offset > 0 && bytes[offset - 1] == b'\r') {
                output.push(b'\r');
            }
            newline += 1;
        }
        output.push(byte);
    }
    if output == bytes {
        Ok(Cow::Borrowed(bytes))
    } else {
        Ok(Cow::Owned(output))
    }
}

fn physical_corrected_text(
    text: &str,
    preserve_crlf: bool,
    retained_crlf: Option<&LineEndingProvenance>,
    native_newlines: bool,
    config: &Config,
) -> String {
    let text = restored_line_endings(text, preserve_crlf, retained_crlf);
    if !native_newlines {
        return text;
    }
    let tail = unparsed_tail_start(&text, config).unwrap_or(text.len());
    let mut physical = text[..tail].replace('\n', "\r\n");
    physical.push_str(&text[tail..]);
    physical
}

fn correction_raw_source(
    text: &str,
    preserve_crlf: bool,
    retained_crlf: Option<&LineEndingProvenance>,
    windows_disk: bool,
    native_newlines: bool,
    config: &Config,
) -> Option<String> {
    if !windows_disk {
        return None;
    }
    if unparsed_tail_start(text, config).is_some() {
        // 保存した DATA の LF も EmptyLines の raw quick guard に影響する。
        // 保護 tail を隠す仮想 CRLF にすると、次回の物理入力で新たな補正を生じる。
        Some(physical_corrected_text(
            text,
            preserve_crlf,
            retained_crlf,
            native_newlines,
            config,
        ))
    } else {
        Some(text.replace('\n', "\r\n"))
    }
}

/// 最終書出しと補正前script検査を、同じ物理改行の復元へ向ける。
/// currentの位置表を次passへ変更しないよう、pureの最終正規化は複製へ適用する。
fn restored_line_endings(
    text: &str,
    preserve_crlf: bool,
    retained_crlf: Option<&LineEndingProvenance>,
) -> String {
    if preserve_crlf {
        // 元pairは全LFのdefault CR、markerは追加の元bare CRだけを表す。
        let mut text = text.to_owned();
        let mut retained_crlf = retained_crlf.cloned();
        normalize_corrected_line_endings(&mut text, &mut None, &mut retained_crlf);
        if let Some(state) = retained_crlf {
            text = restore_copied_crlf(&text, &state.hidden_cr);
        }
        text.replace('\n', "\r\n")
    } else if let Some(state) = retained_crlf {
        restore_copied_crlf(text, &state.hidden_cr)
    } else {
        text.to_owned()
    }
}

/// mixedの挿入LFやDATA内の元LFは変えず、copyした元CRLFだけを書出し時に戻す。
fn restore_copied_crlf(text: &str, positions: &[usize]) -> String {
    let mut output = String::with_capacity(text.len() + positions.len());
    let mut last = 0;
    for &position in positions {
        debug_assert_eq!(text.as_bytes().get(position), Some(&b'\n'));
        output.push_str(&text[last..position]);
        output.push('\r');
        last = position;
    }
    output.push_str(&text[last..]);
    output
}

/// 解析viewのCR除去を、文字由来と元改行由来の両方のbyte位置へ同じように適用する。
fn normalize_corrected_line_endings(
    text: &mut String,
    introduced: &mut Option<Vec<usize>>,
    retained_crlf: &mut Option<LineEndingProvenance>,
) {
    let removed: Vec<_> = text
        .match_indices("\r\n")
        .map(|(offset, _)| offset)
        .collect();
    let shift = |positions: &mut Vec<usize>| {
        let mut preceding = 0;
        for position in positions {
            while removed
                .get(preceding)
                .is_some_and(|offset| *offset < *position)
            {
                preceding += 1;
            }
            *position -= preceding;
        }
    };
    if let Some(positions) = introduced {
        shift(positions);
    }
    if let Some(state) = retained_crlf {
        shift(&mut state.hidden_cr);
        // 元bare CRが次passでCRLFのCRになっても、元byteを失わず復元markerへ移す。
        // 同じLFに既存markerがあっても相殺しない。copy LF自体の削除/置換だけが消す。
        state.visible_cr.retain_mut(|position| {
            if removed.binary_search(position).is_ok() {
                let lf = *position + 1;
                state
                    .hidden_cr
                    .push(lf - removed.partition_point(|offset| *offset < lf));
                false
            } else {
                *position -= removed.partition_point(|offset| *offset < *position);
                true
            }
        });
        state.hidden_cr.sort_unstable();
        state.physical = false;
    }
    if !removed.is_empty() {
        *text = text.replace("\r\n", "\n");
    }
}

pub fn correct_file(
    report: FileReport,
    mode: CorrectMode,
    config: &Config,
    selection: &Selection,
) -> Result<CorrectionOutcome> {
    correct_file_with_original(report, mode, config, selection, None, false)
}

/// stdin とディスクの補正は、復号前と書き戻し予定のバイト列で構文を比較する。
pub(crate) fn correct_file_with_original(
    report: FileReport,
    mode: CorrectMode,
    config: &Config,
    selection: &Selection,
    original_bytes: Option<&[u8]>,
    disk: bool,
) -> Result<CorrectionOutcome> {
    correct_file_with_views(report, mode, config, selection, original_bytes, disk, None)
}

pub(crate) fn correct_shared_stdin(
    report: FileReport,
    mode: CorrectMode,
    config: &Config,
    selection: &Selection,
    original: &[u8],
    shared: &mut SharedStdinState,
) -> Result<CorrectionOutcome> {
    correct_file_with_views(
        report,
        mode,
        config,
        selection,
        Some(original),
        false,
        Some(shared),
    )
}

fn remember_stdin_state(
    result: Result<CorrectionOutcome>,
    shared: Option<&mut SharedStdinState>,
    logical: Option<String>,
    retained_crlf: Option<&LineEndingProvenance>,
    preserve_crlf: bool,
) -> Result<CorrectionOutcome> {
    let outcome = result?;
    if outcome.rollback.is_none()
        && let Some(shared) = shared
        && let Some(logical) = logical
    {
        shared.logical = Some(logical);
        shared.retained_crlf = retained_crlf.cloned();
        shared.preserve_crlf = preserve_crlf;
    }
    Ok(outcome)
}

fn correct_file_with_views(
    mut report: FileReport,
    mode: CorrectMode,
    config: &Config,
    selection: &Selection,
    original_bytes: Option<&[u8]>,
    disk: bool,
    shared: Option<&mut SharedStdinState>,
) -> Result<CorrectionOutcome> {
    let indexed = if selection.project_index.is_none() && crate::ProjectIndex::enabled(config) {
        let configs = ConfigStore::new(config.clone(), false, false);
        Some(selection.with_project_index(std::slice::from_ref(&report.path), &configs)?)
    } else {
        None
    };
    let selection = indexed.as_ref().unwrap_or(selection);
    let mut text = report.source.text().to_owned();
    if mode == CorrectMode::None {
        return Ok(CorrectionOutcome {
            report,
            text,
            corrected_count: 0,
            rewritten: false,
            introduced_non_ascii: false,
            infinite_loop: None,
            rollback: None,
        });
    }

    let original = text.clone();
    let inherited = shared.as_deref().filter(|state| state.logical.is_some());
    let preserve_crlf =
        inherited.map_or_else(|| uses_crlf_only(&original), |state| state.preserve_crlf);
    let windows_disk = cfg!(windows) && disk;
    // 元 CRLF/mixed の保存方針は維持する。純 LF の通常コードだけ File.write に合わせる。
    let native_newlines = windows_disk && !original.contains("\r\n");
    // 全CRLFでも元bare CRだけは追跡する。元pairは書出しのdefault CRへ任せて二重計数しない。
    let mut retained_crlf = if let Some(inherited) = inherited {
        // 対象間でも LF 座標の位置表を継承し、補正が挿入した LF を元 pair へ化けさせない。
        inherited.retained_crlf.clone()
    } else {
        let track_line_endings = if preserve_crlf {
            original
                .match_indices('\r')
                .any(|(position, _)| original.as_bytes().get(position + 1) != Some(&b'\n'))
        } else {
            original.contains('\r')
        };
        track_line_endings.then(|| {
            let mut state = LineEndingProvenance::new(&original);
            if preserve_crlf {
                state.hidden_cr.clear();
            }
            state
        })
    };
    // 本家は無効な入力も補正するため、元から有効だったファイルだけを保護する。
    let started_valid = !holds_fatal_syntax(&report);
    let path = report.path.clone();
    let mut log = CorrectionLog::default();
    let mut sources = vec![digest(&text)];
    let mut rewritten = false;
    // UTF-8 の通常実行では追加の走査・確保を行わない。
    let mut provenance = declared_label(original.as_bytes())
        .filter(|label| is_legacy_label(label))
        .map(|_| Vec::new());
    // 全パスで同じファイル・設定を使うため、規則の計画は反復前に一度だけ解決する。
    let plan = RulePlan::build(config, selection);
    // Runner#add_redundant_disables は収束後に一度だけ走り、その後の反復には持ち込まない。
    let mut directives_pending = plan.checks_directives();
    // 本家は 201 パス目の要求時に止めるため、実際には 200 回だけ補正する。
    // ファイルを増やす cop では一回のずれが残る内容を変える。
    for pass in 0..MAX_CORRECTION_PASSES {
        // 補正前のscript検査には現在の位置表が必要。次passの由来で上書きしない。
        let mut next_retained_crlf = retained_crlf.clone();
        let correction = corrected_text_with_provenance(
            &mut report,
            mode,
            Correcting::ExceptDirectives,
            provenance.as_deref(),
            next_retained_crlf.as_mut(),
        );
        let (mut corrected, mut count, mut next_provenance, mut openings) = match correction {
            Ok(pass) => pass,
            Err(rejection) => {
                let reason = format!(
                    "Autocorrection was not written to {} because {}.",
                    path.display(),
                    rejection.description()
                );
                return rollback_correction(
                    path, &original, None, reason, config, selection, &plan,
                );
            }
        };
        let mut directive_pass = false;
        if count == 0 && directives_pending {
            directives_pending = false;
            directive_pass = true;
            let correction = corrected_text_with_provenance(
                &mut report,
                mode,
                Correcting::DirectivesOnly,
                provenance.as_deref(),
                next_retained_crlf.as_mut(),
            );
            (corrected, count, next_provenance, openings) = match correction {
                Ok(pass) => pass,
                Err(rejection) => {
                    let reason = format!(
                        "Autocorrection was not written to {} because {}.",
                        path.display(),
                        rejection.description()
                    );
                    return rollback_correction(
                        path, &original, None, reason, config, selection, &plan,
                    );
                }
            };
        }
        if count == 0 {
            let (report, corrected_count) = log.merge_into(report);
            let next_logical = shared
                .as_deref()
                .filter(|state| rewritten || state.logical.is_some())
                .map(|_| text.clone());
            let result = withhold_unparsable(
                restore_crlf(
                    CorrectionOutcome {
                        report,
                        text,
                        corrected_count,
                        rewritten,
                        introduced_non_ascii: provenance
                            .as_ref()
                            .is_some_and(|positions| !positions.is_empty()),
                        infinite_loop: None,
                        rollback: None,
                    },
                    preserve_crlf,
                    retained_crlf.as_ref(),
                    native_newlines,
                    config,
                ),
                &original,
                original_bytes,
                started_valid,
                config,
                selection,
                &plan,
            );
            return remember_stdin_state(
                result,
                shared,
                next_logical,
                retained_crlf.as_ref(),
                preserve_crlf,
            );
        }
        if started_valid
            && !openings.is_empty()
            && crate::rules::capture_parentheses_change_bindings(&report.source, config, &openings)
        {
            let reconstructed = original_bytes.is_none().then(|| {
                output_bytes(&original, declared_label(original.as_bytes()).as_deref())
                    .unwrap_or_else(|_| original.as_bytes().to_vec())
            });
            let original_raw = original_bytes.unwrap_or_else(|| reconstructed.as_deref().unwrap());
            let current_bytes = if pass == 0 {
                Ok(original_raw.to_vec())
            } else {
                // openingは旧reportの座標で照合済み。実byteだけをcurrentの由来で復元する。
                debug_assert!(retained_crlf.as_ref().is_none_or(|state| !state.physical));
                let physical_text = physical_corrected_text(
                    &text,
                    preserve_crlf,
                    retained_crlf.as_ref(),
                    native_newlines,
                    config,
                );
                correction_bytes(
                    &physical_text,
                    original_raw,
                    Some(
                        provenance
                            .as_ref()
                            .is_some_and(|positions| !positions.is_empty()),
                    ),
                )
            };
            if current_bytes.is_ok_and(|bytes| {
                crate::prism::script_is_valid(bytes, config.target_ruby_version())
            }) {
                let reason = format!(
                    "Autocorrection was not written to {} because parentheses would change a regexp capture variable into a method call.",
                    path.display()
                );
                return rollback_correction(
                    path, &original, None, reason, config, selection, &plan,
                );
            }
        }
        log.record_pass(&mut report, directive_pass);
        // SourceBuffer の解析座標は LF、Windows の raw_source は別の view で保持する。
        // 最後は予定物理 bytes へ戻して検証し、元改行の由来と DATA の値を守る。
        normalize_corrected_line_endings(
            &mut corrected,
            &mut next_provenance,
            &mut next_retained_crlf,
        );
        retained_crlf = next_retained_crlf;
        provenance = next_provenance;
        rewritten |= corrected != text;

        // 既出のソースへ戻ったら、補正を交換し続ける循環として起点を報告する。
        let corrected_digest = digest(&corrected);
        let repeated = sources.iter().position(|seen| *seen == corrected_digest);
        if pass + 1 == MAX_CORRECTION_PASSES || repeated.is_some() {
            let loop_start = repeated.unwrap_or_else(|| log.cops_by_pass.len().saturating_sub(1));
            let root_cause = log.root_cause(loop_start, &report.offenses, mode, config, selection);
            let (report, corrected_count) = log.merge_into(report);
            let next_logical = shared
                .as_deref()
                .filter(|state| rewritten || state.logical.is_some())
                .map(|_| corrected.clone());
            let result = withhold_unparsable(
                restore_crlf(
                    CorrectionOutcome {
                        report,
                        text: corrected,
                        corrected_count,
                        rewritten,
                        introduced_non_ascii: provenance
                            .as_ref()
                            .is_some_and(|positions| !positions.is_empty()),
                        infinite_loop: Some(format!(
                            "Infinite loop detected in {} and caused by {root_cause}",
                            path.display()
                        )),
                        rollback: None,
                    },
                    preserve_crlf,
                    retained_crlf.as_ref(),
                    native_newlines,
                    config,
                ),
                &original,
                original_bytes,
                started_valid,
                config,
                selection,
                &plan,
            );
            return remember_stdin_state(
                result,
                shared,
                next_logical,
                retained_crlf.as_ref(),
                preserve_crlf,
            );
        }

        sources.push(corrected_digest);
        text = corrected;
        report = inspect_planned(
            path.clone(),
            text.clone(),
            config,
            selection,
            &plan,
            directives_pending,
            correction_raw_source(
                &text,
                preserve_crlf,
                retained_crlf.as_ref(),
                windows_disk,
                native_newlines,
                config,
            ),
        )?;
    }
    unreachable!("the autocorrect loop always returns before exhausting its passes")
}

pub fn correct_until_stable(
    report: FileReport,
    mode: CorrectMode,
    config: &Config,
    selection: &Selection,
) -> Result<(FileReport, String, usize)> {
    let outcome = correct_file(report, mode, config, selection)?;
    match outcome.infinite_loop {
        Some(message) => bail!(message),
        None => Ok((outcome.report, outcome.text, outcome.corrected_count)),
    }
}

/// 書き込み前に判定できる符号化の拒否を、途中まで書いた可能性のある I/O エラーと区別する。
#[derive(Debug)]
pub(crate) struct CorrectionEncodingError(String);

impl std::fmt::Display for CorrectionEncodingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the correction cannot be written back as {}",
            self.0
        )
    }
}

impl std::error::Error for CorrectionEncodingError {}

/// 修正後のソースを、読み込んだときの符号化で書き戻す。
///
/// 本家の runner は最後に単純な `File.write` を呼ぶため、Shift_JIS のファイルも
/// 修正後は UTF-8 で書き戻され、マジックコメントと実際の符号化が食い違う。
/// 元ファイルを壊さないため、ここは意図的に本家と異なる。理由は
/// `tests/conformance/known_divergences.yml` に記録している。
///
/// 符号化は原本から決める。Lint/OrderedMagicComments が宣言を 1 行目へ移しても、
/// 読み込む時点で宣言が無ければ UTF-8 のバイトを別の符号化へ変換しない。
///
/// 修正結果を元の符号化で表せない場合は `Err` を返し、元ファイルを残す。
fn output_bytes(contents: &str, original_label: Option<&str>) -> Result<Vec<u8>> {
    let Some(label) = original_label else {
        return Ok(contents.as_bytes().to_vec());
    };
    // binary ソースは読み込み時と同じく 1 バイトを 1 文字として書き戻す。
    if is_binary_label(label) {
        return match contents.chars().all(|character| (character as u32) < 0x100) {
            true => Ok(contents.chars().map(|character| character as u8).collect()),
            false => Err(CorrectionEncodingError(label.to_owned()).into()),
        };
    }
    // 646 など WHATWG が持たない別名も含め、Ruby の ASCII 制約を先に判定する。
    if is_seven_bit_label(label) {
        return if contents.is_ascii() {
            Ok(contents.as_bytes().to_vec())
        } else {
            Err(CorrectionEncodingError(label.to_owned()).into())
        };
    }
    let Some(encoding) = encoding_for_ruby_label(label) else {
        return Ok(contents.as_bytes().to_vec());
    };
    if encoding == encoding_rs::UTF_8 {
        return Ok(contents.as_bytes().to_vec());
    }
    let (bytes, _, unmappable) = encoding.encode(contents);
    match unmappable {
        // 表せない文字を encode が代替しても、元のファイルを壊す書き戻しには使わない。
        true => Err(CorrectionEncodingError(label.to_owned()).into()),
        false => Ok(bytes.into_owned()),
    }
}

/// WHATWG と Ruby の符号化表が異なる範囲では、新しい非 ASCII 文字を推測で符号化しない。
/// 原本の再符号化がバイトを変える場合も拒否し、補正していない内容の消失を防ぐ。
pub(crate) fn correction_bytes(
    contents: &str,
    original: &[u8],
    introduced_non_ascii: Option<bool>,
) -> Result<Vec<u8>> {
    let original_label = declared_label(original);
    let after = declared_label(contents.as_bytes());
    // frozen コメント等で宣言が無効な位置へ移った場合、本家と同じ有効な UTF-8 を返す。
    let output_label = original_label
        .as_deref()
        .filter(|_| after.as_deref().is_some_and(is_legacy_label));
    if let Some(label) = output_label.filter(|label| is_legacy_label(label)) {
        let same_encoding = after.as_ref().is_some_and(|after| {
            label.eq_ignore_ascii_case(after) || (is_binary_label(label) && is_binary_label(after))
        });
        if !same_encoding && !contents.is_ascii() {
            return Err(CorrectionEncodingError(label.to_owned()).into());
        }
        let Decoded::Text(decoded) = decoded_bytes(original.to_vec()) else {
            return Err(CorrectionEncodingError(label.to_owned()).into());
        };
        if output_bytes(&decoded, Some(label))? != original {
            return Err(CorrectionEncodingError(label.to_owned()).into());
        }
        // CLI は適用された編集の由来を渡す。編集情報のない公開 API は保守的に判定する。
        let introduced = introduced_non_ascii.unwrap_or_else(|| {
            // 編集範囲が不明な呼び出しでは、非 ASCII の変更を安全と断定できない。
            !contents.is_ascii() && contents != decoded
        });
        if introduced {
            return Err(CorrectionEncodingError(label.to_owned()).into());
        }
    }
    let bytes = output_bytes(contents, output_label)?;
    if matches!(decoded_bytes(bytes.clone()), Decoded::Undecodable(_)) {
        let label = declared_label(&bytes).unwrap_or_else(|| "UTF-8".to_owned());
        return Err(CorrectionEncodingError(label).into());
    }
    Ok(bytes)
}

fn is_legacy_label(label: &str) -> bool {
    is_binary_label(label)
        || is_seven_bit_label(label)
        || encoding_for_ruby_label(label).is_some_and(|encoding| encoding != encoding_rs::UTF_8)
}

/// 原本の符号化を確認し、補正後のソースを同じパスへ書き戻す。
///
/// 本家の File.write は symlink を辿り、hard link の inode も保つ。リンクを
/// temp file の rename で置き換えると別名が古い内容を参照し続けるため、リンク経由では
/// 本家と同じ直接書き込みを使う。通常の単一リンクのファイルは、中断時の切り詰めを
/// 防ぐため一時ファイルから置き換える。どちらも元の permission を保つ。
///
/// 編集の由来が分からない公開 API では、非 UTF-8 の非 ASCII 変更を保守的に拒否する。
/// CLI は適用された編集の由来を持つ write_correction を使う。
pub fn write_corrected(path: &Path, contents: &str) -> Result<()> {
    write_corrected_with_provenance(path, contents, None)
}

pub(crate) fn write_correction(path: &Path, outcome: &CorrectionOutcome) -> Result<()> {
    write_corrected_with_provenance(path, &outcome.text, Some(outcome.introduced_non_ascii))
}

fn write_corrected_with_provenance(
    path: &Path,
    contents: &str,
    introduced_non_ascii: Option<bool>,
) -> Result<()> {
    // メモリ上で全パスを終えてから書くため、ディスクには読み込み時の原本が残っている。
    let original = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let bytes = correction_bytes(contents, &original, introduced_non_ascii)
        .with_context(|| format!("refusing to rewrite {}", path.display()))?;
    // rename の権限は親ディレクトリで決まるため、原本へ書けることを先に確認する。
    // truncate は付けず、拒否されたファイルの本文には触れない。
    OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {} for rewriting", path.display()))?;
    if rename_would_lose_the_file(path)? {
        return write_in_place(path, &bytes);
    }
    #[cfg(windows)]
    let replacement_path = windows_atomic_path(path)
        .with_context(|| format!("failed to resolve {} for replacement", path.display()))?;
    #[cfg(not(windows))]
    let replacement_path = path;
    let parent = replacement_path.parent().unwrap_or(Path::new("."));
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read metadata for {}", path.display()))?;
    let mut temporary = match NamedTempFile::new_in(parent) {
        Ok(temporary) => temporary,
        // ファイルには書けるが親に作成できない場合、本家の File.write は成功する。
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return write_in_place(path, &bytes);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to create temporary file beside {}", path.display())
            });
        }
    };
    #[cfg(unix)]
    if replacement_loses_unix_metadata(path, &metadata, temporary.as_file())? {
        return write_in_place(path, &bytes);
    }
    temporary
        .write_all(&bytes)
        .with_context(|| format!("failed to write corrected contents for {}", path.display()))?;
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("failed to flush corrected contents for {}", path.display()))?;
    temporary
        .as_file()
        .set_permissions(metadata.permissions())
        .with_context(|| format!("failed to preserve permissions for {}", path.display()))?;
    temporary
        .persist(replacement_path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {} atomically", path.display()))?;
    Ok(())
}

/// tempfile の置換は Win32 API へ直接渡すため、一時名と置換先の両方に拡張形式が要る。
/// absolute はファイルを開かず通常パスの . や末尾の空白を解決する。canonicalize で
/// 親の読取権限を追加したり、symlink の表記を変えたりしない。
#[cfg(windows)]
fn windows_atomic_path(path: &Path) -> std::io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Prefix};

    let path = std::path::absolute(path)?;
    let mut extended = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => OsString::from(r"\\?\"),
            Prefix::UNC(..) => {
                let remainder: Vec<u16> = path.as_os_str().encode_wide().skip(2).collect();
                let mut extended = OsString::from(r"\\?\UNC\");
                extended.push(OsString::from_wide(&remainder));
                return Ok(extended.into());
            }
            _ => return Ok(path),
        },
        _ => return Ok(path),
    };
    extended.push(path.as_os_str());
    Ok(extended.into())
}

/// 所有者・属性・ACL を落とす置換を避け、本家の File.write による保持を優先する。
#[cfg(unix)]
fn replacement_loses_unix_metadata(
    path: &Path,
    original: &fs::Metadata,
    replacement: &fs::File,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let replacement = replacement.metadata()?;
    if original.uid() != replacement.uid()
        || original.gid() != replacement.gid()
        // setuid/setgid の扱いも、実際の書き込みを行うカーネルへ任せる。
        || original.mode() & 0o6000 != 0
        || xattr::list(path).map_or(true, |mut attributes| attributes.next().is_some())
    {
        return Ok(true);
    }
    #[cfg(target_os = "macos")]
    if exacl::getfacl(path, None).map_or(true, |entries| !entries.is_empty()) {
        return Ok(true);
    }
    Ok(false)
}

/// rename で symlink や同じファイルの別名を切り離してしまう経路を見分ける。
/// metadata はリンク先を辿るため、リンク自体の判定には symlink_metadata が必要になる。
fn rename_would_lose_the_file(path: &Path) -> Result<bool> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(false);
    };
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        windows_link_count(path).map(|count| count > 1)
    }
    #[cfg(not(windows))]
    {
        Ok(link_count(&metadata) > 1)
    }
}

/// Windows の std のリンク数取得は不安定 API のため、安定した Win32 API を使う。
#[cfg(windows)]
fn windows_link_count(path: &Path) -> Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = fs::File::open(path)
        .with_context(|| format!("failed to read link information for {}", path.display()))?;
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // File が生存する間はハンドルが有効で、出力先は構造体全体を書ける領域を持つ。
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to read link information for {}", path.display()));
    }
    // 成功時だけ Win32 が構造体全体を初期化したことを保証する。
    let information = unsafe { information.assume_init() };
    Ok(u64::from(information.nNumberOfLinks))
}

#[cfg(unix)]
fn link_count(metadata: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(metadata)
}

#[cfg(not(any(unix, windows)))]
fn link_count(_metadata: &fs::Metadata) -> u64 {
    1
}

/// 本家の File.write と同じく、リンク先の inode を保って本文だけを書き換える。
fn write_in_place(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("failed to open {} for rewriting", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write corrected contents for {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush corrected contents for {}", path.display()))?;
    Ok(())
}

pub fn offense_count(reports: &[FileReport], fail_level: Severity) -> usize {
    reports
        .iter()
        .flat_map(|report| &report.offenses)
        .filter(|offense| {
            offense.severity >= fail_level && !offense.corrected && !offense.suppressed
        })
        .count()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{Cached, Decoded, decoded_source, output_bytes};
    use crate::config::Config;
    use crate::diagnostic::{
        CorrectionRejection, Edit, FileReport, InsertionAnchor, Offense, Severity,
    };
    use crate::source::SourceFile;

    use super::{
        CorrectMode, Correcting, ResultCache, Selection, correct_file, corrected_text,
        discover_targets, inspect_files, inspect_source, write_corrected,
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn result_cache_retains_raw_messages_and_first_annotation_after_touch() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            b"AllCops:\n  NewCops: disable\n  UseProjectIndex: false\n  TargetRubyVersion: 3.3\n",
        )
        .unwrap();
        let path = directory.path().join("example.rb");
        fs::write(&path, b"x=1\n").unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/SpaceAroundOperators".to_owned()],
            ..Selection::default()
        };
        let root = directory.path().join("cache");
        let first = ResultCache::new(root.clone(), config.path_base(), &selection, 100)
            .unwrap()
            .with_message_options("_", false);
        first.prepare(1);
        let report = inspect_source(path.clone(), "x=1\n".to_owned(), &config, &selection).unwrap();
        assert_eq!(report.offenses[0].cached_display_cop_names(), None);
        store_as_read(&first, &report, &config);
        let index = first.index_path.clone();
        first.prune();
        drop(first);

        let second = ResultCache::new(root.clone(), config.path_base(), &selection, 100)
            .unwrap()
            .with_message_options("_", true);
        assert_eq!(second.index_path, index);
        second.prepare(1);
        let Some(Cached::Fresh(report)) = second.load(&path, &config) else {
            panic!("expected a cache hit")
        };
        assert_eq!(
            report.offenses[0].message,
            "Surrounding space missing for operator `=`."
        );
        assert_eq!(report.offenses[0].cached_display_cop_names(), Some(false));

        // Oracle の touch-and-body 履歴は mtime だけの変更後も初回の裸 message を再利用する。
        let modified =
            fs::metadata(&path).unwrap().modified().unwrap() + std::time::Duration::from_secs(2);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let configs = crate::config::ConfigStore::new(config.clone(), false, false);
        let reports = super::inspect_files_with_store_cached(
            std::slice::from_ref(&path),
            &configs,
            &selection,
            false,
            Some(&second),
        )
        .unwrap();
        assert_eq!(
            reports[0].offenses[0].message,
            "Surrounding space missing for operator `=`."
        );
        assert_eq!(
            reports[0].offenses[0].cached_display_cop_names(),
            Some(false)
        );
        second.prune();
        drop(second);
        let third = ResultCache::new(root, config.path_base(), &selection, 100)
            .unwrap()
            .with_message_options("_", true);
        third.prepare(1);
        let Some(Cached::Fresh(report)) = third.load(&path, &config) else {
            panic!("expected the refreshed stat")
        };
        assert_eq!(report.offenses[0].cached_display_cop_names(), Some(false));
        assert_eq!(fs::read(path).unwrap(), b"x=1\n");
        assert_eq!(super::RESULT_CACHE_SCHEMA, 5);
    }

    /// One cop's corrections: the cop's name, then an offense per inner slice, then the edits that
    /// offense asks for.
    type CopEdits = (
        &'static str,
        &'static [&'static [(usize, usize, &'static str)]],
    );

    /// [`ResultCache::store`] as a run reaches it, for the tests that drive the cache directly
    /// rather than through [`inspect_files_with_store_cached`]. The stat and the digest both come
    /// from the file on disk, which is what the caller in a real run supplies.
    fn store_as_read(cache: &ResultCache, report: &FileReport, config: &Config) {
        let stat = ResultCache::stat(&fs::metadata(&report.path).unwrap());
        let digest = *blake3::hash(&fs::read(&report.path).unwrap()).as_bytes();
        cache.store(report, stat, &digest, config);
    }

    #[test]
    fn symbol_array_invalid_decode_rolls_back_without_changing_original_diagnostics() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.yml");
        for (literal, style) in [
            (r"%I[\x95 foo bar]", "brackets"),
            (r#"[ :"\x95", :foo, :bar, :baz ]"#, "percent"),
        ] {
            fs::write(&path, format!("AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n  NewCops: disable\nStyle/SymbolArray:\n  EnforcedStyle: {style}\n")).unwrap();
            let config = Config::load(Some(&path), directory.path()).unwrap();
            let selection = Selection {
                only: vec![
                    "Style/SymbolArray".to_owned(),
                    "Layout/SpaceAroundOperators".to_owned(),
                ],
                correcting: true,
                ..Selection::default()
            };
            // 両原本にSymbol値は無い。U+FFFD名の発明と別cop編集を同じ経路で拒否する。
            for ending in ["\n", "\r\n"] {
                let source = format!("{literal}\nx=1\n").replace('\n', ending);
                for mode in [CorrectMode::Safe, CorrectMode::All] {
                    let mut report = inspect_source(
                        directory.path().join("input.rb"),
                        source.clone(),
                        &config,
                        &selection,
                    )
                    .unwrap();
                    let original: Vec<_> = report
                        .offenses
                        .iter()
                        .map(|o| {
                            (
                                o.cop_name,
                                o.message.clone(),
                                o.start,
                                o.end,
                                o.is_correctable(),
                            )
                        })
                        .collect();
                    let (text, count) = corrected_text(&mut report, mode, Correcting::Everything);
                    assert_eq!(text, source, "{style}/{ending:?}/{mode:?}");
                    assert_eq!(count, 0);
                    let outcome = correct_file(report, mode, &config, &selection).unwrap();
                    assert_eq!(outcome.text, source);
                    assert_eq!(outcome.corrected_count, 0);
                    assert!(!outcome.rewritten);
                    assert!(
                        outcome
                            .rollback
                            .as_deref()
                            .is_some_and(|r| r.contains("Style/SymbolArray"))
                    );
                    assert!(outcome.report.offenses.iter().all(|o| !o.corrected));
                    let restored: Vec<_> = outcome
                        .report
                        .offenses
                        .iter()
                        .map(|o| {
                            (
                                o.cop_name,
                                o.message.clone(),
                                o.start,
                                o.end,
                                o.is_correctable(),
                            )
                        })
                        .collect();
                    assert_eq!(restored, original);
                }
            }
        }
    }

    fn word_array_byte_safety_configuration() -> (tempfile::TempDir, Config, Selection) {
        let directory = tempdir().unwrap();
        let path = directory.path().join(".rubocop.yml");
        fs::write(
            &path,
            "AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n  NewCops: disable\n",
        )
        .unwrap();
        let config = Config::load(Some(&path), directory.path()).unwrap();
        let selection = Selection {
            only: vec![
                "Style/WordArray".to_owned(),
                "Layout/SpaceAroundOperators".to_owned(),
            ],
            correcting: true,
            ..Selection::default()
        };
        (directory, config, selection)
    }

    #[test]
    fn word_array_invalid_bytes_never_leave_the_low_level_corrector() {
        let (directory, config, selection) = word_array_byte_safety_configuration();
        // 本家のraw 0x95も既存候補のU+FFFDも書かず、別copの安全な編集ごと保留する。
        for ending in ["\n", "\r\n"] {
            let source = "values = %W[\\x95 foo bar]\nx=1\n".replace('\n', ending);
            for mode in [CorrectMode::Safe, CorrectMode::All] {
                let mut report = inspect_source(
                    directory.path().join("input.rb"),
                    source.clone(),
                    &config,
                    &selection,
                )
                .unwrap();
                assert!(report.offenses.iter().any(|offense| {
                    offense.cop_name == "Style/WordArray"
                        && offense.is_correctable()
                        && offense.corrections.is_empty()
                }));
                let (corrected, count) = corrected_text(&mut report, mode, Correcting::Everything);
                assert_eq!(corrected, source, "{ending:?}/{mode:?}");
                assert_eq!(count, 0);
                assert!(report.offenses.iter().all(|offense| !offense.corrected));
            }
        }
    }

    #[test]
    fn word_array_invalid_bytes_roll_back_the_whole_file_and_original_diagnostics() {
        let (directory, config, selection) = word_array_byte_safety_configuration();
        for ending in ["\n", "\r\n"] {
            let source = "values = %W[\\x95 foo bar]\nx=1\n".replace('\n', ending);
            for mode in [CorrectMode::Safe, CorrectMode::All] {
                let report = inspect_source(
                    directory.path().join("input.rb"),
                    source.clone(),
                    &config,
                    &selection,
                )
                .unwrap();
                let original: Vec<_> = report
                    .offenses
                    .iter()
                    .map(|offense| {
                        (
                            offense.cop_name,
                            offense.message.clone(),
                            serde_json::to_value(offense.location(&report.source)).unwrap(),
                            offense.is_correctable(),
                        )
                    })
                    .collect();
                assert!(original.iter().any(|(cop, ..)| *cop == "Style/WordArray"));
                assert!(
                    original
                        .iter()
                        .any(|(cop, ..)| *cop == "Layout/SpaceAroundOperators")
                );
                let outcome = correct_file(report, mode, &config, &selection).unwrap();
                assert_eq!(outcome.text, source, "{ending:?}/{mode:?}");
                assert_eq!(outcome.corrected_count, 0);
                assert!(!outcome.rewritten);
                assert!(outcome.rollback.as_deref().is_some_and(|reason| {
                    reason.contains("Style/WordArray") && reason.contains("bytes")
                }));
                assert!(
                    outcome
                        .report
                        .offenses
                        .iter()
                        .all(|offense| !offense.corrected)
                );
                let restored: Vec<_> = outcome
                    .report
                    .offenses
                    .iter()
                    .map(|offense| {
                        (
                            offense.cop_name,
                            offense.message.clone(),
                            serde_json::to_value(offense.location(&outcome.report.source)).unwrap(),
                            offense.is_correctable(),
                        )
                    })
                    .collect();
                assert_eq!(restored, original);
            }
        }
    }

    #[test]
    fn word_array_rejection_respects_corrector_compatibility_and_conflicts() {
        let edit = |start, end, replacement: &str| Edit {
            start,
            end,
            replacement: replacement.to_owned(),
            safe: true,
        };
        let offense = |cop: &'static str, edits: Vec<Edit>| {
            Offense::new(cop, Severity::Convention, "test", 0, 0).corrected_by_all(edits)
        };
        // 実Registryの非互換pairと衝突規則を使い、採用されない予定では拒否しない。
        let cases = [
            (
                offense("Layout/SpaceBeforeBlockBraces", vec![edit(0, 1, "A")]),
                offense("Style/SymbolProc", vec![edit(4, 5, "X")]),
                "Abcdef",
            ),
            (
                offense("Layout/SpaceAroundOperators", vec![edit(0, 4, "A")]),
                offense("Style/WordArray", vec![edit(1, 3, "X")]),
                "Aef",
            ),
            (
                offense("Layout/SpaceAroundOperators", vec![edit(5, 6, "F")]),
                offense("Style/WordArray", vec![edit(1, 4, "X"), edit(2, 5, "Y")]),
                "abcdeF",
            ),
        ];
        for (ordinary, blocked, expected) in cases {
            let mut report = FileReport {
                path: "input.rb".into(),
                source: SourceFile::new("input.rb", "abcdef".to_owned()),
                offenses: vec![
                    ordinary,
                    blocked.reject_corrections(CorrectionRejection::WordArrayInvalidBytes),
                ],
            };
            let (corrected, count) =
                corrected_text(&mut report, CorrectMode::All, Correcting::Everything);
            assert_eq!(corrected, expected);
            assert_eq!(count, 1);
        }
        for (mode, correcting) in [
            (CorrectMode::All, Correcting::DirectivesOnly),
            (CorrectMode::None, Correcting::Everything),
        ] {
            let mut report = FileReport {
                path: "input.rb".into(),
                source: SourceFile::new("input.rb", "abcdef".to_owned()),
                offenses: vec![
                    offense("Style/WordArray", vec![edit(0, 1, "X")])
                        .reject_corrections(CorrectionRejection::WordArrayInvalidBytes),
                ],
            };
            let pass =
                super::corrected_text_with_provenance(&mut report, mode, correcting, None, None)
                    .unwrap();
            assert_eq!(pass.0, "abcdef");
            assert_eq!(pass.1, 0);
        }
    }

    #[test]
    fn word_array_disabled_or_unsafe_corrections_leave_other_cops_available() {
        for (setting, prefix, mode) in [
            ("  AutoCorrect: false\n", "", CorrectMode::All),
            ("  SafeAutoCorrect: false\n", "", CorrectMode::Safe),
            ("", "# rubocop:disable Style/WordArray\n", CorrectMode::All),
        ] {
            let (directory, _, selection) = word_array_byte_safety_configuration();
            let path = directory.path().join(".rubocop.yml");
            if !setting.is_empty() {
                let original = fs::read_to_string(&path).unwrap();
                fs::write(&path, format!("{original}Style/WordArray:\n{setting}")).unwrap();
            }
            let config = Config::load(Some(&path), directory.path()).unwrap();
            let source = format!("{prefix}values = %W[\\x95 foo bar]\nx=1\n");
            let report = inspect_source(
                directory.path().join("input.rb"),
                source.clone(),
                &config,
                &selection,
            )
            .unwrap();
            let outcome = correct_file(report, mode, &config, &selection).unwrap();
            assert!(outcome.rollback.is_none());
            assert_eq!(outcome.text, source.replace("x=1", "x = 1"));
            assert!(outcome.report.offenses.iter().any(|offense| {
                offense.cop_name == "Layout/SpaceAroundOperators" && offense.corrected
            }));
            assert!(
                outcome
                    .report
                    .offenses
                    .iter()
                    .all(|offense| { offense.cop_name != "Style/WordArray" || !offense.corrected })
            );
        }
    }

    #[test]
    fn word_array_rejection_follows_nonempty_parent_child_conflicts() {
        for parent_first in [true, false] {
            let replacement = |start, end, text: &str| {
                Offense::new("Style/WordArray", Severity::Convention, "test", start, end)
                    .corrected_by(Edit {
                        start,
                        end,
                        replacement: text.to_owned(),
                        safe: true,
                    })
            };
            let parent = replacement(0, 10, "safe");
            let child = replacement(2, 3, "bad")
                .reject_corrections(CorrectionRejection::WordArrayInvalidBytes);
            let offenses = match parent_first {
                true => vec![parent, child],
                false => vec![child, parent],
            };
            let mut report = FileReport {
                path: "input.rb".into(),
                source: SourceFile::new("input.rb", "abcdefghij".to_owned()),
                offenses,
            };
            // WordArray同様の非empty置換では、親が先なら子が衝突し、逆順なら親が衝突する。
            let (cop, _) = super::build_cop_corrector(&report, &[0, 1], report.source.text());
            let mut replacements = Vec::new();
            cop.ordered_replacements(&mut replacements);
            let final_text: Vec<_> = replacements
                .iter()
                .map(|(start, end, text, _)| (*start, *end, *text))
                .collect();
            assert_eq!(
                final_text,
                if parent_first {
                    vec![(0, 10, "safe")]
                } else {
                    vec![(2, 3, "bad")]
                },
                "{parent_first}"
            );
            let (text, count) =
                corrected_text(&mut report, CorrectMode::All, Correcting::Everything);
            if parent_first {
                assert_eq!(text, "safe", "{parent_first}");
                assert!(count > 0);
            } else {
                assert_eq!(text, "abcdefghij");
                assert_eq!(count, 0);
            }
        }
    }

    #[test]
    fn code_nul_projection_keeps_tree_end_in_original_byte_coordinates() {
        // Semicolonのtoken走査上限に使うprogram末尾は原文のbyte座標である必要がある。
        // コメントのstand-inと文字列のNULが先行しても、最後のcode NULの位置を保つ。
        for source in [
            "a=1;\0kept;\n",
            "名前=1;\0kept;\n",
            "a=1;\n\0kept;\n",
            "# \0 kept\na=1;\0kept;\n",
            "a=\"x\0y\";\0kept;\n",
        ] {
            for source in [source.to_owned(), source.replace('\n', "\r\n")] {
                let semantics = crate::regexp_ast::Semantics::default();
                let projected =
                    crate::nul_bytes::as_ruby_reads_it_configured(&source, semantics).unwrap();
                let tree = crate::parser::parse_configured(&projected, semantics).unwrap();
                let code_nul = source.rfind('\0').unwrap();
                assert_eq!(tree.root_node().byte_range(), 0..code_nul, "{source:?}");
                assert_eq!(projected.len(), code_nul, "{source:?}");
            }
        }
    }

    #[test]
    fn inspection_keeps_the_original_nul_and_the_unparsed_tail() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/SpaceAroundOperators".to_owned()],
            ..Selection::default()
        };
        // SOHを逆変換せず、copが参照する本文と字句器のEOF後も原本のまま保つ。
        for source in [
            "a=1\n# \0\u{1}\0 kept\n",
            "a=1\0unparsed ) tail\n# \u{1}\0\n",
            "a=1\n__END__\n\0\u{1}\0 data\n",
        ] {
            let report = inspect_source(
                directory.path().join("example.rb"),
                source.to_owned(),
                &config,
                &selection,
            )
            .unwrap();
            assert_eq!(report.source.text().as_bytes(), source.as_bytes());
            assert!(
                report
                    .offenses
                    .iter()
                    .any(|offense| offense.cop_name == "Layout/SpaceAroundOperators")
            );
        }
    }

    #[test]
    fn result_cache_round_trips_reports_and_rejects_changed_source() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        let source = "x = 1  \n";
        fs::write(&path, source).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let cache = ResultCache::new(
            directory.path().join("cache"),
            config.path_base(),
            &selection,
            100,
        )
        .unwrap();
        cache.prepare(1);
        let report = inspect_source(path.clone(), source.to_owned(), &config, &selection).unwrap();

        store_as_read(&cache, &report, &config);
        // 置いたままのファイルなので stat が一致し、中身を読まずに報告が返る。
        let Some(Cached::Fresh(cached)) = cache.load(&path, &config) else {
            panic!("an untouched file should be served from its stat alone");
        };

        // 読んでいないのだから本文は無い。位置と行はすべて凍結側から来ている。
        assert!(cached.source.text().is_empty());
        assert_eq!(cached.offenses.len(), 1);
        assert_eq!(cached.offenses[0].cop_name, report.offenses[0].cop_name);
        assert_eq!(cached.offenses[0].message, report.offenses[0].message);
        assert_eq!(
            cached.offenses[0].location(&cached.source).line,
            report.offenses[0].location(&report.source).line
        );
        assert_eq!(
            cached.offenses[0].source_line(&cached.source),
            report.offenses[0].source_line(&report.source)
        );
        assert!(cached.offenses[0].is_correctable());

        // 中身が変われば stat も digest も合わず、報告は使えない。
        fs::write(&path, "x = 1\n").unwrap();
        match cache.load(&path, &config) {
            Some(Cached::Fresh(_)) => panic!("a rewritten file must not pass as fresh"),
            Some(Cached::Stale(cached)) => assert!(!cached.matches("x = 1\n")),
            None => {}
        }
    }

    /// 触っただけで中身が同じなら、stat が動いても報告は生き残らなければならない。
    /// 内容の digest を持たずに stat だけで判定していると、ここでキャッシュを捨ててしまう。
    #[test]
    fn result_cache_survives_a_rewrite_that_changes_nothing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        let source = "x = 1  \n";
        fs::write(&path, source).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let cache = ResultCache::new(
            directory.path().join("cache"),
            config.path_base(),
            &selection,
            100,
        )
        .unwrap();
        cache.prepare(1);
        let report = inspect_source(path.clone(), source.to_owned(), &config, &selection).unwrap();
        store_as_read(&cache, &report, &config);

        fs::write(&path, source).unwrap();

        match cache.load(&path, &config) {
            // 更新時刻の分解能が足りず stat が動かないことはある。それでも結果は同じ。
            Some(Cached::Fresh(cached)) => assert_eq!(cached.offenses.len(), 1),
            Some(Cached::Stale(cached)) => {
                assert!(cached.matches(source));
                let restored = cached.into_report(path.clone(), Some(source)).unwrap();
                assert_eq!(restored.offenses.len(), 1);
                assert_eq!(restored.offenses[0].cop_name, report.offenses[0].cop_name);
            }
            None => panic!("a rewrite that changes nothing must not lose the report"),
        }
    }

    /// `Lint/ScriptPermission` stats the file itself, and `chmod` moves neither the size nor the
    /// modification time nor a single byte of the text. Without the mode in the stat, the report
    /// made before the `chmod` is served as fresh forever, and the digest cannot correct it either
    /// because the bytes really are unchanged.
    #[cfg(unix)]
    #[test]
    fn result_cache_notices_a_permission_change() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        let source = "#!/usr/bin/env ruby\nx = 1\n";
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Lint/ScriptPermission".to_owned()],
            ..Selection::default()
        };
        let cache = ResultCache::new(
            directory.path().join("cache"),
            config.path_base(),
            &selection,
            100,
        )
        .unwrap();
        cache.prepare(1);
        let report = inspect_source(path.clone(), source.to_owned(), &config, &selection).unwrap();
        assert_eq!(report.offenses.len(), 1);
        store_as_read(&cache, &report, &config);

        // 置いたままなら stat が一致し、読まずに返る。
        assert!(matches!(cache.load(&path, &config), Some(Cached::Fresh(_))));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            cache.load(&path, &config).is_none(),
            "実行権が変わった報告は、stale としてでも差し出してはならない"
        );
    }

    /// [`ResultCache::store`] must record the stat its caller took before reading, not whatever the
    /// file measures by the time the report is ready. A rewrite landing in between would otherwise
    /// pair the old report with the new stat, which the next run accepts on the stat alone.
    #[test]
    fn result_cache_records_the_stat_it_was_given() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        let source = "x = 1  \n";
        fs::write(&path, source).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let cache = ResultCache::new(
            directory.path().join("cache"),
            config.path_base(),
            &selection,
            100,
        )
        .unwrap();
        cache.prepare(1);
        let report = inspect_source(path.clone(), source.to_owned(), &config, &selection).unwrap();
        let stat = ResultCache::stat(&fs::metadata(&path).unwrap());
        let digest = *blake3::hash(source.as_bytes()).as_bytes();

        // 読んだ後にファイルが書き換わった状況。store には読んだ時点の stat と digest を渡す。
        fs::write(&path, "x = 1\n").unwrap();
        cache.store(&report, stat, &digest, &config);

        match cache.load(&path, &config) {
            Some(Cached::Fresh(_)) => {
                panic!("読んだ時点の stat を記録していれば、書き換え後のファイルには一致しない")
            }
            // digest も読んだ時点のものなので、新しい本文とは合わず作り直しになる。
            Some(Cached::Stale(cached)) => assert!(!cached.matches("x = 1\n")),
            None => {}
        }
    }

    /// A NUL can make Ruby stop reading before the physical end of the file, so such a file is not
    /// cached at all. The digest is taken over the bytes on disk rather than over the text
    /// `nul_bytes::as_ruby_reads_it` rewrites them into -- the rewrite is what removes the NUL, so
    /// hashing it would both defeat this guard and let two different files share a digest.
    #[test]
    fn result_cache_declines_a_file_holding_a_nul() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        fs::write(&path, "\0").unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection::default();
        let cache = ResultCache::new(
            directory.path().join("cache"),
            config.path_base(),
            &selection,
            100,
        )
        .unwrap();
        cache.prepare(1);
        let configs = super::ConfigStore::new(config.clone(), false, false);
        let paths = vec![path.clone()];

        super::inspect_files_with_store_cached(&paths, &configs, &selection, false, Some(&cache))
            .unwrap();
        assert!(
            cache.load(&path, &config).is_none(),
            "NUL を含むファイルは長さが二通りあるのでキャッシュしない"
        );

        // 空にすると `Lint/EmptyFile` の答えが変わる。前の報告が残っていれば取り違える。
        fs::write(&path, "").unwrap();
        let reports = super::inspect_files_with_store_cached(
            &paths,
            &configs,
            &selection,
            false,
            Some(&cache),
        )
        .unwrap();
        assert!(
            reports[0]
                .offenses
                .iter()
                .any(|offense| offense.cop_name == "Lint/EmptyFile"),
            "空になったファイルは空として報告されなければならない"
        );
    }

    #[test]
    fn result_cache_prunes_to_the_configured_file_limit() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let cache = ResultCache::new(
            directory.path().join("cache"),
            config.path_base(),
            &selection,
            1,
        )
        .unwrap();
        let paths = [
            directory.path().join("first.rb"),
            directory.path().join("second.rb"),
        ];
        cache.prepare(paths.len());
        for path in &paths {
            // 保存側が更新時刻と大きさを記録するので、報告の相手は実在していなければならない。
            fs::write(path, "x = 1  \n").unwrap();
            let report =
                inspect_source(path.clone(), "x = 1  \n".to_owned(), &config, &selection).unwrap();
            store_as_read(&cache, &report, &config);
        }

        cache.prune();

        assert_eq!(
            paths
                .iter()
                .filter(|path| cache.load(path, &config).is_some())
                .count(),
            1
        );
    }

    #[test]
    fn result_cache_persists_and_reloads_one_index() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("cache");
        let path = directory.path().join("example.rb");
        let source = "x = 1  \n";
        fs::write(&path, source).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };

        let cache = ResultCache::new(root.clone(), config.path_base(), &selection, 100).unwrap();
        cache.prepare(1);
        let report = inspect_source(path.clone(), source.to_owned(), &config, &selection).unwrap();
        store_as_read(&cache, &report, &config);
        cache.prune();
        assert!(cache.index_path.is_file());
        drop(cache);

        let reloaded = ResultCache::new(root, config.path_base(), &selection, 100).unwrap();
        reloaded.prepare(1);
        let Some(Cached::Fresh(report)) = reloaded.load(&path, &config) else {
            panic!("a report should survive an index reload");
        };
        assert_eq!(report.offenses.len(), 1);
    }

    #[test]
    fn result_cache_identity_is_scoped_to_the_project_path_base() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("cache");
        let first_project = directory.path().join("first");
        let second_project = directory.path().join("second");
        fs::create_dir_all(&first_project).unwrap();
        fs::create_dir_all(&second_project).unwrap();
        let first_config = Config::load(None, &first_project).unwrap();
        let second_config = Config::load(None, &second_project).unwrap();
        let selection = Selection::default();

        let first =
            ResultCache::new(root.clone(), first_config.path_base(), &selection, 100).unwrap();
        let second = ResultCache::new(root, second_config.path_base(), &selection, 100).unwrap();

        assert_ne!(first.identity, second.identity);
        assert_ne!(first.index_path, second.index_path);
    }

    #[test]
    fn oversized_result_cache_index_is_bypassed_for_a_narrow_run() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("cache");
        let path = directory.path().join("example.rb");
        let source = "x = 1\n";
        fs::write(&path, source).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection::default();
        let cache = ResultCache::new(root.clone(), config.path_base(), &selection, 100).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::File::create(&cache.index_path)
            .unwrap()
            .set_len(super::RESULT_CACHE_INDEX_BYTES_PER_TARGET + 1)
            .unwrap();
        let original_len = fs::metadata(&cache.index_path).unwrap().len();

        cache.prepare(1);
        assert_eq!(cache.prepared.get(), Some(&false));
        assert!(cache.load(&path, &config).is_none());
        store_as_read(
            &cache,
            &FileReport {
                path,
                source: SourceFile::new(directory.path().join("example.rb"), source.to_owned()),
                offenses: Vec::new(),
            },
            &config,
        );
        cache.prune();
        assert_eq!(fs::metadata(&cache.index_path).unwrap().len(), original_len);

        let wider = ResultCache::new(root, config.path_base(), &selection, 100).unwrap();
        wider.prepare(2);
        assert_eq!(wider.prepared.get(), Some(&true));
    }

    #[test]
    fn concurrent_result_caches_merge_their_updates() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("cache");
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let first = ResultCache::new(root.clone(), config.path_base(), &selection, 100).unwrap();
        let second = ResultCache::new(root.clone(), config.path_base(), &selection, 100).unwrap();
        first.prepare(2);
        second.prepare(2);
        let paths = [
            directory.path().join("first.rb"),
            directory.path().join("second.rb"),
        ];
        for (cache, path) in [(&first, &paths[0]), (&second, &paths[1])] {
            fs::write(path, "x = 1  \n").unwrap();
            let report =
                inspect_source(path.clone(), "x = 1  \n".to_owned(), &config, &selection).unwrap();
            store_as_read(cache, &report, &config);
        }

        first.prune();
        second.prune();
        let reloaded = ResultCache::new(root, config.path_base(), &selection, 100).unwrap();
        reloaded.prepare(2);
        assert!(
            paths
                .iter()
                .all(|path| reloaded.load(path, &config).is_some())
        );
    }

    #[test]
    fn malformed_result_cache_index_is_rebuilt_without_becoming_a_hit() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("cache");
        let path = directory.path().join("example.rb");
        let source = "x = 1  \n";
        fs::write(&path, source).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };

        let empty = ResultCache::new(root.clone(), config.path_base(), &selection, 100).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::write(&empty.index_path, b"{unfinished").unwrap();
        drop(empty);
        let rebuilt = ResultCache::new(root.clone(), config.path_base(), &selection, 100).unwrap();
        rebuilt.prepare(1);
        assert!(rebuilt.load(&path, &config).is_none());
        let report = inspect_source(path.clone(), source.to_owned(), &config, &selection).unwrap();
        store_as_read(&rebuilt, &report, &config);
        rebuilt.prune();
        drop(rebuilt);

        let reloaded = ResultCache::new(root, config.path_base(), &selection, 100).unwrap();
        reloaded.prepare(1);
        assert!(matches!(
            reloaded.load(&path, &config),
            Some(Cached::Fresh(_))
        ));
    }

    #[test]
    fn result_cache_file_limit_is_shared_across_session_indexes() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("cache");
        let config = Config::load(None, directory.path()).unwrap();
        let path = directory.path().join("example.rb");
        let source = "x = 1  \n";
        fs::write(&path, source).unwrap();
        let first_selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let second_selection = Selection {
            only: vec!["Layout/TrailingEmptyLines".to_owned()],
            ..Selection::default()
        };

        let first =
            ResultCache::new(root.clone(), config.path_base(), &first_selection, 1).unwrap();
        first.prepare(1);
        let report =
            inspect_source(path.clone(), source.to_owned(), &config, &first_selection).unwrap();
        store_as_read(&first, &report, &config);
        first.prune();
        drop(first);

        let second =
            ResultCache::new(root.clone(), config.path_base(), &second_selection, 1).unwrap();
        second.prepare(1);
        let report =
            inspect_source(path.clone(), source.to_owned(), &config, &second_selection).unwrap();
        store_as_read(&second, &report, &config);
        second.prune();
        drop(second);

        let first =
            ResultCache::new(root.clone(), config.path_base(), &first_selection, 1).unwrap();
        let second = ResultCache::new(root, config.path_base(), &second_selection, 1).unwrap();
        first.prepare(1);
        second.prepare(1);
        assert!(first.load(&path, &config).is_none());
        assert!(second.load(&path, &config).is_some());
    }

    #[test]
    fn result_cache_root_has_a_size_budget_across_session_indexes() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("cache");
        fs::create_dir_all(&root).unwrap();
        let old = root.join(format!(
            "{}{}.index",
            super::RESULT_CACHE_INDEX_PREFIX,
            "a".repeat(64)
        ));
        let current = root.join(format!(
            "{}{}.index",
            super::RESULT_CACHE_INDEX_PREFIX,
            "b".repeat(64)
        ));
        fs::File::create(&old)
            .unwrap()
            .set_len(super::RESULT_CACHE_ROOT_MAX_BYTES)
            .unwrap();
        fs::write(&current, b"{}").unwrap();
        for path in [&old, &current] {
            let summary = super::CacheIndexSummary {
                schema: super::RESULT_CACHE_INDEX_SUMMARY_SCHEMA,
                entries: 1,
            };
            fs::write(
                super::cache_index_summary_path(path),
                serde_json::to_vec(&summary).unwrap(),
            )
            .unwrap();
        }

        super::evict_old_cache_indexes(&root, &current, usize::MAX);

        assert!(!old.exists());
        assert!(!super::cache_index_summary_path(&old).exists());
        assert!(current.exists());
    }

    #[test]
    fn indentation_and_trailing_whitespace_merge_in_the_same_pass() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec![
                "Layout/IndentationWidth".to_owned(),
                "Layout/TrailingWhitespace".to_owned(),
            ],
            correcting: true,
            ..Selection::default()
        };
        let mut report = inspect_source(
            directory.path().join("example.rb"),
            "  foo do\nx  \n  end\n".to_owned(),
            &config,
            &selection,
        )
        .unwrap();
        // 本家 Runner#inspect_iteration の最初の結果。最終結果だけでは見送りを見抜けない。
        let (text, _) = corrected_text(&mut report, CorrectMode::Safe, Correcting::Everything);
        assert_eq!(text, "  foo do\n    x\n  end\n");
    }

    #[test]
    fn physical_regexp_encoding_uses_exact_ruby_aliases() {
        use sonicop_onigmo_compat::Encoding;
        // 実 Ruby 4.0.8 の Encoding.find で index を比較。WHATWG の UTF8 は Ruby の別名ではない。
        for (label, expected) in [
            ("UTF-8", Some(Encoding::Utf8)),
            ("CP65001", Some(Encoding::Utf8)),
            ("EUCJP", Some(Encoding::EucJp)),
            ("SJIS", Some(Encoding::Windows31J)),
            ("PCK", Some(Encoding::Windows31J)),
            ("CP932", Some(Encoding::Windows31J)),
            ("csWindows31J", Some(Encoding::Windows31J)),
            ("UTF8", None),
            ("Shift_JIS", None),
            ("EUCJP-MS", None),
        ] {
            assert_eq!(
                super::regexp_source_encoding(&format!("# encoding: {label}\n")),
                expected,
                "{label}",
            );
        }
    }

    #[test]
    fn capture_parentheses_follow_applied_edits_across_a_spacing_clash() {
        let directory = tempdir().unwrap();
        std::fs::write(
            directory.path().join(".rubocop.yml"),
            "AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n",
        )
        .unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec![
                "Layout/SpaceBeforeFirstArg".to_owned(),
                "Lint/AmbiguousOperator".to_owned(),
            ],
            correcting: true,
            ..Selection::default()
        };
        let original = "# encoding: EUC-JP\n/(?<あ>x)/e =~ 'x'\nあ    +''\n";
        let mut report = inspect_source(
            directory.path().join("example.rb"),
            original.to_owned(),
            &config,
            &selection,
        )
        .unwrap();
        let (first, _, _, openings) = super::corrected_text_with_provenance(
            &mut report,
            CorrectMode::Safe,
            Correcting::Everything,
            None,
            None,
        )
        .unwrap();
        // 本家の corrected は衝突で捨てた括弧にも立つ。安全判定は適用した Action だけを見る。
        assert!(
            report.offenses.iter().any(|offense| {
                offense.cop_name == "Lint/AmbiguousOperator" && offense.corrected
            })
        );
        assert_eq!(first, original.replace("あ    +", "あ +"));
        assert!(openings.is_empty());
        let mut report = inspect_source(
            directory.path().join("example.rb"),
            first,
            &config,
            &selection,
        )
        .unwrap();
        let (_, _, _, openings) = super::corrected_text_with_provenance(
            &mut report,
            CorrectMode::Safe,
            Correcting::Everything,
            None,
            None,
        )
        .unwrap();
        assert_eq!(openings.len(), 1);
        assert!(crate::rules::capture_parentheses_change_bindings(
            &report.source,
            &config,
            &openings,
        ));
    }

    #[test]
    fn autocorrect_support_covers_the_complete_registered_cop_set() {
        let support: Vec<(String, bool)> =
            serde_json::from_str(include_str!("../config/autocorrect_support.json")).unwrap();
        let names: std::collections::BTreeSet<_> =
            crate::rules::rules().map(|rule| rule.name).collect();
        assert_eq!(support.len(), names.len());
        assert_eq!(
            support
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            names
        );
    }

    #[test]
    fn a_loop_names_uncorrected_cops_only_in_its_last_iteration() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection::default();
        let mut report = FileReport {
            path: "example.rb".into(),
            source: SourceFile::new("example.rb", "x\n".to_owned()),
            offenses: [
                ("Layout/IndentationWidth", true),
                ("Metrics/BlockNesting", false),
                ("Layout/BlockAlignment", true),
                ("Layout/LineLength", false),
                ("Layout/EmptyLines", true),
                ("Layout/EmptyLinesAroundModuleBody", true),
            ]
            .into_iter()
            .map(|(cop, corrected)| {
                let mut offense = Offense::new(cop, Severity::Convention, "test", 0, 1);
                offense.corrected = corrected;
                offense
            })
            .collect(),
        };
        let mut log = super::CorrectionLog::default();
        log.record_pass(&mut report, false);
        log.record_pass(&mut report, false);
        // 本家は cop の登録順。最後だけ、補正機能を持つ LineLength の未補正 offense も残る。
        assert_eq!(
            log.root_cause(0, &report.offenses, CorrectMode::All, &config, &selection),
            "Layout/BlockAlignment, Layout/EmptyLinesAroundModuleBody, Layout/EmptyLines, Layout/IndentationWidth -> Layout/BlockAlignment, Layout/EmptyLinesAroundModuleBody, Layout/EmptyLines, Layout/IndentationWidth, Layout/LineLength"
        );
        fs::write(
            directory.path().join(".rubocop.yml"),
            "Layout/LineLength:\n  AutoCorrect: false\n",
        )
        .unwrap();
        let disabled = Config::load(None, directory.path()).unwrap();
        assert_eq!(
            log.root_cause(1, &report.offenses, CorrectMode::All, &disabled, &selection),
            "Layout/BlockAlignment, Layout/EmptyLinesAroundModuleBody, Layout/EmptyLines, Layout/IndentationWidth"
        );
    }

    /// Runs `corrected_text` over synthetic offenses, so that the composition rules can be pinned
    /// against `Parser::Source::TreeRewriter` without a cop in the way. The cops are named so that
    /// the merge order matches the order they are listed in.
    fn composed(source: &str, cops: &[CopEdits]) -> String {
        const NAMES: [&str; 6] = [
            "Layout/A", "Layout/B", "Layout/C", "Layout/D", "Layout/E", "Layout/F",
        ];
        let offenses = cops
            .iter()
            .enumerate()
            .flat_map(|(position, (cop, offenses))| {
                offenses.iter().map(move |edits| {
                    let name = match *cop {
                        "" => NAMES[position],
                        named => named,
                    };
                    Offense::new(name, Severity::Convention, "test", 0, 0).corrected_by_all(
                        edits.iter().map(|(start, end, replacement)| Edit {
                            start: *start,
                            end: *end,
                            replacement: (*replacement).to_owned(),
                            safe: true,
                        }),
                    )
                })
            })
            .collect();
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", source.to_owned()),
            offenses,
        };
        corrected_text(&mut report, CorrectMode::All, Correcting::Everything).0
    }

    #[test]
    fn disjoint_corrections_keep_a_clashing_offense_atomic() {
        let source = "a\n".repeat(2048);
        // 本家の Corrector に各 a の置換を積んだ実出力は、全行 b になる。
        let mut offenses: Vec<_> = (0..2048)
            .map(|line| {
                let start = line * 2;
                Offense::new("Layout/A", Severity::Convention, "test", start, start + 1)
                    .corrected_by_all([Edit {
                        start,
                        end: start + 1,
                        replacement: "b".to_owned(),
                        safe: true,
                    }])
            })
            .collect();
        // 新しい範囲の編集を先に渡しても、同じ offense の衝突があれば両方を捨てる。
        offenses.push(
            Offense::new("Layout/A", Severity::Convention, "test", 0, source.len())
                .corrected_by_all([
                    Edit {
                        start: source.len() - 1,
                        end: source.len(),
                        replacement: "*".to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: 0,
                        end: 1,
                        replacement: "c".to_owned(),
                        safe: true,
                    },
                ]),
        );
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", source),
            offenses,
        };
        let (corrected, applied) =
            corrected_text(&mut report, CorrectMode::All, Correcting::Everything);
        assert_eq!(corrected, "b\n".repeat(2048));
        assert_eq!(applied, 2048);
        assert!(
            report.offenses[..2048]
                .iter()
                .all(|offense| offense.corrected)
        );
        assert!(!report.offenses[2048].corrected);
    }

    #[test]
    fn appending_disjoint_corrections_preserves_wrapping_and_empty_ranges() {
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", "abc".to_owned()),
            offenses: vec![
                Offense::new("Layout/A", Severity::Convention, "test", 0, 1).corrected_by(Edit {
                    start: 0,
                    end: 1,
                    replacement: "A".to_owned(),
                    safe: true,
                }),
                Offense::new("Layout/A", Severity::Convention, "test", 2, 3).corrected_by(Edit {
                    start: 2,
                    end: 2,
                    replacement: "(".to_owned(),
                    safe: true,
                }),
                Offense::new("Layout/A", Severity::Convention, "test", 2, 3).corrected_by(Edit {
                    start: 2,
                    end: 2,
                    replacement: ")".to_owned(),
                    safe: true,
                }),
                Offense::new("Layout/A", Severity::Convention, "test", 1, 3).corrected_by_all([
                    Edit {
                        start: 1,
                        end: 1,
                        replacement: "[".to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: 3,
                        end: 3,
                        replacement: "]".to_owned(),
                        safe: true,
                    },
                ]),
            ],
        };
        // RuboCop 1.89.0 の Corrector に同じアンカーを渡して得た順序を固定する。
        let (corrected, applied) =
            corrected_text(&mut report, CorrectMode::All, Correcting::Everything);
        assert_eq!(corrected, "A[b)(c]");
        assert_eq!(applied, 4);
        assert!(report.offenses.iter().all(|offense| offense.corrected));
        assert_eq!(
            composed("abc", &[("", &[&[(3, 3, "x")], &[(3, 3, "y")]])]),
            "abcyx"
        );
    }

    #[test]
    fn merging_disjoint_batches_matches_the_original_hierarchy() {
        use super::Action;

        let action = |begin, end, text: &str| Action {
            begin_pos: begin,
            end_pos: end,
            insert_before: String::new(),
            insert_after: String::new(),
            replacement: Some(text.to_owned()),
            characters_from: None,
            children: Vec::new(),
        };
        // 置換・削除・空範囲の挿入・入れ子を持つアンカーを、旧階層構築と比較する。
        for seed in 0..96 {
            let mut original = Action::root();
            let mut incoming = Vec::new();
            for position in 0..24 {
                let begin = position * 8;
                let mut child = action(begin, begin + 1, "x");
                match (position + seed) % 4 {
                    0 => child.replacement = Some(String::new()),
                    1 => {
                        child.end_pos = begin;
                        child.replacement = None;
                        child.insert_before = "!".to_owned();
                    }
                    2 => {
                        child.end_pos = begin + 3;
                        child.replacement = None;
                        child.insert_before = "[".to_owned();
                        child.insert_after = "]".to_owned();
                        child.children = vec![action(begin + 1, begin + 2, "y")];
                    }
                    _ => {}
                }
                if (position + seed) % 3 == 0 {
                    incoming.push(child);
                } else {
                    original = original.place_in_hierarchy(&child).unwrap();
                }
            }
            let mut expected = original.clone();
            for child in &incoming {
                expected = expected.place_in_hierarchy(child).unwrap();
            }
            assert!(original.merge_disjoint_children(&incoming));
            assert_eq!(original, expected);
        }

        let mut parent = Action::root();
        parent.begin_pos = 2;
        parent.end_pos = 20;
        parent.children = vec![action(4, 5, "a"), action(5, 6, "b")];
        let endpoints = [action(2, 3, "c"), action(19, 20, "d")];
        let mut expected = parent.clone();
        for child in &endpoints {
            expected = expected.place_in_hierarchy(child).unwrap();
        }
        assert!(parent.merge_disjoint_children(&endpoints));
        assert_eq!(parent, expected);

        for incoming in [
            vec![action(3, 4, "touch")],
            vec![action(4, 5, "clash")],
            vec![action(3, 7, "nested")],
            vec![action(12, 13, "later"), action(9, 10, "earlier")],
            vec![action(21, 22, "outside")],
            vec![action(2, 20, "parent")],
        ] {
            let before = parent.clone();
            assert!(!parent.merge_disjoint_children(&incoming));
            assert_eq!(parent, before);
        }
        parent.replacement = Some(String::new());
        let before = parent.clone();
        assert!(!parent.merge_disjoint_children(&[action(8, 9, "kept")]));
        assert_eq!(parent, before);
    }

    #[test]
    fn interleaved_cop_batches_keep_a_late_clash_atomic() {
        let source = "a b\n".repeat(2048);
        let mut offenses = Vec::new();
        for (cop, offset, replacement) in [("Layout/A", 0, "A"), ("Layout/B", 2, "B")] {
            for line in 0..2048 {
                let start = line * 4 + offset;
                offenses.push(
                    Offense::new(cop, Severity::Convention, "test", start, start + 1).corrected_by(
                        Edit {
                            start,
                            end: start + 1,
                            replacement: replacement.to_owned(),
                            safe: true,
                        },
                    ),
                );
            }
        }
        offenses.push(
            Offense::new("Layout/C", Severity::Convention, "test", 0, source.len())
                .corrected_by_all([
                    Edit {
                        start: 0,
                        end: 1,
                        replacement: "clash".to_owned(),
                        safe: true,
                    },
                    Edit {
                        start: source.len() - 1,
                        end: source.len(),
                        replacement: "must not land".to_owned(),
                        safe: true,
                    },
                ]),
        );
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", source),
            offenses,
        };
        let (corrected, applied) =
            corrected_text(&mut report, CorrectMode::All, Correcting::Everything);
        assert_eq!(corrected, "A B\n".repeat(2048));
        assert_eq!(applied, 4096);
        // 本家はチームの衝突で捨てた cop も corrected のまま報告する。
        assert!(report.offenses.iter().all(|offense| offense.corrected));
    }

    #[test]
    fn current_line_endings_restore_copied_bytes_without_using_the_next_pass_map() {
        for original in [
            "# あ\r\nv\n",
            "# あ\r\r\nv\n",
            "# あ\r\nv\r\n",
            "# あ\r\r\r\nv\r\n",
        ] {
            let pure = super::uses_crlf_only(original);
            let mut state = Some(super::LineEndingProvenance::new(original));
            if pure {
                state.as_mut().unwrap().hidden_cr.clear();
            }
            let mut text = original.to_owned();
            for pass in 1..=2 {
                let mut next = state.clone();
                let edit = super::Action::from_edit(
                    &Edit {
                        start: 0,
                        end: 0,
                        replacement: "p\n".to_owned(),
                        safe: true,
                    },
                    (0, 0),
                );
                let (mut corrected, mut introduced) =
                    edit.rewrite_with_line_endings(&text, None, next.as_mut());
                // currentとnextの位置がずれる編集でも、旧textの復元へnext表を渡さない。
                if pass > 1 {
                    let expected = format!("{}{original}", if pure { "p\r\n" } else { "p\n" });
                    assert_eq!(
                        super::restored_line_endings(&text, pure, state.as_ref()),
                        expected,
                    );
                }
                super::normalize_corrected_line_endings(&mut corrected, &mut introduced, &mut next);
                state = next;
                text = corrected;
                let before = state.as_ref().map(|state| {
                    (
                        state.hidden_cr.clone(),
                        state.visible_cr.clone(),
                        state.physical,
                    )
                });
                let expected = format!(
                    "{}{original}",
                    if pure { "p\r\n" } else { "p\n" }.repeat(pass),
                );
                assert_eq!(
                    super::restored_line_endings(&text, pure, state.as_ref()),
                    expected,
                );
                assert_eq!(
                    state.as_ref().map(|state| {
                        (
                            state.hidden_cr.clone(),
                            state.visible_cr.clone(),
                            state.physical,
                        )
                    }),
                    before,
                );
            }
        }
    }

    #[test]
    fn pure_crlf_restores_original_bare_cr_at_pass_and_writeback_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        for original in ["x\r\r\ny\r\n", "x\r\r\r\ny\r\n", "x\rZ\r\ny\r"] {
            for passes in [1, 2] {
                let mut retained = super::LineEndingProvenance::new(original);
                retained.hidden_cr.clear();
                let mut retained = Some(retained);
                let mut text = original.to_owned();
                let mut introduced = None;
                for _ in 0..passes {
                    let action = super::Action::from_edit(
                        &Edit {
                            start: 0,
                            end: 0,
                            replacement: "a\n".to_owned(),
                            safe: true,
                        },
                        (0, 0),
                    );
                    (text, introduced) = action.rewrite_with_line_endings(
                        &text,
                        introduced.as_deref(),
                        retained.as_mut(),
                    );
                    super::normalize_corrected_line_endings(
                        &mut text,
                        &mut introduced,
                        &mut retained,
                    );
                }
                let outcome = super::CorrectionOutcome {
                    report: FileReport {
                        path: "test.rb".into(),
                        source: SourceFile::new("test.rb", text.clone()),
                        offenses: Vec::new(),
                    },
                    text,
                    corrected_count: passes,
                    rewritten: true,
                    introduced_non_ascii: false,
                    infinite_loop: None,
                    rollback: None,
                };
                let actual = super::restore_crlf(outcome, true, retained.as_ref(), false, &config);
                assert_eq!(actual.text, format!("{}{original}", "a\r\n".repeat(passes)));
            }
        }
    }

    #[test]
    fn pure_crlf_keeps_only_surviving_original_bare_cr_without_pair_double_counting() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        // 置換の同じCRへ由来を渡さず、元pairのLFだけの置換でbare CRまで消さない。
        for (start, end, replacement, expected) in [
            (1, 2, "", "x\r\ny\r\n"),
            (1, 2, "\r", "x\r\ny\r\n"),
            (3, 4, "", "x\r\ry\r\n"),
            (3, 4, "\n", "x\r\r\ny\r\n"),
            (0, 0, "\n", "\r\nx\r\r\ny\r\n"),
        ] {
            let original = "x\r\r\ny\r\n";
            let mut retained = super::LineEndingProvenance::new(original);
            retained.hidden_cr.clear();
            let action = super::Action::from_edit(
                &Edit {
                    start,
                    end,
                    replacement: replacement.to_owned(),
                    safe: true,
                },
                (start, end),
            );
            let (mut text, mut introduced) =
                action.rewrite_with_line_endings(original, None, Some(&mut retained));
            let mut retained = Some(retained);
            super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
            let outcome = super::CorrectionOutcome {
                report: FileReport {
                    path: "test.rb".into(),
                    source: SourceFile::new("test.rb", text.clone()),
                    offenses: Vec::new(),
                },
                text,
                corrected_count: 1,
                rewritten: true,
                introduced_non_ascii: false,
                infinite_loop: None,
                rollback: None,
            };
            assert_eq!(
                super::restore_crlf(outcome, true, retained.as_ref(), false, &config).text,
                expected
            );
        }
    }

    #[test]
    fn copied_crlf_view_distinguishes_visible_cr_from_hidden_markers() {
        // 最初の正規化後のx\r\nは、原本x\r\r\nのbare CRと隠したpairを表す。
        // 可視CRだけの削除は隠したCRを消さず、LFの置換は古いhidden markerを消す。
        for (start, end, replacement, expected) in [
            (1, 2, "", "x\r\n"),
            (2, 3, "", "x\r"),
            (2, 3, "\n", "x\r\n"),
            (1, 2, "\r", "x\r\n"),
            (2, 2, "_", "x\r_\r\n"),
        ] {
            let mut retained = super::LineEndingProvenance {
                hidden_cr: vec![2],
                visible_cr: vec![1],
                physical: false,
            };
            let action = super::Action::from_edit(
                &Edit {
                    start,
                    end,
                    replacement: replacement.to_owned(),
                    safe: true,
                },
                (start, end),
            );
            let (mut text, mut introduced) =
                action.rewrite_with_line_endings("x\r\n", None, Some(&mut retained));
            let mut retained = Some(retained);
            super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
            assert_eq!(
                super::restore_copied_crlf(&text, &retained.as_ref().unwrap().hidden_cr),
                expected,
                "{start}..{end} / {replacement:?}"
            );
        }
        // 同じLFのmarkerは複数あってよいが、そのLFを置換したら全部脱落する。
        for (start, end, replacement, expected) in [
            (0, 0, "_", "_x\r\r\n"),
            (1, 2, "\n", "x\n"),
            (1, 2, "", "x"),
        ] {
            let mut retained = super::LineEndingProvenance {
                hidden_cr: vec![1, 1],
                visible_cr: Vec::new(),
                physical: false,
            };
            let action = super::Action::from_edit(
                &Edit {
                    start,
                    end,
                    replacement: replacement.to_owned(),
                    safe: true,
                },
                (start, end),
            );
            let (mut text, mut introduced) =
                action.rewrite_with_line_endings("x\n", None, Some(&mut retained));
            let mut retained = Some(retained);
            super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
            assert_eq!(
                super::restore_copied_crlf(&text, &retained.as_ref().unwrap().hidden_cr),
                expected
            );
        }
    }

    #[test]
    fn copied_bare_cr_can_become_crlf_after_inserting_or_exposing_lf() {
        for (original, first_start, first_end, first_replacement) in
            [("x\r", 2, 2, "\n"), ("x\r_\n", 2, 3, "")]
        {
            let mut retained = Some(super::LineEndingProvenance::new(original));
            let mut text = original.to_owned();
            let mut introduced = None;
            for (start, end, replacement) in
                [(first_start, first_end, first_replacement), (0, 0, "_\n")]
            {
                let action = super::Action::from_edit(
                    &Edit {
                        start,
                        end,
                        replacement: replacement.to_owned(),
                        safe: true,
                    },
                    (start, end),
                );
                (text, introduced) = action.rewrite_with_line_endings(
                    &text,
                    introduced.as_deref(),
                    retained.as_mut(),
                );
                super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
            }
            assert_eq!(
                super::restore_copied_crlf(&text, &retained.as_ref().unwrap().hidden_cr),
                "_\nx\r\n"
            );
        }
    }

    #[test]
    fn copied_crlf_keeps_original_bare_cr_across_two_normalizations() {
        let original = "x\r\r\ny\n";
        let mut text = original.to_owned();
        let mut retained = Some(super::LineEndingProvenance::new(original));
        let mut introduced = None;
        for prefix in ["a\n", "b\n"] {
            let action = super::Action::from_edit(
                &Edit {
                    start: 0,
                    end: 0,
                    replacement: prefix.to_owned(),
                    safe: true,
                },
                (0, 0),
            );
            (text, introduced) =
                action.rewrite_with_line_endings(&text, introduced.as_deref(), retained.as_mut());
            super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
        }
        assert_eq!(
            super::restore_copied_crlf(&text, &retained.as_ref().unwrap().hidden_cr),
            format!("b\na\n{original}")
        );
    }

    #[test]
    fn copied_crlf_follows_only_whole_surviving_pairs() {
        let source = "a\r\nb\n";
        // CR/LF片側の削除・置換、間への挿入は元pairの由来を残さない。
        // 一方、直前への挿入や後続行の変更はcopyされたpairを保つ。
        for (start, end, replacement, expected) in [
            (0, 0, "#\n", "#\na\r\nb\n"),
            (1, 2, "", "a\nb\n"),
            (2, 3, "", "a\rb\n"),
            (2, 3, "\n\n", "a\n\nb\n"),
            (2, 3, "\n", "a\nb\n"),
            (2, 2, "x", "a\rx\nb\n"),
            (1, 1, "x", "ax\r\nb\n"),
            (0, 3, "x\n", "x\nb\n"),
            (3, 4, "x", "a\r\nx\n"),
            (5, 5, "c\n", "a\r\nb\nc\n"),
        ] {
            let action = super::Action::from_edit(
                &Edit {
                    start,
                    end,
                    replacement: replacement.to_owned(),
                    safe: true,
                },
                (start, end),
            );
            let mut retained = super::LineEndingProvenance::new(source);
            let (mut text, mut introduced) =
                action.rewrite_with_line_endings(source, None, Some(&mut retained));
            let mut retained = Some(retained);
            super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
            assert_eq!(
                super::restore_copied_crlf(&text, &retained.as_ref().unwrap().hidden_cr),
                expected,
                "{start}..{end} / {replacement:?}"
            );
        }
    }

    #[test]
    fn copied_crlf_and_legacy_positions_survive_multiple_lf_passes() {
        let mut retained = super::LineEndingProvenance::new("a\r\né\n");
        let action = super::Action::from_edit(
            &Edit {
                start: 0,
                end: 0,
                replacement: "€\n".to_owned(),
                safe: true,
            },
            (0, 0),
        );
        let (mut text, mut introduced) =
            action.rewrite_with_line_endings("a\r\né\n", Some(&[]), Some(&mut retained));
        assert_eq!(introduced, Some(vec![0]));
        let mut retained = Some(retained);
        super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
        assert_eq!(text, "€\na\né\n");
        assert_eq!(retained.as_ref().unwrap().hidden_cr, vec![5]);
        assert_eq!(introduced, Some(vec![0]));
        let action = super::Action::from_edit(
            &Edit {
                start: 0,
                end: 4,
                replacement: String::new(),
                safe: true,
            },
            (0, 4),
        );
        let (mut text, mut introduced) =
            action.rewrite_with_line_endings(&text, introduced.as_deref(), retained.as_mut());
        super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
        assert_eq!(introduced, Some(Vec::new()));
        assert_eq!(retained.as_ref().unwrap().hidden_cr, vec![1]);
        assert_eq!(
            super::restore_copied_crlf(&text, &retained.as_ref().unwrap().hidden_cr),
            "a\r\né\n"
        );
        // Unicodeの後ろの新規文字位置も、CR除去数だけbyte単位で詰める。
        let mut text = "a\r\né€\n".to_owned();
        let mut introduced = Some(vec![5]);
        let mut retained = Some(super::LineEndingProvenance::new(&text));
        super::normalize_corrected_line_endings(&mut text, &mut introduced, &mut retained);
        assert_eq!(introduced, Some(vec![4]));
        assert_eq!(retained.as_ref().unwrap().hidden_cr, vec![1]);
        assert_eq!(text, "a\né€\n");
    }

    /// 本家Correctorのclobber方針を使い、Teamと同じcopごとの結合順を固定する。
    /// crossing_deletionsだけを許し、異なる置換とswallowed_insertionsは拒む。
    #[test]
    fn legacy_character_provenance_follows_surviving_edits_across_passes() {
        let action = |start, end, replacement: &str| {
            super::Action::from_edit(
                &Edit {
                    start,
                    end,
                    replacement: replacement.to_owned(),
                    safe: true,
                },
                (start, end),
            )
        };
        let tree = super::Action::root()
            .combine(&action(0, 2, ""))
            .unwrap()
            .combine(&action(3, 3, "é"))
            .unwrap();
        let (text, positions) = tree.rewrite("éx", Some(&[]));
        assert_eq!(text, "xé");
        assert_eq!(positions.as_deref(), Some([1].as_slice()));
        let (text, positions) = action(0, 0, "_").rewrite(&text, positions.as_deref());
        assert_eq!(text, "_xé");
        assert_eq!(positions.as_deref(), Some([2].as_slice()));
        let (text, positions) = action(0, text.len(), "'é'").rewrite(&text, positions.as_deref());
        assert_eq!(text, "'é'");
        assert_eq!(positions.as_deref(), Some([1].as_slice()));
        let (text, positions) = action(1, 3, "").rewrite(&text, positions.as_deref());
        assert_eq!(text, "''");
        assert!(positions.unwrap().is_empty());
        // 元の文字の並べ替えや、新しい文字の後続にある原本文字も由来を保つ。
        let (_, positions) = action(0, 6, "本日").rewrite("日本", Some(&[]));
        assert!(positions.unwrap().is_empty());
        let (text, positions) = action(0, 3, "€日").rewrite("日", Some(&[]));
        assert_eq!(positions.as_deref(), Some([0].as_slice()));
        let (text, positions) = action(0, 3, "").rewrite(&text, positions.as_deref());
        assert_eq!(text, "日");
        assert!(positions.unwrap().is_empty());
        // 曖昧な全体置換は由来を消さず、明示された残す範囲なら正しく分けられる。
        let mut selection = action(0, 5, "é");
        let (_, positions) = selection.rewrite("é é", Some(&[3]));
        assert_eq!(positions.as_deref(), Some([0].as_slice()));
        selection.characters_from = Some(super::CorrectionCharacterSource::Candidates(0, 2));
        let (_, positions) = selection.rewrite("é é", Some(&[3]));
        assert!(positions.unwrap().is_empty());
        selection.characters_from = Some(super::CorrectionCharacterSource::Candidates(3, 5));
        let (_, positions) = selection.rewrite("é é", Some(&[3]));
        assert_eq!(positions.as_deref(), Some([0].as_slice()));
        let mut copy = action(0, 5, "'é é'");
        copy.characters_from = Some(super::CorrectionCharacterSource::Copied(0, 5));
        let (text, positions) = copy.rewrite("é é", Some(&[3]));
        assert_eq!(positions.as_deref(), Some([4].as_slice()));
        let mut first = action(0, text.len(), "é");
        first.characters_from = Some(super::CorrectionCharacterSource::Copied(1, 3));
        let (_, positions) = first.rewrite(&text, positions.as_deref());
        assert!(positions.unwrap().is_empty());
        // 非 ASCII を複写する置換や、取り消された挿入は新しい由来を作らない。
        let (text, positions) = action(0, 2, "'é'").rewrite("éx", Some(&[]));
        assert_eq!(text, "'é'x");
        assert!(positions.unwrap().is_empty());
        let kept = action(0, 2, "y");
        assert!(kept.combine(&action(1, 1, "é")).is_err());
        let (text, positions) = kept.rewrite("ab", Some(&[]));
        assert_eq!(text, "y");
        assert!(positions.unwrap().is_empty());
    }

    #[test]
    fn edits_compose_the_way_tree_rewriter_composes_them() {
        let source = "abcdefghij";
        // Two cops inserting at one offset both land, later text first -- the whole reason the
        // composition cannot be "one insertion per offset wins".
        assert_eq!(
            composed(source, &[("", &[&[(3, 3, "A")]]), ("", &[&[(3, 3, "B")]])]),
            "abcBAdefghij"
        );
        assert_eq!(
            composed(
                source,
                &[
                    ("", &[&[(3, 3, "A")]]),
                    ("", &[&[(3, 3, "B")]]),
                    ("", &[&[(3, 3, "C")]]),
                ]
            ),
            "abcCBAdefghij"
        );
        // One offense inserting twice at one offset composes with itself the same way.
        assert_eq!(
            composed(source, &[("", &[&[(3, 3, "A"), (3, 3, "B")]])]),
            "abcBAdefghij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(3, 3, "A")], &[(3, 3, "B")]])]),
            "abcBAdefghij"
        );

        // A replacement swallows what it strictly contains, and an insertion cannot be swallowed
        // silently: whichever of the two was merged second is the one that loses.
        assert_eq!(
            composed(source, &[("", &[&[(5, 5, "X")]]), ("", &[&[(3, 8, "R")]])]),
            "abcdeXfghij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(3, 8, "R")]]), ("", &[&[(5, 5, "X")]])]),
            "abcRij"
        );
        // An empty range only touching the replacement is disjoint from it, so both land.
        assert_eq!(
            composed(source, &[("", &[&[(3, 3, "X")]]), ("", &[&[(3, 8, "R")]])]),
            "abcXRij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(8, 8, "X")]]), ("", &[&[(3, 8, "R")]])]),
            "abcRXij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(3, 3, "A")]]), ("", &[&[(0, 3, "R")]])]),
            "RAdefghij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(0, 3, "R")]]), ("", &[&[(3, 3, "A")]])]),
            "RAdefghij"
        );
        // A deletion is a replacement too, so it swallows a deletion silently and clobbers on an
        // insertion.
        assert_eq!(
            composed(source, &[("", &[&[(4, 6, "")]]), ("", &[&[(2, 8, "R")]])]),
            "abRij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(2, 8, "R")]]), ("", &[&[(4, 6, "")]])]),
            "abRij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(4, 6, "")]]), ("", &[&[(2, 8, "")]])]),
            "abij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(5, 5, "X")]]), ("", &[&[(2, 8, "")]])]),
            "abcdeXfghij"
        );

        // Deletions are the one crossing RuboCop accepts: they fuse into the span of them all.
        assert_eq!(
            composed(source, &[("", &[&[(2, 4, "")]]), ("", &[&[(4, 6, "")]])]),
            "abghij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(2, 5, "")]]), ("", &[&[(4, 7, "")]])]),
            "abhij"
        );
        assert_eq!(
            composed(
                source,
                &[
                    ("", &[&[(2, 4, "")]]),
                    ("", &[&[(6, 8, "")]]),
                    ("", &[&[(3, 7, "")]]),
                ]
            ),
            "abij"
        );
        // Any other crossing clobbers, and so does a second replacement of the same span.
        assert_eq!(
            composed(source, &[("", &[&[(2, 6, "X")]]), ("", &[&[(4, 8, "Y")]])]),
            "abXghij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(2, 5, "X")]]), ("", &[&[(4, 7, "")]])]),
            "abXfghij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(2, 6, "X")]]), ("", &[&[(2, 6, "Y")]])]),
            "abXghij"
        );
        assert_eq!(
            composed(source, &[("", &[&[(2, 6, "X")]]), ("", &[&[(2, 6, "X")]])]),
            "abXghij"
        );
        // An insertion and a replacement of the same empty range merge rather than collide.
        assert_eq!(
            composed(source, &[("", &[&[(3, 3, "R")]]), ("", &[&[(3, 3, "A")]])]),
            "abcARdefghij"
        );

        // A pair of insertions bracketing a span leaves room for an edit between them.
        assert_eq!(
            composed(
                source,
                &[
                    ("", &[&[(2, 2, "("), (8, 8, ")")]]),
                    ("", &[&[(4, 6, "Q")]])
                ]
            ),
            "ab(cdQgh)ij"
        );
        // Two insertions and a replacement reaching over both: the replacement loses.
        assert_eq!(
            composed(
                source,
                &[
                    ("", &[&[(4, 4, "A")]]),
                    ("", &[&[(6, 6, "B")]]),
                    ("", &[&[(2, 8, "R")]]),
                ]
            ),
            "abcdAefBghij"
        );
    }

    /// The range a cop hands `insert_after` -- not the offset the text lands at -- decides whether
    /// another cop's replacement of the same construct swallows the insertion or wraps around it.
    ///
    /// `Layout/EmptyLineAfterGuardClause` is the pair that measures it: it reports the `end` keyword
    /// but inserts after `range_by_whole_lines(node.source_range)`, and `Style/IfUnlessModifier`
    /// replaces exactly that conditional. On the whole lines the insertion is the *parent* of the
    /// replacement and both land; on the keyword it is a *child* of it, which is the
    /// `swallowed_insertions` clobbering -- and that costs `Style/IfUnlessModifier` every correction
    /// it asked for in the file, so a different cop's form wins the node in a later pass. Measured on
    /// rails' `activerecord/.../schema_definitions.rb`.
    #[test]
    fn the_range_an_insertion_hangs_off_decides_who_survives() {
        let source = "if a\n  raise\nend\nb\n";
        // `end` is 13..16; the whole lines of the conditional are 0..16; the blank line lands at 16.
        let insertion = |anchor: std::ops::Range<usize>| {
            Offense::new(
                "Layout/EmptyLineAfterGuardClause",
                Severity::Convention,
                "test",
                13,
                16,
            )
            .corrected_by(Edit {
                start: 16,
                end: 16,
                replacement: "\n".to_owned(),
                safe: true,
            })
            .corrections_anchored_at(anchor)
        };
        let fold = || {
            Offense::new(
                "Style/IfUnlessModifier",
                Severity::Convention,
                "test",
                0,
                16,
            )
            .corrected_by(Edit {
                start: 0,
                end: 16,
                replacement: "raise if a".to_owned(),
                safe: true,
            })
        };
        let run = |offenses: Vec<Offense>| {
            let mut report = FileReport {
                path: "test.rb".into(),
                source: SourceFile::new("test.rb", source.to_owned()),
                offenses,
            };
            corrected_text(&mut report, CorrectMode::All, Correcting::Everything).0
        };

        assert_eq!(run(vec![insertion(0..16), fold()]), "raise if a\n\nb\n");
        // The negative control: anchored on the keyword the fold is swallowed and dropped whole, so
        // the conditional keeps its written form and only the blank line lands.
        assert_eq!(
            run(vec![insertion(13..16), fold()]),
            "if a\n  raise\nend\n\nb\n"
        );
    }

    /// 本家の corrector の呼び出しを、範囲と向きごと写した編集を組む。
    fn edit(start: usize, end: usize, replacement: &str) -> Edit {
        Edit {
            start,
            end,
            replacement: replacement.to_owned(),
            safe: true,
        }
    }

    /// `corrected_text` に通す 1 件の offense。報告範囲は空にしておく (アンカーを指定しない編集は
    /// この範囲から推すので、テストの編集はすべてアンカーか置換で書く)。
    fn anchored(
        cop: &'static str,
        edits: impl IntoIterator<Item = (Edit, Option<InsertionAnchor>)>,
    ) -> Offense {
        Offense::new(cop, Severity::Convention, "test", 0, 0).corrected_by_all_anchored(edits)
    }

    fn apply_offenses(source: &str, offenses: Vec<Offense>) -> String {
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", source.to_owned()),
            offenses,
        };
        corrected_text(&mut report, CorrectMode::All, Correcting::Everything).0
    }

    /// `Lint/AmbiguousBlockAssociation#wrap_in_parentheses` の 3 つの呼び出し:
    /// `remove(空白)`、`insert_before(空白, '(')`、`insert_after(最後の引数, ')')`。
    fn wrap_in_parentheses() -> Offense {
        anchored(
            "Lint/AmbiguousBlockAssociation",
            [
                (edit(3, 4, ""), None),
                InsertionAnchor::Before(3..4).insert("("),
                InsertionAnchor::After(4..13).insert(")"),
            ],
        )
    }

    /// `Style/NestedParenthesizedCalls#autocorrect`: `replace(空白, '(')` と
    /// `insert_after(最後の引数, ')')`。
    fn nested_parentheses() -> Offense {
        anchored(
            "Style/NestedParenthesizedCalls",
            [
                (edit(3, 4, "("), None),
                InsertionAnchor::After(4..13).insert(")"),
            ],
        )
    }

    /// 同じ範囲の削除と `insert_before` は 1 つの action になり、置換は `""` のまま残る。
    /// 本家の木と同じく `before="(" replace=Some("")` で、`(` を置換で持つ形ではない。
    #[test]
    fn a_removal_and_an_insertion_before_it_share_one_action() {
        let source = "foo bar { 1 }";
        let report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", source.to_owned()),
            offenses: vec![wrap_in_parentheses()],
        };
        let (tree, placed) = super::build_cop_corrector(&report, &[0], source);
        assert_eq!(placed, [0]);
        let shape: Vec<_> = tree
            .children
            .iter()
            .map(|action| {
                (
                    action.begin_pos..action.end_pos,
                    action.insert_before.as_str(),
                    action.replacement.as_deref(),
                    action.insert_after.as_str(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [(3..4, "(", Some(""), ""), (4..13, "", None, ")")],
            "本家の木: [3...4] before=\"(\" replace=\"\" / [4...13] after=\")\""
        );
    }

    /// 期待値はすべて、同じ呼び出しを本家の `RuboCop::Cop::Corrector` に積んで `merge!` した
    /// 実出力 (衝突した cop は `Team#suppress_clobbering` と同じく丸ごと捨てる)。
    ///
    /// 空白の置換が `""` と `"("` で食い違うので、後に結合する NestedParenthesizedCalls は
    /// 衝突してそのパスの編集を全部失う。`(` への置換 1 つに畳んだ形では同じ置換として合流し、
    /// 両方の `)` が入る -- 本家でも畳めば `foo(bar { 1 }))` になる。
    #[test]
    fn a_removal_with_an_insertion_clobbers_a_replacement_of_the_same_range() {
        let source = "foo bar { 1 }";
        assert_eq!(
            apply_offenses(source, vec![wrap_in_parentheses(), nested_parentheses()]),
            "foo(bar { 1 })"
        );
        // 衝突しない別の箇所の編集も、衝突した cop と一緒に捨てられる。
        let with_disjoint = anchored(
            "Style/NestedParenthesizedCalls",
            [
                InsertionAnchor::Before(0..3).insert("!"),
                (edit(3, 4, "("), None),
                InsertionAnchor::After(4..13).insert(")"),
            ],
        );
        assert_eq!(
            apply_offenses(source, vec![wrap_in_parentheses(), with_disjoint]),
            "foo(bar { 1 })"
        );
        // 陰性対照: 削除と挿入を `(` への置換に畳むと、閉じ括弧が 2 つ入る。
        let folded = anchored(
            "Lint/AmbiguousBlockAssociation",
            [
                (edit(3, 4, "("), None),
                InsertionAnchor::After(4..13).insert(")"),
            ],
        );
        assert_eq!(
            apply_offenses(source, vec![folded, nested_parentheses()]),
            "foo(bar { 1 }))"
        );
    }

    /// 挿入が掛かる範囲が木の親子を決める。空白と引数をまとめて置換する後続の補正は、本家の
    /// 木では挿入を飲み込むので衝突する。`(` と `)` を「空白から引数の末尾まで」の 1 つの wrap に
    /// すると、置換と同じ範囲の合流になって通ってしまう (本家で同じ形を積むと `foo((x))`)。
    #[test]
    fn the_ranges_the_insertions_hang_off_decide_what_a_wider_replacement_swallows() {
        let source = "foo bar { 1 }";
        let wide = || anchored("Style/Wide", [(edit(3, 13, "(x)"), None)]);
        assert_eq!(
            apply_offenses(source, vec![wrap_in_parentheses(), wide()]),
            "foo(bar { 1 })"
        );
        let one_wrap = anchored(
            "Lint/AmbiguousBlockAssociation",
            [
                (edit(3, 4, ""), None),
                InsertionAnchor::Before(3..13).insert("("),
                InsertionAnchor::After(3..13).insert(")"),
            ],
        );
        assert_eq!(apply_offenses(source, vec![one_wrap, wide()]), "foo((x))");

        // キーワード引数の並びは本家では 1 つの hash。閉じ括弧をその全体に掛けると、hash を
        // 置換する補正と同じ範囲で合流し、最後の pair に掛けると飲み込まれて衝突する。
        let source = "foo a: 1, b: 2";
        let hash = || anchored("Style/Hash", [(edit(4, 14, "h"), None)]);
        let close = |anchor| {
            anchored(
                "Style/NestedParenthesizedCalls",
                [
                    (edit(3, 4, "("), None),
                    InsertionAnchor::After(anchor).insert(")"),
                ],
            )
        };
        assert_eq!(apply_offenses(source, vec![close(4..14), hash()]), "foo(h)");
        assert_eq!(
            apply_offenses(source, vec![close(10..14), hash()]),
            "foo(a: 1, b: 2)"
        );
    }

    /// 空の範囲への挿入は向きで並びが変わる。`insert_after` は後から来たものが後ろ、
    /// `insert_before` は前に付く。向きを持たない挿入は `insert_before` として扱われる。
    #[test]
    fn an_insertion_into_an_empty_range_keeps_its_side() {
        let source = "abcdefghij";
        let at = |cop, anchor: InsertionAnchor, text| anchored(cop, [anchor.insert(text)]);
        assert_eq!(
            apply_offenses(
                source,
                vec![
                    at("Layout/A", InsertionAnchor::After(5..5), "A"),
                    at("Layout/B", InsertionAnchor::After(5..5), "B"),
                ]
            ),
            "abcdeABfghij"
        );
        assert_eq!(
            apply_offenses(
                source,
                vec![
                    at("Layout/A", InsertionAnchor::Before(5..5), "A"),
                    at("Layout/B", InsertionAnchor::Before(5..5), "B"),
                ]
            ),
            "abcdeBAfghij"
        );
        assert_eq!(
            apply_offenses(
                source,
                vec![
                    at("Layout/A", InsertionAnchor::After(5..5), "A"),
                    at("Layout/B", InsertionAnchor::Before(5..5), "B"),
                ]
            ),
            "abcdeBAfghij"
        );
    }

    /// 位置がアンカーの向きの端にない挿入は cop の指定の誤りなので、アンカーを使わず従来の
    /// 推定に戻す。置換にはアンカーが効かない。
    #[test]
    fn an_anchor_that_does_not_fit_its_edit_is_ignored() {
        let source = "abcdefghij";
        let misplaced = anchored(
            "Layout/A",
            [(edit(7, 7, "X"), Some(InsertionAnchor::After(2..5)))],
        );
        assert_eq!(apply_offenses(source, vec![misplaced]), "abcdefgXhij");
        let replacement = anchored(
            "Layout/A",
            [(edit(2, 5, "R"), Some(InsertionAnchor::Before(2..5)))],
        );
        assert_eq!(apply_offenses(source, vec![replacement]), "abRfghij");
    }

    #[test]
    fn a_clobbering_cop_loses_every_correction_it_asked_for() {
        // The cop's second edit collides with what is already scheduled, so the run drops the cop
        // whole -- the insertion at 9, which collides with nothing, goes with it.
        assert_eq!(
            composed(
                "abcdefghij",
                &[
                    ("", &[&[(1, 1, "A")]]),
                    ("", &[&[(9, 9, "Z"), (0, 3, "R")]]),
                ]
            ),
            "aAbcdefghij"
        );
        // Two offenses of one cop colliding with each other is a cop error instead, which costs
        // only the offense that could not be placed.
        assert_eq!(
            composed(
                "abcdefghij",
                &[("", &[&[(0, 3, "R")], &[(1, 1, "X")], &[(9, 9, "Z")]])]
            ),
            "RdefghiZj"
        );
    }

    #[test]
    fn cops_merge_in_registry_order() {
        // 名前順から先行するクラスも含め、全 cop が本家の登録順へ戻ることを確かめる。
        let mut names = crate::formatter::COP_REGISTRY_ORDER.to_vec();
        names.reverse();
        names.sort_by(|left, right| {
            super::cop_merge_order(left).cmp(&super::cop_merge_order(right))
        });
        assert_eq!(names, crate::formatter::COP_REGISTRY_ORDER);
        // 先にマージする挿入ほど内側になる。架空名による補正ツリーの対照も保つ。
        assert_eq!(
            composed(
                "abcdefghij",
                &[
                    ("Style/A", &[&[(3, 3, "A")]]),
                    ("Layout/Z", &[&[(3, 3, "B")]])
                ]
            ),
            "abcABdefghij"
        );
        assert_eq!(
            composed(
                "abcdefghij",
                &[
                    ("Style/Wide", &[&[(3, 8, "R")]]),
                    ("Layout/Narrow", &[&[(5, 5, "X")]]),
                ]
            ),
            "abcdeXfghij"
        );
    }

    #[test]
    fn an_insertion_hangs_off_the_range_its_offense_was_reported_on() {
        // `Layout/SpaceAfterComma` reports the comma and puts a space after it; `Lint/`
        // `UnusedBlockArgument` reports the argument and puts an underscore before it. Both land on
        // the same offset, and upstream orders them by the ranges the cops passed their correctors:
        // the comma comes first, so the space does. Ordering them by cop instead would spell the
        // argument `_ v`.
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", "abcdefghij".to_owned()),
            offenses: vec![
                Offense::new("Layout/SpaceAfterComma", Severity::Convention, "test", 3, 4)
                    .corrected_by(Edit {
                        start: 4,
                        end: 4,
                        replacement: " ".to_owned(),
                        safe: true,
                    }),
                Offense::new("Lint/UnusedBlockArgument", Severity::Warning, "test", 4, 5)
                    .corrected_by(Edit {
                        start: 4,
                        end: 4,
                        replacement: "_".to_owned(),
                        safe: true,
                    }),
            ],
        };

        assert_eq!(
            corrected_text(&mut report, CorrectMode::All, Correcting::Everything).0,
            "abcd _efghij"
        );
    }

    #[test]
    fn a_pair_of_insertions_around_an_offense_wraps_it() {
        // A cop bracketing what it reported puts one insertion at either end of the offense, which
        // is the `wrap` upstream records as a single action. Two cops wrapping the same thing nest,
        // rather than crossing the way two bare insertions at those offsets would.
        let bracket = |cop, open: &str, close: &str| {
            Offense::new(cop, Severity::Convention, "test", 2, 8).corrected_by_all([
                Edit {
                    start: 2,
                    end: 2,
                    replacement: open.to_owned(),
                    safe: true,
                },
                Edit {
                    start: 8,
                    end: 8,
                    replacement: close.to_owned(),
                    safe: true,
                },
            ])
        };
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", "abcdefghij".to_owned()),
            offenses: vec![bracket("Layout/A", "(", ")"), bracket("Layout/B", "[", "]")],
        };

        assert_eq!(
            corrected_text(&mut report, CorrectMode::All, Correcting::Everything).0,
            "ab[(cdefgh)]ij"
        );
    }

    #[test]
    fn an_offense_whose_edits_change_nothing_is_not_corrected() {
        let mut report = FileReport {
            path: "test.rb".into(),
            source: SourceFile::new("test.rb", "abcdefghij".to_owned()),
            offenses: vec![
                Offense::new("Layout/A", Severity::Convention, "test", 0, 0).corrected_by(Edit {
                    start: 3,
                    end: 3,
                    replacement: String::new(),
                    safe: true,
                }),
            ],
        };

        let (text, corrected) =
            corrected_text(&mut report, CorrectMode::All, Correcting::Everything);

        assert_eq!(text, "abcdefghij");
        assert_eq!(corrected, 0);
        assert!(!report.offenses[0].corrected);
    }

    #[test]
    fn discovers_ruby_files_and_honors_exclusions() {
        let directory = tempdir().unwrap();
        std::fs::write(directory.path().join("good.rb"), "puts 1\n").unwrap();
        std::fs::create_dir(directory.path().join("vendor")).unwrap();
        std::fs::write(directory.path().join("vendor/skip.rb"), "puts 1\n").unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection::default();
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(
            inspect_files(&targets, &config, &selection, false)
                .unwrap()
                .len(),
            1
        );
    }

    /// The guard against writing a correction that does not parse is on when nothing asks for it.
    ///
    /// **The switch that turns it off is an environment variable, so nothing in the argument list
    /// or the configuration records which way it was set.** A reader of a `-A` run cannot tell a
    /// guarded run from an unguarded one by looking at the command. This pins the default here so
    /// that turning it off has to be a deliberate edit to a test, not a forgotten variable.
    ///
    /// `Lint/SafeNavigationChain` rewrites `x&.foo[bar] += 1` as `x&.foo&.[](bar) += 1`, which
    /// assigns to a method call and does not parse. Measured with `ruby -c`, not assumed.
    ///
    /// ## ★ The fixture is chosen so that fixing a cop cannot break this test
    ///
    /// Two earlier fixtures were bugs of ours, and both were fixed within hours -- `foo:bar => 1`
    /// through `Style/HashSyntax`, then `foo&.bar ? foo&.bar - 1 : baz` through this same cop --
    /// each time leaving this test failing with "the source must come back untouched" for a reason
    /// that had nothing to do with the guard.
    ///
    /// **This one is upstream's output too.** RuboCop 1.89.0 writes the same unparsable line for
    /// the same input, so reproducing it is what compatibility requires and no one will "fix" it
    /// here. If upstream ever changes, the port follows and this fixture is reconsidered then.
    ///
    /// **If it does fail, do not delete it -- swap in another case that still breaks**, and check
    /// the candidate first: with `SONICOP_NO_SYNTAX_GUARD=1` its output must fail `ruby -c`. The
    /// list lives in `#41` ("the cases the guard stopped").
    ///
    /// The whole default selection is used, not `only`. Under `--only <cop>` the guard does not
    /// fire here, so narrowing would make this pass without testing anything.
    #[test]
    fn a_correction_that_would_not_parse_is_not_applied() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection::default();
        let original = "x&.foo[bar] += 1\n";
        let report = inspect_source(
            directory.path().join("example.rb"),
            original.to_owned(),
            &config,
            &selection,
        )
        .unwrap();

        let outcome = correct_file(report, CorrectMode::All, &config, &selection).unwrap();

        assert_eq!(
            outcome.text, original,
            "the source must come back untouched"
        );
        assert!(!outcome.rewritten, "nothing may be written");
        assert_eq!(
            outcome.corrected_count, 0,
            "a correction that never lands is not a correction"
        );
        assert!(
            outcome
                .rollback
                .as_deref()
                .is_some_and(|message| message.contains("syntax error")),
            "the refusal has to be reported, not swallowed: {:?}",
            outcome.rollback
        );
    }

    #[test]
    fn autocorrect_keeps_corrected_offenses_and_their_original_lines() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection::default();
        let report = inspect_source(
            directory.path().join("example.rb"),
            "x = 'a'  \n".to_owned(),
            &config,
            &selection,
        )
        .unwrap();

        let outcome = correct_file(report, CorrectMode::Safe, &config, &selection).unwrap();

        assert!(outcome.infinite_loop.is_none());
        assert!(outcome.corrected_count > 0);
        assert_eq!(
            outcome
                .report
                .offenses
                .iter()
                .filter(|offense| offense.corrected)
                .count(),
            outcome.corrected_count
        );
        let trailing = outcome
            .report
            .offenses
            .iter()
            .find(|offense| offense.cop_name == "Layout/TrailingWhitespace")
            .expect("the corrected trailing whitespace offense survives into the final report");
        assert!(trailing.corrected);
        assert_eq!(trailing.source_line(&outcome.report.source), "x = 'a'  \n");
    }

    #[test]
    fn an_undecodable_file_reports_a_fatal_offense_without_stopping_the_run() {
        let directory = tempdir().unwrap();
        std::fs::write(directory.path().join("good.rb"), "puts 1\n").unwrap();
        std::fs::write(directory.path().join("bad.rb"), b"x = \"\xff\xfe\"\n").unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection::default();
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();

        let reports = inspect_files(&targets, &config, &selection, false).unwrap();

        assert_eq!(reports.len(), 2);
        let bad = reports
            .iter()
            .find(|report| report.path.ends_with("bad.rb"))
            .unwrap();
        assert_eq!(bad.offenses.len(), 1);
        assert_eq!(bad.offenses[0].cop_name, "Lint/Syntax");
        assert_eq!(bad.offenses[0].severity, Severity::Fatal);
        assert_eq!(bad.offenses[0].message, "Invalid byte sequence in utf-8.");
        let location = bad.offenses[0].location(&bad.source);
        assert_eq!((location.line, location.column), (1, 1));
    }

    #[test]
    fn a_file_is_decoded_with_the_encoding_its_magic_comment_names() {
        // `Prefer single-quoted` on a Shift_JIS line: reachable only once the bytes are decoded,
        // and the column has to count characters of the decoded text, not the encoded bytes.
        let directory = tempdir().unwrap();
        let mut bytes = b"# encoding: cp932\nx = \"".to_vec();
        bytes.extend_from_slice(b"\x93\xfa\x96\x7b"); // 日本 in Shift_JIS
        bytes.extend_from_slice(b"\"\n");
        std::fs::write(directory.path().join("sjis.rb"), bytes).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Style/StringLiterals".to_owned()],
            ..Selection::default()
        };
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();

        let reports = inspect_files(&targets, &config, &selection, false).unwrap();

        let report = &reports[0];
        assert_eq!(report.offenses.len(), 1);
        let location = report.offenses[0].location(&report.source);
        assert_eq!((location.line, location.column, location.length), (2, 5, 4));
    }

    #[test]
    fn ruby_euc_jp_aliases_decode_and_preserve_original_bytes() {
        // 実 Ruby が認める大小文字の別名でも、復号と再符号化を同じ解決へ向ける。
        for label in ["EUC-JP", "eucJP", "EUCJP", "eucjp", "EucJp"] {
            for ending in ["\n", "\r\n"] {
                let decoded = format!("# encoding: {label}{ending}puts(\"あ\"){ending}");
                let mut original = format!("# encoding: {label}{ending}puts(\"").into_bytes();
                original.extend_from_slice(b"\xa4\xa2");
                original.extend_from_slice(format!("\"){ending}").as_bytes());
                let Decoded::Text(found) = super::decoded_bytes(original.clone()) else {
                    panic!("Ruby の EUC-JP alias を拒否した: {label}");
                };
                assert_eq!(found, decoded);
                assert_eq!(output_bytes(&decoded, Some(label)).unwrap(), original);
            }
        }
    }

    #[test]
    fn browser_only_euc_jp_labels_remain_unknown_to_ruby() {
        // 記号の除去や WHATWG の別名を流用すると、本家の構文エラーまで受理してしまう。
        for label in [
            "x-euc-jp",
            "cseucpkdfmtjapanese",
            "EUC_JP",
            "EUCJPP",
            "XEUCJP",
            "EUCJP_",
        ] {
            let original = format!("# encoding: {label}\nputs 1\n").into_bytes();
            let Decoded::Undecodable(message) = super::decoded_bytes(original) else {
                panic!("Ruby が拒むラベルを受理した: {label}");
            };
            assert_eq!(
                message,
                format!("Unknown encoding name - {}.", label.to_ascii_lowercase())
            );
        }
    }

    #[test]
    fn a_binary_source_is_measured_one_byte_at_a_time() {
        // Valid UTF-8, but declared binary: Ruby counts each byte of `あ` as its own character, so
        // the string literal is 5 long rather than 3.
        let directory = tempdir().unwrap();
        std::fs::write(
            directory.path().join("binary.rb"),
            "# encoding: ASCII-8BIT\nx = \"\u{3042}\"\n",
        )
        .unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Style/StringLiterals".to_owned()],
            ..Selection::default()
        };
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();

        let reports = inspect_files(&targets, &config, &selection, false).unwrap();

        let report = &reports[0];
        let location = report.offenses[0].location(&report.source);
        assert_eq!((location.line, location.column, location.length), (2, 5, 5));
    }

    #[test]
    fn a_correction_goes_back_out_in_the_encoding_the_file_declares() {
        // RuboCop would write UTF-8 here and leave the file claiming cp932, which no longer loads.
        let corrected = "# encoding: cp932\nx = '\u{65e5}\u{672c}'\n";

        let bytes = output_bytes(corrected, Some("cp932")).unwrap();

        assert!(bytes.ends_with(b"x = '\x93\xfa\x96\x7b'\n"));
    }

    #[test]
    fn a_correction_that_the_declared_encoding_cannot_hold_is_refused() {
        // cp932 にない絵文字を代替すると、補正とは無関係な元の文字まで失う。
        let corrected = "# encoding: cp932\nx = '\u{1f363}'\n";

        assert!(output_bytes(corrected, Some("cp932")).is_err());
    }

    #[test]
    fn a_seven_bit_write_refuses_non_ascii_and_preserves_the_file() {
        for label in ["US-ASCII", "ASCII", "646"] {
            let directory = tempdir().unwrap();
            let path = directory.path().join("example.rb");
            let original = format!("# coding: {label}\nx = '\\u00e9'\n");
            std::fs::write(&path, &original).unwrap();
            // encoding_rs では表せる é でも、Ruby の ASCII 宣言では 1 バイトも書けない。
            let corrected = format!("# coding: {label}\nx = 'é'\n");
            let error = write_corrected(&path, &corrected).unwrap_err();
            assert!(error.is::<super::CorrectionEncodingError>());
            assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
            let ascii = format!("# coding: {label}\nx = 1\n");
            write_corrected(&path, &ascii).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), ascii.as_bytes());
        }
    }

    #[test]
    fn a_legacy_write_keeps_original_multibyte_sequences_or_refuses_the_whole_write() {
        for label in ["Shift_JIS", "Windows-31J"] {
            let directory = tempdir().unwrap();
            let path = directory.path().join("example.rb");
            let mut original = format!("# coding: {label}\n# ").into_bytes();
            original.extend_from_slice(b"\xED\x40\nx = 1\n");
            std::fs::write(&path, &original).unwrap();
            let Decoded::Text(decoded) = decoded_source(&path).unwrap() else {
                panic!("原本の読み込みに失敗した");
            };
            let corrected = decoded.replace("x = 1", "x = 2");
            assert!(write_corrected(&path, &corrected).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        let mut original = b"# coding: cp932\nx = \"".to_vec();
        original.extend_from_slice(b"\x93\xfa\x96\x7b\"\n");
        std::fs::write(&path, &original).unwrap();
        super::write_corrected_with_provenance(&path, "# coding: cp932\nx = '日本'\n", Some(false))
            .unwrap();
        let mut expected = b"# coding: cp932\nx = '".to_vec();
        expected.extend_from_slice(b"\x93\xfa\x96\x7b'\n");
        assert_eq!(std::fs::read(&path).unwrap(), expected);
    }

    #[test]
    fn a_utf8_file_that_names_another_encoding_goes_back_out_unchanged() {
        // rails' `1_currencies_have_symbols.rb`: the comment says ISO-8859-15, the bytes are UTF-8,
        // and `decoded_source` read it as UTF-8. Encoding the `€` to `\xa4` on the way out would
        // turn a three-character string into a one-character one -- a change no cop asked for.
        let corrected = "# coding: ISO-8859-15\nx = '\u{20ac}'\n";

        let bytes = output_bytes(corrected, None).unwrap();

        assert_eq!(bytes, corrected.as_bytes());
    }

    #[test]
    fn an_encoding_a_magic_comment_does_not_name_stays_undecodable() {
        // A Vim modeline is not a Ruby magic comment, so RuboCop never reads the encoding out of
        // one and reports the file as a syntax error instead.
        let directory = tempdir().unwrap();
        let mut bytes = b"# vim: set fileencoding=cp932\nx = \"".to_vec();
        bytes.extend_from_slice(b"\x93\xfa\x96\x7b\"\n");
        std::fs::write(directory.path().join("vim.rb"), bytes).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();

        let reports = inspect_files(&targets, &config, &Selection::default(), false).unwrap();

        assert_eq!(reports[0].offenses.len(), 1);
        assert_eq!(reports[0].offenses[0].cop_name, "Lint/Syntax");
        // The cop name alone passed while the message was wrong for every case but this one, which
        // is how the missing `Unknown encoding name` went unnoticed. Read the message too.
        assert_eq!(
            reports[0].offenses[0].message,
            "Invalid byte sequence in utf-8."
        );
    }

    #[test]
    fn a_correction_that_would_leave_the_file_unparsable_is_not_written() {
        // `Layout/LineLength` folds the line that opens the heredoc, so the rest of the statement
        // lands before the body and Ruby can no longer read the file. RuboCop writes it anyway,
        // and the `-A` comparison calls the two byte-identical outputs a match.
        let directory = tempdir().unwrap();
        let source = "# frozen_string_literal: true\n\ndef m(account, limit)\n               Account.find_by_sql([<<~SQL.squish, { id: account.id, limit: limit, extra: 1,              more: 2, yet: 3, again: 4, plus: 5, over: 6 }])\n    select 1\n  SQL\nend\n";
        std::fs::write(directory.path().join("heredoc.rb"), source).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();
        let selection = Selection {
            correcting: true,
            ..Selection::default()
        };
        let reports = inspect_files(&targets, &config, &selection, false).unwrap();

        let outcome = correct_file(
            reports.into_iter().next().unwrap(),
            CorrectMode::All,
            &config,
            &selection,
        )
        .unwrap();

        assert!(outcome.rollback.is_some());
        // The text handed back is the source as read, so the caller writes nothing.
        assert_eq!(outcome.text, source);
        assert!(!outcome.rewritten);
        // Nothing reached disk, so nothing may be reported as corrected.
        assert_eq!(outcome.corrected_count, 0);
        assert!(
            !outcome
                .report
                .offenses
                .iter()
                .any(|offense| offense.corrected)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_path_keeps_namespaces_and_wide_names() {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use std::path::Path;

        for (path, expected) in [
            (r"C:\parent\child\..\case.rb. ", r"\\?\C:\parent\case.rb"),
            (
                r"\\server\share\child\..\case.rb",
                r"\\?\UNC\server\share\case.rb",
            ),
            (r"\\?\C:\parent\case.rb. ", r"\\?\C:\parent\case.rb. "),
            (
                r"\\?\UNC\server\share\case.rb",
                r"\\?\UNC\server\share\case.rb",
            ),
            (r"\\.\pipe\sonicop", r"\\.\pipe\sonicop"),
        ] {
            assert_eq!(
                super::windows_atomic_path(Path::new(path)).unwrap(),
                Path::new(expected)
            );
        }

        // OsString は Unicode に復号できない NTFS 名も持つため、UTF-16 を損失なく保つ。
        let mut units: Vec<u16> = r"C:\parent\".encode_utf16().collect();
        units.push(0xd800);
        units.extend(".rb".encode_utf16());
        let path = OsString::from_wide(&units);
        let expected: Vec<u16> = r"\\?\".encode_utf16().chain(units).collect();
        assert_eq!(
            super::windows_atomic_path(Path::new(&path))
                .unwrap()
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            expected
        );
    }

    /// A symlink names a file; it is not one. `rename(2)` replaces the name, so the link becomes a
    /// regular file holding the correction while the file it pointed at keeps the original -- the
    /// user's edit lands in a place nothing reads, and the link is gone with no message about it.
    #[cfg(unix)]
    #[test]
    fn write_corrected_rewrites_through_a_symlink_and_leaves_the_link_alone() {
        let directory = tempdir().unwrap();
        let target = directory.path().join("real.rb");
        let link = directory.path().join("link.rb");
        std::fs::write(&target, "y  = 2\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        write_corrected(&link, "y = 2\n").unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink was replaced by a regular file"
        );
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "y = 2\n",
            "the file the link points at was not corrected"
        );
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640,
            "writing in place must not change the file's permissions"
        );
    }

    /// 同じファイルの別名を残したまま置換すると、補正済みと報告した本文が別名へ届かない。
    #[cfg(any(unix, windows))]
    #[test]
    fn write_corrected_keeps_a_hard_linked_inode_shared() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("one.rb");
        let second = directory.path().join("two.rb");
        std::fs::write(&first, "y  = 2\n").unwrap();
        std::fs::hard_link(&first, &second).unwrap();

        write_corrected(&first, "y = 2\n").unwrap();

        assert!(
            same_file::is_same_file(&first, &second).unwrap(),
            "補正でハードリンクの共有が失われた"
        );
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::MetadataExt::nlink(&std::fs::metadata(&first).unwrap()),
            2,
            "補正で元の inode へのリンクが失われた"
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            "y = 2\n",
            "同じファイルの別名に補正前の本文が残った"
        );
    }

    /// The ordinary file keeps the atomic path, and its permissions with it. Nothing about the
    /// distinction above may cost the common case what it already had.
    #[test]
    fn write_corrected_replaces_an_ordinary_file_and_preserves_its_permissions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("plain.rb");
        std::fs::write(&path, "y  = 2\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_corrected(&path, "y = 2\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "y = 2\n");
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "the temporary file was persisted without the original's permissions"
        );
    }

    #[test]
    fn write_corrected_checks_the_files_write_access_before_replacement() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("readonly.rb");
        fs::write(&path, "y  = 2\n").unwrap();
        let original_permissions = fs::metadata(&path).unwrap().permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        // root 等で許されるかどうかも含め、実際の OS のアクセス判定に合わせる。
        let writable = fs::OpenOptions::new().write(true).open(&path).is_ok();
        let result = write_corrected(&path, "y = 2\n");
        fs::set_permissions(&path, original_permissions).unwrap();
        assert_eq!(result.is_ok(), writable);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            if writable { "y = 2\n" } else { "y  = 2\n" }
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_corrected_can_write_a_file_in_a_readonly_directory() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        fs::write(&path, "y  = 2\n").unwrap();
        let permissions = fs::metadata(directory.path()).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(directory.path(), readonly).unwrap();
        let result = write_corrected(&path, "y = 2\n");
        fs::set_permissions(directory.path(), permissions).unwrap();
        result.unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "y = 2\n");
    }

    #[cfg(unix)]
    #[test]
    fn write_corrected_keeps_extended_attributes() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        fs::write(&path, "y  = 2\n").unwrap();
        xattr::set(&path, "user.sonicop_test", b"retained").unwrap();
        write_corrected(&path, "y = 2\n").unwrap();
        assert_eq!(
            xattr::get(&path, "user.sonicop_test").unwrap(),
            Some(b"retained".to_vec())
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "y = 2\n");
    }

    #[cfg(unix)]
    #[test]
    fn write_corrected_keeps_foreign_ownership_when_running_as_root() {
        use std::os::unix::fs::MetadataExt;

        let directory = tempdir().unwrap();
        if fs::metadata(directory.path()).unwrap().uid() != 0 {
            return;
        }
        // 通常のテストに加え、CI ではこのケースを特権ユーザーで別に実行する。
        let path = directory.path().join("example.rb");
        fs::write(&path, "y  = 2\n").unwrap();
        std::os::unix::fs::chown(&path, Some(65534), Some(65534)).unwrap();
        write_corrected(&path, "y = 2\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), (65534, 65534));
        assert_eq!(fs::read_to_string(&path).unwrap(), "y = 2\n");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn write_corrected_keeps_a_macos_acl() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        fs::write(&path, "y  = 2\n").unwrap();
        let mut entries = exacl::getfacl(&path, None).unwrap();
        entries.push(exacl::AclEntry::allow_group(
            "everyone",
            exacl::Perm::READ,
            None,
        ));
        exacl::setfacl(&[&path], &entries, None).unwrap();
        let before = exacl::getfacl(&path, None).unwrap();
        assert!(!before.is_empty());
        write_corrected(&path, "y = 2\n").unwrap();
        assert_eq!(exacl::getfacl(&path, None).unwrap(), before);
        assert_eq!(fs::read_to_string(&path).unwrap(), "y = 2\n");
    }

    #[test]
    fn result_cache_rechecks_content_when_size_and_mtime_are_restored() {
        for (original, replacement, expected) in
            [("puts 1 \n", "puts 10\n", 0), ("puts 10\n", "puts 1 \n", 1)]
        {
            let directory = tempdir().unwrap();
            let path = directory.path().join("example.rb");
            fs::write(&path, original).unwrap();
            let before = fs::metadata(&path).unwrap();
            let config = Config::load(None, directory.path()).unwrap();
            let selection = Selection {
                only: vec!["Layout/TrailingWhitespace".to_owned()],
                ..Selection::default()
            };
            let configs = super::ConfigStore::new(config.clone(), false, false);
            let cache = ResultCache::new(
                directory.path().join("cache"),
                config.path_base(),
                &selection,
                100,
            )
            .unwrap();
            cache.prepare(1);
            let paths = vec![path.clone()];
            let first = super::inspect_files_with_store_cached(
                &paths,
                &configs,
                &selection,
                false,
                Some(&cache),
            )
            .unwrap();
            assert_eq!(first[0].offenses.len(), 1 - expected);
            assert!(matches!(cache.load(&path, &config), Some(Cached::Fresh(_))));

            fs::write(&path, replacement).unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(before.modified().unwrap()))
                .unwrap();
            assert_eq!(
                ResultCache::stat(&fs::metadata(&path).unwrap()),
                ResultCache::stat(&before)
            );
            let second = super::inspect_files_with_store_cached(
                &paths,
                &configs,
                &selection,
                false,
                Some(&cache),
            )
            .unwrap();
            assert_eq!(second[0].offenses.len(), expected);
        }
    }

    #[test]
    fn an_encoding_no_such_name_exists_for_is_a_syntax_error_however_the_bytes_read() {
        // The body is plain ASCII, so nothing about the bytes is wrong. Ruby still refuses the file
        // because the name is not an encoding, and RuboCop reports that rather than reading on.
        let directory = tempdir().unwrap();
        std::fs::write(
            directory.path().join("typo.rb"),
            "# coding: no-such-encoding\nx = 1\n",
        )
        .unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();

        let reports = inspect_files(&targets, &config, &Selection::default(), false).unwrap();

        assert_eq!(reports[0].offenses.len(), 1);
        assert_eq!(
            reports[0].offenses[0].message,
            "Unknown encoding name - no-such-encoding."
        );
    }

    #[test]
    fn a_byte_over_seven_bits_under_a_us_ascii_declaration_names_itself() {
        // `encoding_rs` folds `us-ascii` into windows-1252 and would read this happily; Ruby does
        // not, and the byte it stopped at is part of the message.
        let directory = tempdir().unwrap();
        let mut bytes = b"# coding: ascii\nx = \"".to_vec();
        bytes.extend_from_slice(b"\xe2\"\n");
        std::fs::write(directory.path().join("seven.rb"), bytes).unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let targets = discover_targets(&[], directory.path(), &config, false, false).unwrap();

        let reports = inspect_files(&targets, &config, &Selection::default(), false).unwrap();

        assert_eq!(reports[0].offenses.len(), 1);
        // `ascii` and `646` are aliases; upstream reports the name they resolve to, in lower case.
        assert_eq!(reports[0].offenses[0].message, "\"\\xe2\" on us-ascii.");
    }

    #[test]
    fn a_declaration_of_a_single_byte_encoding_moves_the_columns_after_it() {
        // The bytes spell `€` in UTF-8, but the file says ISO-8859-15, so Ruby reads three
        // characters where UTF-8 would read one. Every column after it moves by two.
        let directory = tempdir().unwrap();
        let mut bytes = b"# coding: ISO-8859-15\nx = \"".to_vec();
        bytes.extend_from_slice("\u{20ac}".as_bytes());
        bytes.extend_from_slice(b"\" ; y  = 1\n");
        let path = directory.path().join("cols.rb");
        std::fs::write(&path, bytes).unwrap();

        let Decoded::Text(text) = decoded_source(&path).unwrap() else {
            panic!("ISO-8859-15 maps every byte, so this decodes");
        };

        // Reading the same bytes as UTF-8 gives 16 characters; the declaration makes it 18, and
        // that difference of two is exactly what upstream's columns show after the `€`.
        let as_utf8 = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(as_utf8.lines().nth(1).unwrap().chars().count(), 16);
        assert_eq!(text.lines().nth(1).unwrap().chars().count(), 18);
    }
}

#[cfg(test)]
mod fail_fast_order_tests {
    use super::*;

    #[test]
    fn recorded_ruby_sort_order_is_preserved() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/fail_fast_sort.json")).unwrap();
        let cases = document["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 720);
        let mut checked = 0;
        for (number, row) in cases.iter().enumerate() {
            // WindowsのCRT fallbackを他libcの期待へ流用しない。POSIX極端Epochは未測定。
            if !cfg!(windows) && row["uniform"] != true {
                continue;
            }
            let mut entries = row["keys"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, key)| MtimeEntry {
                    seconds: -key.as_i64().unwrap(),
                    index,
                })
                .collect::<Vec<_>>();
            sort_entries(&mut entries, c_long::BITS);
            let indices = entries
                .iter()
                .map(|entry| entry.index as u64)
                .collect::<Vec<_>>();
            assert_eq!(
                serde_json::json!(indices),
                row["indices"],
                "reference {number}"
            );
            checked += 1;
        }
        assert_eq!(checked, if cfg!(windows) { 720 } else { 403 });
    }

    #[test]
    fn ruby_c_long_boundaries_do_not_overflow() {
        assert!(mtime_key_is_fixnum(1 << 30, 32));
        assert!(!mtime_key_is_fixnum((1 << 30) + 1, 32));
        assert!(mtime_key_is_fixnum(-((1 << 30) - 1), 32));
        assert!(!mtime_key_is_fixnum(-(1 << 30), 32));
        assert!(!mtime_key_is_fixnum(i64::MIN, 64));
        assert!(!mtime_key_is_fixnum(i64::MAX, 64));
    }
}
