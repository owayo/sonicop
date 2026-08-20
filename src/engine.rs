use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tree_sitter::Parser;

use crate::config::{Config, ConfigStore};
use crate::cop_name::selector_matches;
use crate::diagnostic::{FileReport, Offense, Severity};
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

const RESULT_CACHE_SCHEMA: u32 = 1;

/// Persistent reports for unchanged files.
///
/// A cache identity includes the executable bytes, not only the package version. Development
/// builds often keep the same version while their rules change; keying on the binary prevents a
/// freshly rebuilt linter from accepting reports produced by older code.
pub(crate) struct ResultCache {
    root: PathBuf,
    identity: blake3::Hash,
    max_files: usize,
}

#[derive(Serialize, Deserialize)]
struct CachedReport {
    schema: u32,
    offenses: Vec<CachedOffense>,
}

#[derive(Serialize, Deserialize)]
struct CachedOffense {
    cop_name: String,
    severity: String,
    message: String,
    start: usize,
    end: usize,
    correctable: bool,
    suppressed: bool,
    justification: Option<String>,
}

impl ResultCache {
    pub(crate) fn new(root: PathBuf, selection: &Selection, max_files: usize) -> Result<Self> {
        let mut identity = blake3::Hasher::new();
        hash_part(&mut identity, b"sonicop-result-cache");
        hash_part(&mut identity, &RESULT_CACHE_SCHEMA.to_le_bytes());
        hash_part(&mut identity, crate::VERSION.as_bytes());
        hash_part(
            &mut identity,
            &serde_json::to_vec(selection).context("failed to fingerprint the cop selection")?,
        );
        if let Ok(executable) = std::env::current_exe()
            && let Ok(bytes) = fs::read(executable)
        {
            hash_part(&mut identity, &bytes);
        }
        Ok(Self {
            root,
            identity: identity.finalize(),
            max_files,
        })
    }

    fn key(&self, path: &Path, text: &str, config: &Config) -> Option<blake3::Hash> {
        // A NUL can make Ruby stop reading before the physical end of the file. It is rare and
        // preserving both lengths in the cache buys less than keeping this path unambiguous.
        if text.as_bytes().contains(&0) {
            return None;
        }
        let path = path.to_str()?;
        let config = config.cache_key_material().ok()?;
        let mut key = blake3::Hasher::new();
        hash_part(&mut key, self.identity.as_bytes());
        hash_part(&mut key, path.as_bytes());
        hash_part(&mut key, &config);
        hash_part(&mut key, text.as_bytes());
        Some(key.finalize())
    }

    pub(crate) fn load(&self, path: &Path, text: &str, config: &Config) -> Option<FileReport> {
        let key = crate::profile::phase(crate::profile::Phase::CacheKey, || {
            self.key(path, text, config)
        })?;
        crate::profile::phase(crate::profile::Phase::CacheLoad, || {
            let bytes = fs::read(self.path(key)).ok()?;
            let cached: CachedReport = serde_json::from_slice(&bytes).ok()?;
            if cached.schema != RESULT_CACHE_SCHEMA {
                return None;
            }
            let source = SourceFile::new(path.to_path_buf(), text.to_owned());
            let mut offenses = Vec::with_capacity(cached.offenses.len());
            for cached in cached.offenses {
                let cop_name = rules().find(|rule| rule.name == cached.cop_name)?.name;
                let severity = Severity::parse(&cached.severity)?;
                let mut offense =
                    Offense::new(cop_name, severity, cached.message, cached.start, cached.end);
                offense.correctable = cached.correctable;
                offense.suppressed = cached.suppressed;
                offense.justification = cached.justification;
                offenses.push(offense);
            }
            Some(FileReport {
                path: path.to_path_buf(),
                source,
                offenses,
            })
        })
    }

