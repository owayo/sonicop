use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use rayon::prelude::*;
use tempfile::NamedTempFile;
use tree_sitter::Parser;

use crate::config::{Config, ConfigStore};
use crate::cop_name::selector_matches;
use crate::diagnostic::{FileReport, Offense, Severity};
use crate::directives::DirectiveState;
use crate::magic_comment::MagicComment;
use crate::rules::{AstIndex, Rule, RuleContext, rules};
use crate::source::SourceFile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectMode {
    None,
    Safe,
    All,
}

#[derive(Clone, Debug, Default)]
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
}

/// RuboCop refuses to let syntax checking be turned off, so the cop stays on no matter how it is
/// selected away. Both the `--except` guard and cop selection have to agree on the names that
/// denote it, including the legacy `Syntax` spelling RuboCop still accepts.
pub fn is_mandatory_cop(name: &str) -> bool {
    matches!(name, "Lint/Syntax" | "Syntax")
}

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
}

/// The cops a run applies, with every configuration decision that does not depend on the file
/// resolved once.
///
/// Resolving `Enabled`, `Severity` and `SafeAutoCorrect` out of YAML costs a lookup per cop per
/// file, which is work that grows with the registry as it fills out RuboCop's full cop set. Only
/// `Exclude` reads the path being inspected, so it is all that stays per-file.
pub(crate) struct RulePlan {
    entries: Vec<PlannedRule>,
}

struct PlannedRule {
    rule: &'static Rule,
    /// `rule.severity` unless the configuration overrode it.
    severity: Severity,
    safe_autocorrect: bool,
}

impl RulePlan {
    pub(crate) fn build(config: &Config, selection: &Selection) -> Self {
        let entries = rules()
            .filter(|rule| {
                let enabled = config.rule_enabled_with_pending(
                    rule.name,
                    selection.enable_pending,
                    selection.disable_pending,
                );
                selection.includes(rule.name, enabled, config.rule_safe(rule.name))
            })
            .map(|rule| PlannedRule {
                rule,
                severity: config
                    .cop_value::<String>(rule.name, "Severity")
                    .and_then(|value| Severity::parse(&value))
                    .unwrap_or(rule.severity),
                safe_autocorrect: config.rule_safe_autocorrect(rule.name),
            })
            .collect();
        Self { entries }
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
    )
}

