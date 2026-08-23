use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tree_sitter::Parser;

use crate::config::{Config, ConfigStore};
use crate::cop_name::selector_matches;
use crate::diagnostic::{FileReport, Location, Offense, OffenseSnapshot, Severity};
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
}

/// 4: the stat a report is keyed on records the file's permission bits as well as its size and
/// modification time. One session index holds every path-keyed report in memory. Reports still
/// carry that stat, the text's digest, and each offense's frozen location so an unchanged file
/// need not be read at all.
const RESULT_CACHE_SCHEMA: u32 = 4;
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
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CachedReport {
    schema: u32,
    /// What the file measured when the report was made. A run that finds all three unchanged serves
    /// the report without opening the file, which is the whole point of keying on the path rather
    /// than on the text.
    size: u64,
    modified: Option<(u64, u32)>,
    /// The permission bits, which `Lint/ScriptPermission` reads from the file itself. `chmod` moves
    /// neither the size nor the modification time nor a single byte of the text, so without this
    /// neither the stat nor the digest can tell that the answer has changed.
    mode: u32,
    /// The digest of the text the report was made from, for when the stat moved but the text did
    /// not -- a checkout that restores a file, a formatter that rewrites it byte for byte.
    text: [u8; 32],
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
        for cached in self.offenses {
            let cop_name = rules().find(|rule| rule.name == cached.cop_name)?.name;
            let severity = Severity::parse(&cached.severity)?;
            let mut offense =
                Offense::new(cop_name, severity, cached.message, cached.start, cached.end);
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
        })
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

    /// Where a file's report lives, which depends on the file's name and the configuration but not
    /// on its contents.
    ///
    /// Keying on the text instead would mean hashing every file before its report could even be
    /// looked for -- and worse, reading every file to have something to hash. The entry carries the
    /// text's digest so a stale one is still recognized; what the path key buys is the chance to
    /// recognize a *fresh* one from the directory entry alone.
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

    /// The stored report for `path`, if there is one, together with what still has to be checked
    /// before it can be trusted.
    ///
    /// The caller has not read the file at this point, and a `Fresh` answer means it never has to.
    pub(crate) fn load(&self, path: &Path, config: &Config) -> Option<Cached> {
        if self.prepared.get() != Some(&true) {
            return None;
        }
        let key = crate::profile::phase(crate::profile::Phase::CacheKey, || {
            self.key(path, config).map(|key| key.to_hex().to_string())
        })?;
        crate::profile::phase(crate::profile::Phase::CacheLoad, || {
            let entries = self.entries.read().ok()?;
            let cached = entries.as_ref()?.get(&key)?.report.clone();
            if cached.schema != RESULT_CACHE_SCHEMA {
                return None;
            }
            let stat = fs::metadata(path).ok().as_ref().map(Self::stat)?;
            // A missing modification time never matches a stored one, so such a file always goes on
            // to be read and digested rather than being trusted on its size alone.
            if cached.size == stat.size
                && stat.modified.is_some()
                && cached.modified == stat.modified
                && cached.mode == stat.mode
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

    /// Records `report` under the stat and digest the file had **when the run read it**.
    ///
    /// Both are the caller's to supply, and neither can be recovered here. The stat has to be the
    /// one taken *before* the read: taking it afterwards pairs the report of the old bytes with the
    /// stat of the new ones whenever a rewrite lands in between, and the next run then accepts that
    /// as fresh on the stat alone without ever reaching the digest. Taken before, such a rewrite
    /// leaves a stat the next run rejects, and the digest settles it from there.
    ///
    /// The digest likewise belongs to the bytes on disk. `report.source` has been through
    /// [`crate::nul_bytes::as_ruby_reads_it`], which rewrites the text; hashing that would make two
    /// different files share a digest and would defeat the guard below, since the rewrite is what
    /// removes the NUL.
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
                    // Resolved here, where the text is still at hand, because a run served from a
                    // stat match has no text to resolve it against.
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
    if serde_json::to_writer(temporary.as_file_mut(), index).is_err()
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
                offense.corrections.clear();
                offense.correctable = false;
            }
            return;
        }
        if !self.safe_autocorrect {
            for offense in reported {
                for correction in &mut offense.corrections {
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

pub fn inspect_source(
    path: impl Into<PathBuf>,
    text: String,
    config: &Config,
    selection: &Selection,
) -> Result<FileReport> {
    inspect_planned(
        path,
        text,
        config,
        selection,
        &RulePlan::build(config, selection),
        true,
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
) -> Result<FileReport> {
    // Settled before the file is parsed, so every cop sees the source Ruby would have read rather
    // than only the one that reports the parse.
    let length_as_read = text.len();
    let text = crate::nul_bytes::as_ruby_reads_it(&text).unwrap_or(text);
    let source = SourceFile::new(path, text).read_as_long_as(length_as_read);
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .context("failed to initialize the Ruby parser")?;
    let tree = crate::profile::phase(crate::profile::Phase::Parse, || {
        parser.parse(source.text(), None)
    })
    .context("Ruby parser returned no syntax tree")?;
    let ast = crate::profile::phase(crate::profile::Phase::Index, || {
        AstIndex::new(tree.root_node())
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
    .with_disabled_cops(&plan.standby_names);
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
    // Most runs resolve every file to the store's root configuration, so the plan for it is worth
    // building once. A file that a nested `.rubocop.yml` gives a different configuration falls back
    // to building its own, which costs no more than resolving the cops inline would have.
    let root_plan = RulePlan::build(configs.root(), selection);
    let inspect = |path: &PathBuf| -> Result<FileReport> {
        let config = configs.for_path(path)?;
        // Asked before the file is read, because the answer decides whether it has to be. A cached
        // report whose file has not moved stands on the directory entry alone.
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
        )?;
        if let Some(cache) = cache
            && cacheable
            && let Some(stat) = stat
        {
            cache.store(&report, stat, &digest, &config);
        }
        Ok(report)
    };
    // Collecting every outcome rather than short-circuiting keeps the surfaced error the first one
    // in path order instead of whichever thread rayon happened to finish first.
    let inspected: Vec<Result<FileReport>> = if parallel && paths.len() > 1 {
        paths.par_iter().map(inspect).collect()
    } else {
        paths.iter().map(inspect).collect()
    };
    let mut reports = inspected.into_iter().collect::<Result<Vec<_>>>()?;
    reports.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(reports)
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

/// Ruby's names for "no encoding at all", where one byte is one character.
fn is_binary_label(label: &str) -> bool {
    matches!(
        label.to_ascii_lowercase().as_str(),
        "binary" | "ascii-8bit" | "ascii8bit"
    )
}

/// Resolves an encoding name the way Ruby spells it. `encoding_rs` answers to the WHATWG label
/// registry, which covers the names a browser sees but not the code page names Ruby also accepts,
/// so those are mapped onto the equivalent registry label first.
fn encoding_for_ruby_label(label: &str) -> Option<&'static encoding_rs::Encoding> {
    if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
        return Some(encoding);
    }
    let alias = match label.to_ascii_lowercase().as_str() {
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

pub fn discover_targets_with_store(
    arguments: &[PathBuf],
    cwd: &Path,
    configs: &ConfigStore,
    force_exclusion: bool,
    only_recognized_file_types: bool,
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
            let config = configs.for_path(&root)?;
            let recognized = config.path_included(&root) || has_ruby_shebang(&root);
            if (!force_exclusion || !config.path_excluded(&root))
                && (!only_recognized_file_types || recognized)
            {
                targets.push(root);
            }
            continue;
        }

        // RuboCop resolves targets from `AllCops/Include` and `AllCops/Exclude` alone and never reads
        // `.gitignore`. Honouring it here would silently drop files that git itself still tracks —
        // a checkout whose `.gitignore` lists `bin/*` for binstubs keeps a committed `bin/console`,
        // and the ignore crate has no view of the index to notice that.
        // RuboCop globs with `File::FNM_DOTMATCH`, so a hidden file under a visible directory is a
        // target like any other; only a path whose *first* component is hidden is shortcut away, and
        // `Config::path_included` does that. What keeps the walk cheap is pruning the directories
        // the configuration excludes outright, which is what upstream's `wanted_dir_patterns` does
        // before it descends.
        let mut walked = Vec::new();
        let pruned = configs.root().clone();
        let mut builder = WalkBuilder::new(&root);
        builder
            .filter_entry(move |entry| {
                !entry.file_type().is_some_and(|kind| kind.is_dir())
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
            let config = crate::profile::phase(crate::profile::Phase::ConfigLookup, || {
                configs.for_path(path)
            })?;
            let verdict = crate::profile::phase(crate::profile::Phase::PathMatch, || {
                let included = config.path_included(path);
                if config.path_excluded(path) {
                    return Verdict::Drop;
                }
                if included {
                    return Verdict::Keep;
                }
                if config.path_hidden(path) {
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
        // Only what a directory expanded to is put in order. RuboCop keeps the arguments themselves
        // in the order they were given, so `sonicop b.rb a.rb` inspects `b.rb` first -- which is what
        // an editor passing one file at a time, or a script feeding a worklist, relies on.
        walked.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
        targets.append(&mut walked);
    }

    let mut seen = HashSet::new();
    targets.retain(|target| seen.insert(target.clone()));
    Ok(targets)
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
#[derive(Clone, Debug)]
struct Action {
    begin_pos: usize,
    end_pos: usize,
    insert_before: String,
    replacement: Option<String>,
    insert_after: String,
    children: Vec<Action>,
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

    /// Upstream's `Action#with`, which drops the children of any action that carries a replacement
    /// -- the replacement covers their whole range, so the text they acted on is gone. Losing a
    /// deletion that way is silent; losing an insertion is the `swallowed_insertions` clobbering
    /// that RuboCop's `Corrector` asks to be raised.
    fn with(&self, children: Vec<Action>, replacement: Option<String>) -> Combined {
        let children = match replacement.is_some() {
            true if children.iter().any(Action::inserts) => return Err(Clobbering),
            true => Vec::new(),
            false => children,
        };
        Ok(Self {
            replacement,
            children,
            ..self.clone()
        })
    }

    /// Two actions on the same range. The later insertion wraps around the earlier one, and a
    /// replacement supersedes an absent one -- but RuboCop asks for `different_replacements` to be
    /// raised, so two cops replacing the same text with different text clobber.
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
        let merged = Self {
            insert_before: format!("{}{}", action.insert_before, self.insert_before),
            insert_after: format!("{}{}", self.insert_after, action.insert_after),
            ..self.clone()
        }
        .with(self.children.clone(), replacement)?;
        merged.combine_children(&action.children)
    }

    fn combine_children(self, children: &[Action]) -> Combined {
        children
            .iter()
            .try_fold(self, |parent, child| parent.place_in_hierarchy(child))
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

    fn ordered_replacements<'a>(&'a self, replacements: &mut Vec<(usize, usize, &'a str)>) {
        if !self.insert_before.is_empty() {
            replacements.push((self.begin_pos, self.begin_pos, &self.insert_before));
        }
        if let Some(replacement) = &self.replacement {
            replacements.push((self.begin_pos, self.end_pos, replacement));
        }
        for child in &self.children {
            child.ordered_replacements(replacements);
        }
        if !self.insert_after.is_empty() {
            replacements.push((self.end_pos, self.end_pos, &self.insert_after));
        }
    }

    fn rewrite(&self, source: &str) -> String {
        let mut replacements = Vec::new();
        self.ordered_replacements(&mut replacements);
        let mut text = String::with_capacity(source.len());
        let mut last_end = 0;
        for (begin, end, replacement) in replacements {
            debug_assert!(
                begin >= last_end,
                "the action tree yielded overlapping edits"
            );
            text.push_str(&source[last_end..begin.max(last_end)]);
            text.push_str(replacement);
            last_end = end.max(last_end);
        }
        text.push_str(&source[last_end..]);
        text
    }
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

/// Where a cop's corrections sit in the run's merge order.
///
/// RuboCop gives every cop its own corrector and merges them into the run's one at a time, in the
/// order the registry lists them: departments in the order `rubocop.rb` requires them, then the
/// cops of a department in the order they register, which is alphabetical bar a handful loaded
/// ahead of their alphabetical place so that a base class is defined before its subclass.
fn cop_merge_order(cop_name: &str) -> (usize, &str) {
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
    (index, cop_name)
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
        .corrections
        .iter()
        .all(|edit| is_addressable(edit.start, edit.end, source))
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
    if mode == CorrectMode::None {
        return (report.source.text().to_owned(), 0);
    }
    let source = report.source.text();

    let candidates = correction_candidates(report, mode, correcting, source);

    let mut run = Action::root();
    // Offenses whose own cop accepted their edits. RuboCop stamps an offense corrected while the
    // cop is filling its corrector, before the team decides whether to take it, so an offense a
    // skip or a clash later denies is still reported as corrected.
    let mut corrected: Vec<usize> = Vec::new();
    // Offenses whose edits actually reached the run's corrector, which is what says the pass
    // changed anything and another one is worth running.
    let mut applied = 0;
    // `Team#each_corrector`'s skip set. See [`autocorrect_incompatible_with`].
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
        corrected.extend(&placed);
        // `Team#each_corrector` reads the corrector before merging it, so a cop that filled one
        // bars the cops it declared itself incompatible with whatever the merge then does.
        skips.extend(autocorrect_incompatible_with(cop_name));

        // A cop an earlier one declared itself incompatible with is passed over: its corrections
        // wait for the pass after the one that provoked the incompatibility.
        if skipped {
            trace_outcome("skip ", cop_name);
            continue;
        }
        // `Team#merge_corrector!`: a cop whose corrections clash with what is already scheduled
        // loses every correction it asked for in this file, not just the one that clashed.
        match run.clone().combine_children(&cop.children) {
            Ok(merged) => {
                run = merged;
                applied += placed.len();
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

    for index in &corrected {
        // Edits a cop scheduled outside `add_offense` leave the offense's own status alone. See
        // [`Offense::corrected_without_status`].
        if !report.offenses[*index].corrections_detached {
            report.offenses[*index].corrected = true;
        }
    }
    (run.rewrite(source), applied)
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
                && !offense.corrections.is_empty()
                && (mode == CorrectMode::All || offense.corrections.iter().all(|edit| edit.safe))
                && edits_are_addressable(offense, source)
        })
        .map(|(index, _)| index)
        .collect();
    // Offenses arrive ordered by position; a stable sort by cop leaves them that way within a cop.
    candidates.sort_by_key(|index| cop_merge_order(report.offenses[*index].cop_name));
    candidates
}

/// Builds one cop's corrector and returns the offenses whose edits it accepted.
fn build_cop_corrector(report: &FileReport, group: &[usize], source: &str) -> (Action, Vec<usize>) {
    // An offense that cannot be placed is the cop error RuboCop reports and steps over, so it costs
    // that offense alone rather than discarding the rest of the cop's corrections.
    let mut cop = Action::root();
    let mut placed = Vec::new();
    for &index in group {
        if trace::enabled() {
            for edit in &report.offenses[index].corrections {
                eprintln!(
                    "      {:16} {:?}",
                    trace::span(source, edit.start, edit.end),
                    edit.replacement
                );
            }
        }
        // `combine` rather than `combine_children`: it is the entry point that drops an edit asking
        // for nothing at all, the way `Corrector#replace` and friends do.
        let anchor = anchor_range(&report.offenses[index], source);
        let offense = report.offenses[index]
            .corrections
            .iter()
            .try_fold(Action::root(), |tree, edit| {
                tree.combine(&Action::from_edit(edit, anchor))
            });
        let Ok(offense) = offense else {
            if trace::enabled() {
                eprintln!("      ★ この offense の中で衝突");
            }
            // The guard that names a corrector written twice over. Reaching it from here covers
            // every cop; the cop-side path only sees the four that reparse their own correction.
            trace::overlap(report, index, "offense-tree");
            continue;
        };
        if offense.children.is_empty() {
            continue;
        }
        let Ok(merged) = cop.clone().combine_children(&offense.children) else {
            if trace::enabled() {
                eprintln!("      ★ cop の corrector に入らなかった (この offense だけ捨てた)");
            }
            trace::overlap(report, index, "cop-tree");
            continue;
        };
        cop = merged;
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

    fn root_cause(&self, loop_start: usize) -> String {
        self.cops_by_pass
            .get(loop_start..)
            .unwrap_or_default()
            .iter()
            .map(|cops| cops.join(", "))
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

/// The result of driving autocorrect to a fixed point.
pub struct CorrectionOutcome {
    pub report: FileReport,
    pub text: String,
    pub corrected_count: usize,
    /// `Team#updated_source_file?`: whether any pass produced text differing from what it ran on,
    /// which is what decides the file is written back. Not every rewrite is credited to an offense
    /// -- see [`Offense::corrected_without_status`] -- so the count of corrected offenses cannot
    /// stand in for it.
    pub rewritten: bool,
    /// Set when the passes never settled. RuboCop reports this per file, still writes the last
    /// corrected text, and keeps inspecting the rest of the run.
    pub infinite_loop: Option<String>,
    /// Set when the corrections were withheld because writing them would have left a file Ruby can
    /// no longer parse. The text is then the source exactly as it was read.
    ///
    /// This is not an offense. Reporting it as `Lint/Syntax` would point at a syntax error the
    /// reader cannot find, because the file on disk is the one that parses. It is an autocorrect
    /// failure: nothing was corrected, and the run must not end in success.
    pub rollback: Option<String>,
}

/// Switches off the guard below. **Measurement only** -- see `withhold_unparsable`.
///
/// It is read from the environment rather than the configuration on purpose: `.rubocop.yml` is
/// committed and shared, so a project could turn the guard off for everyone who checks it out.
pub const NO_SYNTAX_GUARD: &str = "SONICOP_NO_SYNTAX_GUARD";

/// Whether a report carries the fatal `Lint/Syntax` offense that says the source did not parse.
fn holds_fatal_syntax(report: &FileReport) -> bool {
    report
        .offenses
        .iter()
        .any(|offense| offense.severity == Severity::Fatal && offense.cop_name == "Lint/Syntax")
}

/// Refuses a correction that would leave the file unparsable, handing back the source as it was.
///
/// A cop can produce text Ruby rejects, and RuboCop writes it: `Layout/LineLength` folds a line
/// that opens a heredoc and the body ends up before the rest of the statement. Both tools then
/// agree byte for byte on a file that no longer loads, so the `-A` comparison calls it a match.
/// **A byte match says "the same as upstream", not "correct".**
///
/// The guard is deliberately narrow. It asks only whether a source that parsed before parses
/// after, so it cannot mistake a pre-existing error for one the correction introduced, and it
/// cannot object to anything but the one failure it can name.
///
/// It also asks the wrong parser. Ruby raises `SyntaxError` for rules this grammar does not
/// model, so text it accepts can still be rejected by Ruby — measured cases: a dynamic constant
/// assignment (`def a; X = 1; end`), a repeated parameter name (`def a(x, x)`), and `break`,
/// `next`, `redo` or `retry` where no loop or block encloses it. Each of those is a hole in the
/// guard. So this **prevents the destructive writes it can detect**; it does not make correction
/// safe in general, and the phrase to avoid when describing it is "guaranteed".
///
/// The corrections are dropped rather than reported as applied: the file on disk is the original,
/// so calling them corrected would describe a state that does not exist anywhere.
///
/// `SONICOP_NO_SYNTAX_GUARD` turns the guard off. It exists because the guard fires exactly when
/// this parser rejects the correction, which is not the same question as whether Ruby rejects it:
/// the gap between the two parsers is the guard's false-positive rate, and with the guard on, the
/// text needed to measure that rate never reaches disk. **A guard that cannot be switched off is a
/// guard whose error rate cannot be quoted.** It is for measurement, not for use.
fn withhold_unparsable(
    outcome: CorrectionOutcome,
    original: &str,
    started_valid: bool,
    config: &Config,
    selection: &Selection,
    plan: &RulePlan,
) -> Result<CorrectionOutcome> {
    if !started_valid || !outcome.rewritten || !holds_fatal_syntax(&outcome.report) {
        return Ok(outcome);
    }
    if selection.skip_syntax_guard || std::env::var_os(NO_SYNTAX_GUARD).is_some() {
        return Ok(outcome);
    }
    let path = outcome.report.path.clone();
    // The report describes text that will never exist on disk. Inspecting the original again is
    // what the caller would have got with correction turned off, which is the state being kept.
    let report = inspect_planned(
        path.clone(),
        original.to_owned(),
        config,
        selection,
        plan,
        false,
    )?;
    Ok(CorrectionOutcome {
        report,
        text: original.to_owned(),
        corrected_count: 0,
        rewritten: false,
        infinite_loop: outcome.infinite_loop,
        rollback: Some(format!(
            "Autocorrection was not written to {} because it introduced a syntax error.",
            path.display()
        )),
    })
}

pub fn correct_file(
    mut report: FileReport,
    mode: CorrectMode,
    config: &Config,
    selection: &Selection,
) -> Result<CorrectionOutcome> {
    let mut text = report.source.text().to_owned();
    if mode == CorrectMode::None {
        return Ok(CorrectionOutcome {
            report,
            text,
            corrected_count: 0,
            rewritten: false,
            infinite_loop: None,
            rollback: None,
        });
    }

    let original = text.clone();
    // A file the parser already rejected cannot be made worse by correcting it, and RuboCop does
    // correct such files. Only a source that started out valid is protected.
    let started_valid = !holds_fatal_syntax(&report);
    let path = report.path.clone();
    let mut log = CorrectionLog::default();
    let mut sources = vec![digest(&text)];
    let mut rewritten = false;
    // Every pass re-inspects the same file under the same configuration, so the plan is resolved
    // once for the whole fixed-point loop.
    let plan = RulePlan::build(config, selection);
    // `Runner#add_redundant_disables` runs once, after the loop has settled, and the loop that
    // follows it does not carry the directive cop at all.
    let mut directives_pending = plan.checks_directives();
    // `Runner#loop_until_no_offense` raises once a **201st** pass is asked for, so 200 of them
    // actually run. A cop that grows the file every time leaves exactly that many marks behind, and
    // an off-by-one here is visible in the file it gives up on.
    for pass in 0..MAX_CORRECTION_PASSES {
        let (mut corrected, mut count) =
            corrected_text(&mut report, mode, Correcting::ExceptDirectives);
        let mut directive_pass = false;
        if count == 0 && directives_pending {
            directives_pending = false;
            directive_pass = true;
            (corrected, count) = corrected_text(&mut report, mode, Correcting::DirectivesOnly);
        }
        if count == 0 {
            let (report, corrected_count) = log.merge_into(report);
            return withhold_unparsable(
                CorrectionOutcome {
                    report,
                    text,
                    corrected_count,
                    rewritten,
                    infinite_loop: None,
                    rollback: None,
                },
                &original,
                started_valid,
                config,
                selection,
                &plan,
            );
        }
        log.record_pass(&mut report, directive_pass);
        rewritten |= corrected != text;

        // Re-producing a source seen before means the passes are trading edits back and forth; the
        // repeat tells us which pass the cycle closed on.
        let corrected_digest = digest(&corrected);
        let repeated = sources.iter().position(|seen| *seen == corrected_digest);
        if pass + 1 == MAX_CORRECTION_PASSES || repeated.is_some() {
            let loop_start = repeated.unwrap_or_else(|| log.cops_by_pass.len().saturating_sub(1));
            let root_cause = log.root_cause(loop_start);
            let (report, corrected_count) = log.merge_into(report);
            return withhold_unparsable(
                CorrectionOutcome {
                    report,
                    text: corrected,
                    corrected_count,
                    rewritten,
                    infinite_loop: Some(format!(
                        "Infinite loop detected in {} and caused by {root_cause}",
                        path.display()
                    )),
                    rollback: None,
                },
                &original,
                started_valid,
                config,
                selection,
                &plan,
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

/// The bytes to write back for corrected source, in the encoding the file was read in.
///
/// This is the one place Sonicop knowingly departs from RuboCop. RuboCop's runner ends in a plain
/// `File.write`, so a corrected Shift_JIS file comes back out as UTF-8 while its magic comment still
/// claims Shift_JIS -- a file that no longer loads. Reproducing that faithfully would mean shipping
/// data loss on purpose, which is further than drop-in compatibility reaches. The divergence is
/// recorded in `tests/conformance/known_divergences.yml`.
///
/// That protection only applies to a file that named an encoding where the reader could see it.
/// [`decoded_source`] reads the declaration from the first line (the second under a shebang) and
/// nowhere else, but `Lint/OrderedMagicComments` can move one onto the first line during the
/// correction loop -- so the corrected text can carry a declaration the reader never applied.
/// `decoded_as_declared` asks the file on disk, and is only asked once a declaration is found.
/// Encoding such a file to the label it names rewrites bytes no cop asked to change: rails'
/// `1_currencies_have_symbols.rb` declares `ISO-8859-15` and holds a UTF-8 `€`, and turning those
/// three bytes into `\xa4` changes what the program says as surely as editing the literal would.
///
/// `Err` when the correction cannot be represented in that encoding, so the caller leaves the file
/// alone rather than writing a lossy approximation.
fn output_bytes(contents: &str, decoded_as_declared: impl FnOnce() -> bool) -> Result<Vec<u8>> {
    let Some(label) = encoding_declaration(contents) else {
        return Ok(contents.as_bytes().to_vec());
    };
    // A binary source was read one byte to one character, so it goes back out the same way.
    if is_binary_label(&label) {
        return match contents.chars().all(|character| (character as u32) < 0x100) {
            true => Ok(contents.chars().map(|character| character as u8).collect()),
            false => bail!("the correction cannot be written back as {label}"),
        };
    }
    let Some(encoding) = encoding_for_ruby_label(&label) else {
        return Ok(contents.as_bytes().to_vec());
    };
    if encoding == encoding_rs::UTF_8 || !decoded_as_declared() {
        return Ok(contents.as_bytes().to_vec());
    }
    let (bytes, _, unmappable) = encoding.encode(contents);
    match unmappable {
        // `encode` substitutes what it cannot represent, so writing this would silently corrupt the
        // very characters the correction was supposed to leave alone.
        true => bail!("the correction cannot be written back as {label}"),
        false => Ok(bytes.into_owned()),
    }
}

/// Writes corrected source back over the file it was read from.
///
/// Upstream ends in `File.write(path, ...)` (`cop/team.rb:180`), which is `O_WRONLY|O_CREAT|O_TRUNC`
/// **on the path itself**. That matters twice, and neither is incidental: the open follows a
/// symlink, so a corrected symlink stays a symlink and its target is what gets rewritten; and it
/// keeps the inode, so every other name hard-linked to it sees the correction too.
///
/// Writing a temporary file beside the path and `rename(2)`-ing it over instead is atomic -- a
/// killed writer cannot leave a half-written file -- but rename replaces the *directory entry*.
/// A symlink is replaced by a regular file holding the corrected text while its target keeps the
/// original, and a hard-linked inode is unlinked, dropping the link count and leaving every other
/// name on the old text. Both are silent data loss in a repository laid out with shared files, and
/// no message anywhere says the link is gone.
///
/// **So the two properties are traded per file rather than one chosen for all of them.** A plain
/// file with a single link has no identity a rename can destroy, and that is nearly every file in
/// nearly every run, so it keeps the atomic path. A symlink or a multiply-linked inode -- where a
/// rename would be wrong however safe -- is written through in place, upstream's way, accepting
/// that a process killed mid-write leaves it truncated. Correctness first: an interrupted write is
/// recoverable from version control, a deleted hard link is not obviously *there* to recover.
///
/// Permissions survive either way: the temporary file is given the original's before it is
/// persisted, and the in-place write never creates a new file to give permissions to.
pub fn write_corrected(path: &Path, contents: &str) -> Result<()> {
    // The file on disk is still the one that was read: the loop corrects in memory and writes once.
    // What matters is whether the file *as read* named its own encoding, not whether the corrected
    // text does: `Lint/OrderedMagicComments` can lift a declaration onto the first line, where the
    // reader never saw it.
    let bytes = output_bytes(contents, || {
        fs::read(path).is_ok_and(|bytes| declared_label(&bytes).is_some())
    })
    .with_context(|| format!("refusing to rewrite {}", path.display()))?;
    if rename_would_lose_the_file(path) {
        return write_in_place(path, &bytes);
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temporary file beside {}", path.display()))?;
    temporary
        .write_all(&bytes)
        .with_context(|| format!("failed to write corrected contents for {}", path.display()))?;
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("failed to flush corrected contents for {}", path.display()))?;
    if let Some(permissions) = permissions {
        temporary
            .as_file()
            .set_permissions(permissions)
            .with_context(|| format!("failed to preserve permissions for {}", path.display()))?;
    }
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {} atomically", path.display()))?;
    Ok(())
}

/// Whether replacing `path`'s directory entry would destroy something the entry does not own: the
/// symlink itself, or the other names sharing its inode.
///
/// `symlink_metadata` is what asks the first question -- `metadata` follows the link and would
/// report on the target, which is exactly the file a rename does *not* touch. A read that fails
/// answers no, which routes the write through the atomic path; it then reports the same IO failure
/// with a message of its own rather than this deciding anything on a guess.
fn rename_would_lose_the_file(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_symlink() || link_count(&metadata) > 1
}

/// How many names the file has. Windows keeps hard links too but exposes no count through `std`,
/// so it answers 1 and every write there takes the atomic path -- which is what it did before this
/// distinction existed.
#[cfg(unix)]
fn link_count(metadata: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(metadata)
}

#[cfg(not(unix))]
fn link_count(_metadata: &fs::Metadata) -> u64 {
    1
}

/// `File.write(path, ...)`: truncate what the path resolves to and write the corrected bytes into
/// it, keeping the inode and following any symlink on the way.
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
    use crate::diagnostic::{Edit, FileReport, Offense, Severity};
    use crate::source::SourceFile;

    use super::{
        CorrectMode, Correcting, ResultCache, Selection, correct_file, corrected_text,
        discover_targets, inspect_files, inspect_source, write_corrected,
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

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

    /// Every expectation below is what `Parser::Source::TreeRewriter` produces under the policies
    /// RuboCop's `Corrector` sets (`crossing_deletions: :accept`, `different_replacements: :raise`,
    /// `swallowed_insertions: :raise`), merged one cop at a time the way `Team` merges them.
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
        // Departments merge in the order `rubocop.rb` requires them and cops alphabetically within
        // one, so the insertion RuboCop schedules first is the one that ends up innermost -- and a
        // replacement reaching over an insertion loses to whichever was scheduled first.
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

        let bytes = output_bytes(corrected, || true).unwrap();

        assert!(bytes.ends_with(b"x = '\x93\xfa\x96\x7b'\n"));
    }

    #[test]
    fn a_correction_that_the_declared_encoding_cannot_hold_is_refused() {
        // Nothing in cp932 stands for an emoji, and substituting one silently would corrupt the
        // very text the correction was meant to leave alone.
        let corrected = "# encoding: cp932\nx = '\u{1f363}'\n";

        assert!(output_bytes(corrected, || true).is_err());
    }

    #[test]
    fn a_utf8_file_that_names_another_encoding_goes_back_out_unchanged() {
        // rails' `1_currencies_have_symbols.rb`: the comment says ISO-8859-15, the bytes are UTF-8,
        // and `decoded_source` read it as UTF-8. Encoding the `€` to `\xa4` on the way out would
        // turn a three-character string into a one-character one -- a change no cop asked for.
        let corrected = "# coding: ISO-8859-15\nx = '\u{20ac}'\n";

        let bytes = output_bytes(corrected, || false).unwrap();

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

    /// Every name on an inode is the file. Unlinking it to put another in its place leaves the
    /// other names on text the run has already reported as corrected.
    #[cfg(unix)]
    #[test]
    fn write_corrected_keeps_a_hard_linked_inode_shared() {
        let directory = tempdir().unwrap();
        let first = directory.path().join("one.rb");
        let second = directory.path().join("two.rb");
        std::fs::write(&first, "y  = 2\n").unwrap();
        std::fs::hard_link(&first, &second).unwrap();

        write_corrected(&first, "y = 2\n").unwrap();

        assert_eq!(
            std::os::unix::fs::MetadataExt::nlink(&std::fs::metadata(&first).unwrap()),
            2,
            "the inode was unlinked and the second name left behind"
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            "y = 2\n",
            "the other name for the same file still holds the old text"
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