    pub(crate) fn store(&self, report: &FileReport, config: &Config) {
        let Some(key) = self.key(&report.path, report.source.text(), config) else {
            return;
        };
        if self.max_files == 0 {
            return;
        }
        let cached = CachedReport {
            schema: RESULT_CACHE_SCHEMA,
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
                })
                .collect(),
        };
        let destination = self.path(key);
        let Some(parent) = destination.parent().map(Path::to_path_buf) else {
            return;
        };
        if fs::create_dir_all(&parent).is_err() {
            return;
        }
        let Ok(mut temporary) = NamedTempFile::new_in(&parent) else {
            return;
        };
        if serde_json::to_writer(temporary.as_file_mut(), &cached).is_err()
            || temporary.as_file_mut().flush().is_err()
        {
            return;
        }
        let _ = temporary.persist(destination);
    }

    fn path(&self, key: blake3::Hash) -> PathBuf {
        let key = key.to_hex();
        self.root.join(&key[..2]).join(format!("{key}.json"))
    }

    /// Enforces RuboCop's `AllCops/MaxFilesInCache` after a run, instead of scanning the cache
    /// after every file a parallel run writes. Only the two-level hash layout owned by Sonicop is
    /// eligible for removal, even when the user points `--cache-root` at a shared directory.
    pub(crate) fn prune(&self) {
        let Ok(shards) = fs::read_dir(&self.root) else {
            return;
        };
        let paths = shards
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && name.len() == 2
                    && name
                        .to_str()
                        .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_hexdigit()))
            })
            .filter_map(|shard| fs::read_dir(shard.path()).ok())
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    return false;
                };
                path.extension().and_then(|extension| extension.to_str()) == Some("json")
                    && stem.len() == 64
                    && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        == Some(&stem[..2])
            })
            .collect::<Vec<_>>();
        if paths.len() <= self.max_files {
            return;
        }
        let mut entries = paths
            .into_iter()
            .map(|path| {
                let modified = fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .ok();
                (modified, path)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let excess = entries.len() - self.max_files;
        for (_, path) in entries.into_iter().take(excess) {
            let _ = fs::remove_file(path);
        }
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
            if !planned.safe_autocorrect {
                for offense in &mut reported {
                    for correction in &mut offense.corrections {
                        correction.safe = false;
                    }
                }
            }
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
        if !planned.safe_autocorrect {
            for offense in &mut offenses[start..] {
                for correction in &mut offense.corrections {
                    correction.safe = false;
                }
            }
        }
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
        let text =
            match crate::profile::phase(crate::profile::Phase::Read, || decoded_source(path))? {
                Decoded::Text(text) => text,
                Decoded::Undecodable(message) => return Ok(undecodable_report(path, &message)),
            };
        let config = configs.for_path(path)?;
        if let Some(report) = cache.and_then(|cache| cache.load(path, &text, &config)) {
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
        if let Some(cache) = cache {
            cache.store(&report, &config);
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
    let Some(label) = declared_label(&bytes) else {
        return Ok(utf8_or_invalid(bytes));
    };
    // A file declaring itself binary has to be read that way even when its bytes happen to be valid
    // UTF-8: Ruby measures an `ASCII-8BIT` source one byte at a time, so a cop reporting a length or
    // a column over a multibyte sequence counts each byte separately.
    if is_binary_label(&label) {
        return Ok(Decoded::Text(
            bytes.iter().map(|byte| *byte as char).collect(),
        ));
    }
    // `encoding_rs` answers to the WHATWG registry, which folds `us-ascii` into windows-1252 and so
    // reads every byte happily. Ruby's `US-ASCII` refuses anything above 7 bits, and that refusal is
    // the whole point of the declaration, so it is checked here rather than delegated.
    if is_seven_bit_label(&label) {
        return Ok(match bytes.iter().position(|byte| *byte > 0x7f) {
            Some(index) => {
                Decoded::Undecodable(format!("\"\\x{:02x}\" on us-ascii.", bytes[index]))
            }
            None => Decoded::Text(String::from_utf8(bytes).expect("7-bit bytes are UTF-8")),
        });
    }
    let Some(encoding) = encoding_for_ruby_label(&label) else {
        // The label reaches here already cut at the first `.`, since the magic comment's token
        // pattern holds no dot -- upstream's does not either, so `ANSI_X3.4-1968` is `ansi_x3` to
        // both of us and neither can name it. Cutting it is upstream's behaviour, not a defect to
        // repair: repairing it would resolve an encoding RuboCop reports as unknown.
        return Ok(Decoded::Undecodable(format!(
            "Unknown encoding name - {}.",
            label.to_ascii_lowercase()
        )));
    };
    if encoding == encoding_rs::UTF_8 {
        return Ok(utf8_or_invalid(bytes));
    }
    let (text, _, malformed) = encoding.decode(&bytes);
    Ok(match malformed {
        true => Decoded::Undecodable(INVALID_UTF8.to_owned()),
        false => Decoded::Text(text.into_owned()),
    })
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
        loop {
            let Some(entry) = crate::profile::phase(crate::profile::Phase::Walk, || walker.next())
            else {
                break;
            };
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
    for pass in 0..=MAX_CORRECTION_PASSES {
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
        if pass == MAX_CORRECTION_PASSES || repeated.is_some() {
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

pub fn write_corrected(path: &Path, contents: &str) -> Result<()> {
    // The file on disk is still the one that was read: the loop corrects in memory and writes once.
    // What matters is whether the file *as read* named its own encoding, not whether the corrected
    // text does: `Lint/OrderedMagicComments` can lift a declaration onto the first line, where the
    // reader never saw it.
    let bytes = output_bytes(contents, || {
        fs::read(path).is_ok_and(|bytes| declared_label(&bytes).is_some())
    })
    .with_context(|| format!("refusing to rewrite {}", path.display()))?;
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
    use tempfile::tempdir;

    use super::{Decoded, decoded_source, output_bytes};
    use crate::config::Config;
    use crate::diagnostic::{Edit, FileReport, Offense, Severity};
    use crate::source::SourceFile;

    use super::{
        CorrectMode, Correcting, ResultCache, Selection, correct_file, corrected_text,
        discover_targets, inspect_files, inspect_source,
    };

    /// One cop's corrections: the cop's name, then an offense per inner slice, then the edits that
    /// offense asks for.
    type CopEdits = (
        &'static str,
        &'static [&'static [(usize, usize, &'static str)]],
    );

    #[test]
    fn result_cache_round_trips_reports_and_rejects_changed_source() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.rb");
        let source = "x = 1  \n";
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let cache = ResultCache::new(directory.path().join("cache"), &selection, 100).unwrap();
        let report = inspect_source(path.clone(), source.to_owned(), &config, &selection).unwrap();

        cache.store(&report, &config);
        let cached = cache.load(&path, source, &config).unwrap();

        assert_eq!(cached.source.text(), report.source.text());
        assert_eq!(cached.offenses.len(), 1);
        assert_eq!(cached.offenses[0].cop_name, report.offenses[0].cop_name);
        assert_eq!(cached.offenses[0].message, report.offenses[0].message);
        assert_eq!(
            cached.offenses[0].location(&cached.source).line,
            report.offenses[0].location(&report.source).line
        );
        assert!(cached.offenses[0].is_correctable());
        assert!(cache.load(&path, "x = 1\n", &config).is_none());
    }

    #[test]
    fn result_cache_prunes_to_the_configured_file_limit() {
        let directory = tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let selection = Selection {
            only: vec!["Layout/TrailingWhitespace".to_owned()],
            ..Selection::default()
        };
        let cache = ResultCache::new(directory.path().join("cache"), &selection, 1).unwrap();
        let paths = [
            directory.path().join("first.rb"),
            directory.path().join("second.rb"),
        ];
        for path in &paths {
            let report =
                inspect_source(path.clone(), "x = 1  \n".to_owned(), &config, &selection).unwrap();
            cache.store(&report, &config);
        }

        cache.prune();

        assert_eq!(
            paths
                .iter()
                .filter(|path| cache.load(path, "x = 1  \n", &config).is_some())
                .count(),
            1
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