/// Inspects one file against an already-resolved [`RulePlan`], which must have been built from
/// `config` and `selection`.
fn inspect_planned(
    path: impl Into<PathBuf>,
    text: String,
    config: &Config,
    selection: &Selection,
    plan: &RulePlan,
) -> Result<FileReport> {
    let source = SourceFile::new(path, text);
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .context("failed to initialize the Ruby parser")?;
    let tree = parser
        .parse(source.text(), None)
        .context("Ruby parser returned no syntax tree")?;
    let ast = AstIndex::new(tree.root_node());
    let directives = (!selection.ignore_disable_comments)
        .then(|| DirectiveState::parse(&source, ast.comment_ranges()));
    let mut offenses = Vec::new();

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
    (syntax_rule.check)(
        &RuleContext::new(&source, &ast, config, syntax_rule, syntax_severity),
        &mut syntax_offenses,
    );
    let valid_syntax = syntax_offenses.is_empty();

    for planned in &plan.entries {
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
        if !valid_syntax {
            continue;
        }
        let context = RuleContext::new(&source, &ast, config, rule, planned.severity);
        let start = offenses.len();
        (rule.check)(&context, &mut offenses);
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
    sort_offenses(&mut offenses, &source);

    Ok(FileReport {
        path: source.path().to_path_buf(),
        source,
        offenses,
    })
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
    // Most runs resolve every file to the store's root configuration, so the plan for it is worth
    // building once. A file that a nested `.rubocop.yml` gives a different configuration falls back
    // to building its own, which costs no more than resolving the cops inline would have.
    let root_plan = RulePlan::build(configs.root(), selection);
    let inspect = |path: &PathBuf| -> Result<FileReport> {
        let Some(text) = decoded_source(path)? else {
            return Ok(undecodable_report(path));
        };
        let config = configs.for_path(path)?;
        let own_plan = (!std::ptr::eq(Arc::as_ptr(&config), configs.root()))
            .then(|| RulePlan::build(&config, selection));
        inspect_planned(
            path.clone(),
            text,
            &config,
            selection,
            own_plan.as_ref().unwrap_or(&root_plan),
        )
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

/// `None` when the file exists but cannot be decoded. RuboCop reports that as a fatal `Lint/Syntax`
/// offense and inspects the remaining files, so it must not abort the run; a genuine IO failure
/// still does.
fn decoded_source(path: &Path) -> Result<Option<String>> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    // A file declaring itself binary has to be read that way even when its bytes happen to be valid
    // UTF-8: Ruby measures an `ASCII-8BIT` source one byte at a time, so a cop reporting a length or
    // a column over a multibyte sequence counts each byte separately.
    if declared_label(&bytes).is_some_and(|label| is_binary_label(&label)) {
        return Ok(Some(bytes.iter().map(|byte| *byte as char).collect()));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(error) => Ok(decode_declared_encoding(error.as_bytes())),
    }
}

/// The encoding a source names for itself, read loosely: the magic comment is ASCII in every
/// encoding this can resolve, so the opening lines can be scanned before anything is decoded.
fn declared_label(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(&bytes[..bytes.len().min(1024)])
        .lines()
        .take_while(|line| line.trim_start().starts_with('#'))
        .find_map(|line| MagicComment::parse(line).encoding())
}

/// Ruby's names for "no encoding at all", where one byte is one character.
fn is_binary_label(label: &str) -> bool {
    matches!(
        label.to_ascii_lowercase().as_str(),
        "binary" | "ascii-8bit" | "ascii8bit"
    )
}

/// Decodes a file that is not UTF-8 using the encoding its own magic comment names, which is what
/// Ruby's parser does before handing the source to a cop. A file with no such comment -- a Vim
/// `fileencoding` line does not count, since Ruby does not read those -- stays undecodable, and
/// RuboCop reports it as a syntax error rather than guessing.
fn decode_declared_encoding(bytes: &[u8]) -> Option<String> {
    let label = declared_label(bytes)?;
    let encoding = encoding_for_ruby_label(&label)?;
    let (text, _, malformed) = encoding.decode(bytes);
    (!malformed).then(|| text.into_owned())
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

fn undecodable_report(path: &Path) -> FileReport {
    // RuboCop capitalizes the parser's `invalid byte sequence in UTF-8` and anchors the offense at
    // the head of the file, since it never got a syntax tree to locate anything against.
    let mut offense = Offense::new(
        "Lint/Syntax",
        Severity::Fatal,
        "Invalid byte sequence in utf-8.",
        0,
        0,
    );
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
        let pruned = configs.root().clone();
        let mut builder = WalkBuilder::new(&root);
        builder
            .filter_entry(move |entry| {
                !entry.file_type().is_some_and(|kind| kind.is_dir())
                    || !pruned.directory_excluded(entry.path())
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
        for entry in builder.build() {
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
            let config = configs.for_path(path)?;
            let included = config.path_included(path);
            if config.path_excluded(path)
                || (!included && (config.path_hidden(path) || !has_ruby_shebang(path)))
            {
                continue;
            }
            targets.push(normalized_target_path(path));
        }
    }

    targets.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
    targets.dedup();
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

pub fn corrected_text(report: &mut FileReport, mode: CorrectMode) -> (String, usize) {
    if mode == CorrectMode::None {
        return (report.source.text().to_owned(), 0);
    }

    // An offense is corrected whole or not at all: its edits are one rewrite the cop asked for, and
    // applying half of them would leave source the cop never intended to produce.
    let mut candidates: Vec<(usize, Vec<crate::diagnostic::Edit>)> = report
        .offenses
        .iter()
        .enumerate()
        .filter(|(_, offense)| {
            !offense.corrections.is_empty()
                && (mode == CorrectMode::All || offense.corrections.iter().all(|edit| edit.safe))
        })
        .map(|(index, offense)| {
            let mut edits = offense.corrections.clone();
            edits.sort_by_key(|edit| (edit.start, edit.end));
            (index, edits)
        })
        .collect();
    candidates.sort_by_key(|(_, edits)| edits.first().map(|edit| (edit.start, edit.end)));

    let mut selected: Vec<(usize, Vec<crate::diagnostic::Edit>)> = Vec::new();
    let mut occupied_end = 0;
    let mut occupied_insertions = HashSet::new();
    for (index, edits) in candidates {
        let mut taken_insertions = Vec::new();
        let fits = edits.iter().all(|edit| {
            let insertion_conflict = edit.start == edit.end
                && (occupied_insertions.contains(&edit.start)
                    || taken_insertions.contains(&edit.start));
            if edit.start == edit.end {
                taken_insertions.push(edit.start);
            }
            edit.start >= occupied_end && !insertion_conflict
        });
        if !fits {
            continue;
        }
        occupied_end = edits
            .iter()
            .fold(occupied_end, |end, edit| end.max(edit.end));
        occupied_insertions.extend(taken_insertions);
        selected.push((index, edits));
    }

    let mut text = report.source.text().to_owned();
    for (offense_index, edits) in selected.iter().rev() {
        for edit in edits.iter().rev() {
            if edit.start <= edit.end
                && edit.end <= text.len()
                && text.is_char_boundary(edit.start)
                && text.is_char_boundary(edit.end)
            {
                text.replace_range(edit.start..edit.end, &edit.replacement);
                report.offenses[*offense_index].corrected = true;
            }
        }
    }
    let corrected = report
        .offenses
        .iter()
        .filter(|offense| offense.corrected)
        .count();
    (text, corrected)
}

const MAX_CORRECTION_PASSES: usize = 200;

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
    fn record_pass(&mut self, report: &mut FileReport) {
        let source = &report.source;
        let mut cops: Vec<&'static str> = Vec::new();
        for offense in &mut report.offenses {
            if !offense.corrected {
                continue;
            }
            offense.freeze_location(source);
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
    /// Set when the passes never settled. RuboCop reports this per file, still writes the last
    /// corrected text, and keeps inspecting the rest of the run.
    pub infinite_loop: Option<String>,
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
            infinite_loop: None,
        });
    }

    let path = report.path.clone();
    let mut log = CorrectionLog::default();
    let mut sources = vec![text.clone()];
    // Every pass re-inspects the same file under the same configuration, so the plan is resolved
    // once for the whole fixed-point loop.
    let plan = RulePlan::build(config, selection);
    for pass in 0..=MAX_CORRECTION_PASSES {
        let (corrected, count) = corrected_text(&mut report, mode);
        if count == 0 {
            let (report, corrected_count) = log.merge_into(report);
            return Ok(CorrectionOutcome {
                report,
                text,
                corrected_count,
                infinite_loop: None,
            });
        }
        log.record_pass(&mut report);

        // Re-producing a source seen before means the passes are trading edits back and forth; the
        // repeat tells us which pass the cycle closed on.
        let repeated = sources.iter().position(|source| *source == corrected);
        if pass == MAX_CORRECTION_PASSES || repeated.is_some() {
            let loop_start = repeated.unwrap_or_else(|| log.cops_by_pass.len().saturating_sub(1));
            let root_cause = log.root_cause(loop_start);
            let (report, corrected_count) = log.merge_into(report);
            return Ok(CorrectionOutcome {
                report,
                text: corrected,
                corrected_count,
                infinite_loop: Some(format!(
                    "Infinite loop detected in {} and caused by {root_cause}",
                    path.display()
                )),
            });
        }

        sources.push(corrected.clone());
        text = corrected;
        report = inspect_planned(path.clone(), text.clone(), config, selection, &plan)?;
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

/// The bytes to write back for corrected source, in the encoding the file declares for itself.
///
/// This is the one place Sonicop knowingly departs from RuboCop. RuboCop's runner ends in a plain
/// `File.write`, so a corrected Shift_JIS file comes back out as UTF-8 while its magic comment still
/// claims Shift_JIS -- a file that no longer loads. Reproducing that faithfully would mean shipping
/// data loss on purpose, which is further than drop-in compatibility reaches. The divergence is
/// recorded in `tests/conformance/known_divergences.yml`.
///
/// `Err` when the correction cannot be represented in that encoding, so the caller leaves the file
/// alone rather than writing a lossy approximation.
fn output_bytes(contents: &str) -> Result<Vec<u8>> {
    let Some(label) = contents
        .lines()
        .take_while(|line| line.trim_start().starts_with('#'))
        .find_map(|line| MagicComment::parse(line).encoding())
    else {
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
    if encoding == encoding_rs::UTF_8 {
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
    let bytes = output_bytes(contents)
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

    use super::output_bytes;
    use crate::config::Config;
    use crate::diagnostic::Severity;

    use super::{
        CorrectMode, Selection, correct_file, discover_targets, inspect_files, inspect_source,
    };

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

        let bytes = output_bytes(corrected).unwrap();

        assert!(bytes.ends_with(b"x = '\x93\xfa\x96\x7b'\n"));
    }

    #[test]
    fn a_correction_that_the_declared_encoding_cannot_hold_is_refused() {
        // Nothing in cp932 stands for an emoji, and substituting one silently would corrupt the
        // very text the correction was meant to leave alone.
        let corrected = "# encoding: cp932\nx = '\u{1f363}'\n";

        assert!(output_bytes(corrected).is_err());
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
    }
}
