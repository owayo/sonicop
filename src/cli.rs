use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{ArgAction, ArgMatches, CommandFactory, FromArgMatches, Parser, error::ErrorKind};

use crate::config::{Config, ConfigStore};
use crate::cop_name::{self, selector_matches};
use crate::diagnostic::{FileReport, Offense, Severity};
use crate::engine::{
    CorrectMode, NO_SYNTAX_GUARD, ResultCache, Selection, correct_file,
    discover_targets_with_store, inspect_files_with_store_cached, inspect_stdin, is_mandatory_cop,
    offense_count, write_corrected,
};
use crate::formatter::{
    Format, FormatOptions, offenses_by_cop, render, smart_path, yaml_single_quoted,
};
use crate::rules::rule_names;
use crate::{RUBOCOP_COMPAT_VERSION, VERSION};

#[derive(Debug, Parser)]
#[command(name = "sonicop")]
#[command(about = "A fast, native RuboCop-compatible Ruby linter and formatter")]
#[command(disable_version_flag = true)]
#[command(max_term_width = 100)]
struct Cli {
    /// Files or directories to inspect
    #[arg(value_name = "FILE")]
    paths: Vec<PathBuf>,

    #[arg(short = 'l', long, help_heading = "Basic Options")]
    lint: bool,
    #[arg(short = 'x', long, help_heading = "Basic Options")]
    fix_layout: bool,
    #[arg(long, help_heading = "Basic Options")]
    safe: bool,
    #[arg(long, value_name = "COP1,COP2", help_heading = "Basic Options")]
    only: Option<String>,
    #[arg(long, value_name = "COP1,COP2", help_heading = "Basic Options")]
    except: Option<String>,
    #[arg(long, help_heading = "Basic Options")]
    only_guide_cops: bool,
    #[arg(short = 'F', long, help_heading = "Basic Options")]
    fail_fast: bool,
    #[arg(long, help_heading = "Basic Options")]
    disable_pending_cops: bool,
    #[arg(long, help_heading = "Basic Options")]
    enable_pending_cops: bool,
    #[arg(
        long,
        conflicts_with = "disable_all_cops",
        help_heading = "Basic Options"
    )]
    enable_all_cops: bool,
    #[arg(long, help_heading = "Basic Options")]
    disable_all_cops: bool,
    #[arg(long, help_heading = "Basic Options")]
    ignore_disable_comments: bool,
    #[arg(long, help_heading = "Basic Options")]
    force_exclusion: bool,
    #[arg(long, help_heading = "Basic Options")]
    only_recognized_file_types: bool,
    #[arg(long, help_heading = "Basic Options")]
    ignore_parent_exclusion: bool,
    #[arg(long, help_heading = "Basic Options")]
    ignore_unrecognized_cops: bool,
    #[arg(long, help_heading = "Basic Options")]
    force_default_config: bool,
    #[arg(short = 's', long, value_name = "FILE", help_heading = "Basic Options")]
    stdin: Option<PathBuf>,
    #[arg(long, help_heading = "Basic Options")]
    editor_mode: bool,
    #[arg(short = 'P', long, action = ArgAction::SetTrue, help_heading = "Basic Options")]
    parallel: bool,
    #[arg(long, conflicts_with = "parallel", help_heading = "Basic Options")]
    no_parallel: bool,
    #[arg(long, help_heading = "Basic Options")]
    raise_cop_error: bool,
    #[arg(
        long,
        default_value = "refactor",
        value_name = "SEVERITY",
        help_heading = "Basic Options"
    )]
    fail_level: String,

    #[arg(short = 'C', long, value_name = "FLAG", help_heading = "Caching")]
    cache: Option<String>,
    #[arg(long, value_name = "DIR", help_heading = "Caching")]
    cache_root: Option<PathBuf>,

    #[arg(short = 'f', long = "format", action = ArgAction::Append, value_name = "FORMATTER", help_heading = "Output Options")]
    formats: Vec<String>,
    #[arg(short = 'D', long, action = ArgAction::SetTrue, help_heading = "Output Options")]
    display_cop_names: bool,
    #[arg(long, help_heading = "Output Options")]
    no_display_cop_names: bool,
    #[arg(short = 'E', long, help_heading = "Output Options")]
    extra_details: bool,
    #[arg(short = 'S', long, help_heading = "Output Options")]
    display_style_guide: bool,
    #[arg(short = 'o', long, action = ArgAction::Append, value_name = "FILE", help_heading = "Output Options")]
    out: Vec<PathBuf>,
    #[arg(long, help_heading = "Output Options")]
    stderr: bool,
    #[arg(long, help_heading = "Output Options")]
    display_time: bool,
    #[arg(long, help_heading = "Output Options")]
    display_only_failed: bool,
    #[arg(long, help_heading = "Output Options")]
    display_only_fail_level_offenses: bool,
    #[arg(long, help_heading = "Output Options")]
    display_only_correctable: bool,
    #[arg(long, help_heading = "Output Options")]
    display_only_safe_correctable: bool,
    #[arg(long, help_heading = "Output Options")]
    display_suppressed: bool,

    #[arg(short = 'a', long = "autocorrect", help_heading = "Autocorrection")]
    autocorrect: bool,
    #[arg(short = 'A', long = "autocorrect-all", help_heading = "Autocorrection")]
    autocorrect_all: bool,
    #[arg(long = "auto-correct", help_heading = "Autocorrection")]
    deprecated_auto_correct: bool,
    #[arg(long = "safe-auto-correct", help_heading = "Autocorrection")]
    deprecated_safe_auto_correct: bool,
    #[arg(long = "auto-correct-all", help_heading = "Autocorrection")]
    deprecated_auto_correct_all: bool,
    #[arg(long, help_heading = "Autocorrection")]
    disable_uncorrectable: bool,

    #[arg(long, help_heading = "Config Generation")]
    auto_gen_config: bool,
    #[arg(long, help_heading = "Config Generation")]
    regenerate_todo: bool,
    #[arg(
        long,
        default_value_t = 15,
        value_name = "COUNT",
        help_heading = "Config Generation"
    )]
    exclude_limit: usize,
    #[arg(long, help_heading = "Config Generation")]
    no_exclude_limit: bool,
    #[arg(long, action = ArgAction::SetTrue, help_heading = "Config Generation")]
    offense_counts: bool,
    #[arg(long, help_heading = "Config Generation")]
    no_offense_counts: bool,
    #[arg(long, action = ArgAction::SetTrue, help_heading = "Config Generation")]
    auto_gen_only_exclude: bool,
    #[arg(long, help_heading = "Config Generation")]
    no_auto_gen_only_exclude: bool,
    #[arg(long, action = ArgAction::SetTrue, help_heading = "Config Generation")]
    auto_gen_timestamp: bool,
    #[arg(long, help_heading = "Config Generation")]
    no_auto_gen_timestamp: bool,
    #[arg(long, action = ArgAction::SetTrue, help_heading = "Config Generation")]
    auto_gen_enforced_style: bool,
    #[arg(long, help_heading = "Config Generation")]
    no_auto_gen_enforced_style: bool,

    #[arg(long, help_heading = "LSP Option")]
    lsp: bool,
    #[arg(long, help_heading = "MCP Option")]
    mcp: bool,
    #[arg(long, help_heading = "Server Options")]
    server: bool,
    #[arg(long, help_heading = "Server Options")]
    no_server: bool,
    #[arg(long, help_heading = "Server Options")]
    restart_server: bool,
    #[arg(long, help_heading = "Server Options")]
    start_server: bool,
    #[arg(long, help_heading = "Server Options")]
    stop_server: bool,
    #[arg(long, help_heading = "Server Options")]
    server_status: bool,
    #[arg(long, help_heading = "Server Options")]
    no_detach: bool,

    #[arg(short = 'L', long, help_heading = "Additional Modes")]
    list_target_files: bool,
    #[arg(long, value_name = "PATH", help_heading = "Additional Modes")]
    list_enabled_cops_for: Option<PathBuf>,
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "COP1,COP2", help_heading = "Additional Modes")]
    show_cops: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "COP1,COP2", help_heading = "Additional Modes")]
    show_docs_url: Option<String>,

    #[arg(long, help_heading = "General Options")]
    init: bool,
    #[arg(
        short = 'c',
        long,
        value_name = "FILE",
        help_heading = "General Options"
    )]
    config: Option<PathBuf>,
    #[arg(short = 'd', long, help_heading = "General Options")]
    debug: bool,
    #[arg(long, action = ArgAction::Append, value_name = "FILE", help_heading = "General Options")]
    plugin: Vec<String>,
    #[arg(short = 'r', long = "require", action = ArgAction::Append, value_name = "FILE", help_heading = "General Options")]
    requires: Vec<String>,
    #[arg(long, action = ArgAction::SetTrue, help_heading = "General Options")]
    color: bool,
    #[arg(long, conflicts_with = "color", help_heading = "General Options")]
    no_color: bool,
    #[arg(short = 'v', long = "version", help_heading = "General Options")]
    version: bool,
    #[arg(
        short = 'V',
        long = "verbose-version",
        help_heading = "General Options"
    )]
    verbose_version: bool,
    #[arg(long, help_heading = "Profiling Options")]
    profile: bool,
    #[arg(long, help_heading = "Profiling Options")]
    memory: bool,
}

impl Cli {
    fn correct_mode(&self) -> CorrectMode {
        if self.autocorrect_all || self.deprecated_auto_correct_all || self.fix_layout {
            CorrectMode::All
        } else if self.autocorrect
            || self.deprecated_auto_correct
            || self.deprecated_safe_auto_correct
        {
            CorrectMode::Safe
        } else {
            CorrectMode::None
        }
    }
}

pub fn run() -> i32 {
    // Sized from the registry: a fixed table silently drops every cop past its end.
    let names: Vec<&'static str> = crate::rules::rule_names().collect();
    crate::profile::set_enabled(std::env::var_os("SONICOP_PROFILE").is_some(), names.len());
    let code = run_inner();
    crate::profile::report(&names);
    code
}

fn run_inner() -> i32 {
    let arguments = match composed_arguments() {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("Error: {error:#}");
            return 2;
        }
    };
    let matches = match Cli::command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return 0;
        }
        Err(error) => {
            eprint!("{error}");
            return 2;
        }
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => {
            eprint!("{error}");
            return 2;
        }
    };
    let outputs = output_paths_by_format(&matches);
    match try_run(cli, &outputs) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Error: {error:#}");
            2
        }
    }
}

/// RuboCop attaches each `--out` to the `--format` that preceded it on the command line, so the
/// slots have to be filled by argument position rather than by ordinal. An `--out` given before any
/// `--format` belongs to the default formatter, and a second `--out` on the same format is dropped.
fn output_paths_by_format(matches: &ArgMatches) -> Vec<Option<PathBuf>> {
    let positions = |id| -> Vec<usize> {
        matches
            .indices_of(id)
            .map(Iterator::collect)
            .unwrap_or_default()
    };
    let format_positions = positions("formats");
    let out_positions = positions("out");
    let out_paths: Vec<&PathBuf> = matches
        .get_many::<PathBuf>("out")
        .map(Iterator::collect)
        .unwrap_or_default();

    let mut slots = vec![None; format_positions.len().max(1)];
    for (out_position, path) in out_positions.into_iter().zip(out_paths) {
        // `Options#parse`: `-o` attaches to the `-f` it follows. One written before any `-f` lands
        // in `output_path` instead, and the only thing that reads it is `apply_default_formatter`'s
        // `@options[:formatters] ||= [[format, output_path]]` -- a no-op once a `-f` was given. So
        // with a `-f` present such a path is dropped, and the formatter writes to stdout; with none,
        // it is where the default formatter writes.
        let slot = format_positions
            .iter()
            .rposition(|format_position| *format_position < out_position);
        let Some(slot) = slot.or_else(|| format_positions.is_empty().then_some(0)) else {
            continue;
        };
        slots[slot].get_or_insert_with(|| path.clone());
    }
    slots
}

fn composed_arguments() -> Result<Vec<String>> {
    let mut arguments = vec![
        std::env::args()
            .next()
            .unwrap_or_else(|| "sonicop".to_owned()),
    ];
    let dotfile = std::env::current_dir()?.join(".rubocop");
    if dotfile.is_file() {
        let contents = fs::read_to_string(&dotfile)
            .with_context(|| format!("failed to read {}", dotfile.display()))?;
        arguments.extend(
            shell_words::split(&contents)
                .with_context(|| format!("failed to parse {}", dotfile.display()))?,
        );
    }
    if let Some(options) = std::env::var_os("RUBOCOP_OPTS") {
        let options = options
            .into_string()
            .map_err(|_| anyhow::anyhow!("RUBOCOP_OPTS is not valid UTF-8"))?;
        arguments.extend(shell_words::split(&options).context("failed to parse RUBOCOP_OPTS")?);
    }
    arguments.extend(std::env::args().skip(1));
    Ok(arguments)
}

fn try_run(cli: Cli, outputs: &[Option<PathBuf>]) -> Result<i32> {
    let started = Instant::now();
    if cli.version {
        println!("{VERSION}");
        return Ok(0);
    }
    if cli.verbose_version {
        let cwd = std::env::current_dir().context("failed to determine current directory")?;
        let config =
            Config::load_with_options(cli.config.as_deref(), &cwd, cli.force_default_config)?;
        println!(
            "sonicop {VERSION} (RuboCop {RUBOCOP_COMPAT_VERSION} CLI, tree-sitter-ruby owayo@88a64c6, analyzing as Ruby {}) [{} {}]",
            config.target_ruby_version(),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        return Ok(0);
    }
    validate_compatibility(&cli)?;
    let fail_level = FailLevel::parse(&cli.fail_level)?;
    print_deprecation_warnings(&cli);
    warn_if_syntax_guard_disabled();

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    if cli.init {
        return init_config(&cwd);
    }
    let config = Config::load_with_options(cli.config.as_deref(), &cwd, cli.force_default_config)?;
    validate_config(&config, cli.ignore_unrecognized_cops)?;
    let configs = ConfigStore::new(
        config.clone(),
        cli.config.is_none() && !cli.force_default_config,
        cli.ignore_unrecognized_cops,
    );
    report_noop_modes(&cli);

    if cli.lsp || cli.mcp {
        return Ok(0);
    }
    if let Some(filter) = &cli.show_cops {
        show_cops(filter, &config);
        return Ok(0);
    }
    if let Some(filter) = &cli.show_docs_url {
        show_docs_urls(filter, &config);
        return Ok(0);
    }
    if let Some(path) = &cli.list_enabled_cops_for {
        list_enabled_cops(path, &configs)?;
        return Ok(0);
    }

    let correct_mode = cli.correct_mode();
    let selection = build_selection(&cli, &config, correct_mode)?;
    let result_cache = result_cache(&cli, &cwd, &selection, &config)?;
    if !cli.list_target_files {
        warn_unimplemented_enabled(&config, &selection);
    }

    let parallel = !cli.no_parallel && (cli.parallel || cli.stdin.is_none());
    let inspection = inspect_inputs(
        &cli,
        &cwd,
        &configs,
        &selection,
        parallel,
        result_cache.as_ref(),
    )?;
    if let Some(cache) = &result_cache {
        cache.prune();
    }
    let (mut reports, target_count) = match inspection {
        Inspection::Reports {
            reports,
            target_count,
        } => (reports, target_count),
        Inspection::ListedTargets => return Ok(0),
    };

    if cli.debug {
        debug_report(&cwd, &config, reports.len(), parallel);
    }
    if cli.auto_gen_config || cli.regenerate_todo {
        generate_todo(&reports, &cwd, &cli)?;
        return Ok(0);
    }
    if cli.disable_uncorrectable {
        eprintln!(
            "Sonicop: --disable-uncorrectable is accepted; todo insertion is not implemented yet."
        );
    }

    let CorrectionRun {
        reports: corrected_reports,
        corrected_count,
        stdin_corrected,
        had_errors,
    } = apply_corrections(reports, &cli, &configs, correct_mode, &selection)?;
    reports = corrected_reports;

    // A file RuboCop could not finish counts as a failed run even when nothing else offended.
    let failing = fail_level.failing(&reports) || had_errors;

    filter_displayed_offenses(&mut reports, &cli, fail_level);
    render_outputs(
        &RenderRequest {
            cli: &cli,
            config: &config,
            cwd: &cwd,
            fail_level,
            outputs,
            corrected_count,
            target_count,
            elapsed: started.elapsed().as_secs_f64(),
        },
        &reports,
    )?;
    if let Some(corrected) = stdin_corrected {
        print_corrected_stdin(&cli, &corrected)?;
    }

    Ok(i32::from(failing))
}

fn build_selection(cli: &Cli, config: &Config, correct_mode: CorrectMode) -> Result<Selection> {
    let mut only = csv(cli.only.as_deref());
    if cli.lint {
        only.push("Lint".to_owned());
    }
    if cli.fix_layout {
        only.push("Layout".to_owned());
    }
    validate_selection(&only, "--only", config)?;

    let except = csv(cli.except.as_deref());
    validate_selection(&except, "--except", config)?;

    Ok(Selection {
        only,
        except,
        disable_all: cli.disable_all_cops,
        enable_all: cli.enable_all_cops,
        enable_pending: cli.enable_pending_cops,
        disable_pending: cli.disable_pending_cops,
        safe_only: cli.safe,
        ignore_disable_comments: cli.ignore_disable_comments,
        display_suppressed: cli.display_suppressed,
        correcting: correct_mode != CorrectMode::None,
        // `LSP.enabled?`. `--lsp` sets it too, but that mode returns before anything is inspected,
        // so `--editor-mode` is the only way it reaches a cop from here.
        editor_mode: cli.editor_mode,
        // **No flag turns this on from the command line.** The guard is switched off for tests
        // only, and even then per case; a run reaches the environment variable or nothing.
        skip_syntax_guard: false,
    })
}

fn result_cache(
    cli: &Cli,
    cwd: &Path,
    selection: &Selection,
    config: &Config,
) -> Result<Option<ResultCache>> {
    // Profiling reports what inspecting the files costs, so it serves cached reports to nobody --
    // unless the run asks for the cache in as many words, which is how the cost of a cache *hit*
    // gets measured at all.
    let profiling = std::env::var_os("SONICOP_PROFILE").is_some();
    let enabled = cli.cache.as_deref() != Some("false")
        && !selection.correcting
        && cli.stdin.is_none()
        && !cli.profile
        && !cli.memory
        && (!profiling || cli.cache.as_deref() == Some("true"));
    if !enabled {
        return Ok(None);
    }
    let root = cli
        .cache_root
        .clone()
        .unwrap_or_else(|| default_cache_root(cwd));
    let max_files = config.all_cops_value("MaxFilesInCache").unwrap_or(20_000);
    if max_files == 0 {
        return Ok(None);
    }
    ResultCache::new(root, config.path_base(), selection, max_files).map(Some)
}

fn default_cache_root(cwd: &Path) -> PathBuf {
    if let Some(root) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(root).join("sonicop");
    }
    if cfg!(target_os = "macos")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join("Library/Caches/sonicop");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/sonicop");
    }
    cwd.join(".sonicop-cache")
}

enum Inspection {
    Reports {
        reports: Vec<FileReport>,
        /// What discovery produced, which `--fail-fast` leaves larger than the number of reports.
        /// `JSONFormatter` takes `target_file_count` from `started(target_files)` and
        /// `inspected_file_count` from `finished(inspected_files)`, so the two are separate counts
        /// and only a run that inspects everything it found makes them equal.
        target_count: usize,
    },
    ListedTargets,
}

fn inspect_inputs(
    cli: &Cli,
    cwd: &Path,
    configs: &ConfigStore,
    selection: &Selection,
    parallel: bool,
    cache: Option<&ResultCache>,
) -> Result<Inspection> {
    let Some(stdin_path) = &cli.stdin else {
        let mut targets = discover_targets_with_store(
            &cli.paths,
            cwd,
            configs,
            cli.force_exclusion,
            cli.only_recognized_file_types,
        )?;
        if cli.list_target_files {
            for path in targets {
                println!("{}", smart_path(&path, cwd));
            }
            return Ok(Inspection::ListedTargets);
        }
        if let Some(cache) = cache {
            cache.prepare(targets.len());
        }
        let reports = if cli.fail_fast {
            targets.sort_by_key(|path| {
                std::cmp::Reverse(
                    fs::metadata(path)
                        .and_then(|metadata| metadata.modified())
                        .ok(),
                )
            });
            inspect_fail_fast(&targets, configs, selection, cache)?
        } else {
            inspect_files_with_store_cached(&targets, configs, selection, parallel, cache)?
        };
        return Ok(Inspection::Reports {
            reports,
            target_count: targets.len(),
        });
    };

    if !cli.paths.is_empty() {
        bail!("--stdin requires exactly one path supplied as its argument and no file arguments");
    }
    // Read as bytes, not as text: `$stdin.binmode.read` (`options.rb:46`) is what RuboCop reads,
    // and a source that is not valid UTF-8 is an offense to report rather than a run to abort.
    let mut bytes = Vec::new();
    io::stdin()
        .read_to_end(&mut bytes)
        .context("failed to read source from stdin")?;
    let target_config = configs.for_path(stdin_path)?;
    let report = inspect_stdin(stdin_path.clone(), bytes, &target_config, selection)?;
    Ok(Inspection::Reports {
        reports: vec![report],
        target_count: 1,
    })
}

struct CorrectionRun {
    reports: Vec<FileReport>,
    corrected_count: usize,
    /// The buffer `--stdin` is left holding, which is `@options[:stdin]` after the run: the
    /// corrected source when a cop rewrote it, and the source that was read when none did.
    /// `Team#autocorrect` (`cop/team.rb:175-178`) only assigns it on a rewrite, so the unchanged
    /// case is not an absence -- it is the original buffer, and upstream prints that too.
    stdin_corrected: Option<String>,
    had_errors: bool,
}

fn apply_corrections(
    reports: Vec<FileReport>,
    cli: &Cli,
    configs: &ConfigStore,
    correct_mode: CorrectMode,
    selection: &Selection,
) -> Result<CorrectionRun> {
    let mut corrected_count = 0;
    let mut stdin_corrected = None;
    let mut had_errors = false;
    let mut corrected_reports = Vec::with_capacity(reports.len());

    for report in reports {
        let path = report.path.clone();
        let target_config = configs.for_path(&path)?;
        let outcome = correct_file(report, correct_mode, &target_config, selection)?;
        corrected_count += outcome.corrected_count;
        if let Some(message) = outcome.infinite_loop {
            // RuboCop keeps the run going and still writes what it managed to correct.
            eprintln!("{message}");
            had_errors = true;
        }
        // Not an offense: the file on disk parses, so a reader told to look for a syntax error
        // would find none. It is an autocorrect failure, and it has to reach the exit code --
        // a `-A` run that silently declined to correct must not look like a clean one to CI.
        if let Some(message) = outcome.rollback {
            eprintln!("{message}");
            had_errors = true;
        }
        if cli.stdin.is_some() {
            if correct_mode != CorrectMode::None {
                stdin_corrected = Some(outcome.text);
            }
        } else if outcome.rewritten {
            write_corrected(&path, &outcome.text)?;
        }
        corrected_reports.push(outcome.report);
    }

    Ok(CorrectionRun {
        reports: corrected_reports,
        corrected_count,
        stdin_corrected,
        had_errors,
    })
}

/// RuboCop's `--fail-level` accepts a severity plus the pseudo level `autocorrect`, which changes
/// which offenses count as failures rather than raising the severity threshold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailLevel {
    Severity(Severity),
    Autocorrect,
}

impl FailLevel {
    fn parse(value: &str) -> Result<Self> {
        if matches!(value.to_ascii_lowercase().as_str(), "a" | "autocorrect") {
            return Ok(Self::Autocorrect);
        }
        Severity::parse(value)
            .map(Self::Severity)
            .with_context(|| format!("unknown severity: {value}"))
    }

    /// `autocorrect` is not a severity, so everything that needs a threshold falls back to the
    /// default the way RuboCop's `minimum_severity_to_fail` does.
    fn severity(self) -> Severity {
        match self {
            Self::Severity(severity) => severity,
            Self::Autocorrect => Severity::Refactor,
        }
    }

    /// `Runner#considered_failure?` (`runner.rb:561-569`).
    ///
    /// `autocorrect` adds a reason to fail; it does not replace the one every run has. Upstream
    /// returns early for a correctable offense **and then falls through** to the severity
    /// comparison, so an uncorrectable offense at or above the threshold still fails the run.
    /// Reading the pseudo level as "correctable offenses only" instead lets `--fail-level
    /// autocorrect` exit 0 on a `Metrics/MethodLength` no cop can fix -- a green CI on code
    /// RuboCop fails, which is the one direction a linter must never be wrong in.
    fn failing(self, reports: &[FileReport]) -> bool {
        // `minimum_severity_to_fail` reads `autocorrect` as `:refactor`, which [`Self::severity`]
        // already does, so the threshold applies whichever level was asked for.
        if offense_count(reports, self.severity()) > 0 {
            return true;
        }
        // A correctable offense fails even once it has been corrected.
        self == Self::Autocorrect
            && reports
                .iter()
                .flat_map(|report| &report.offenses)
                .any(Offense::is_correctable)
    }
}

fn validate_compatibility(cli: &Cli) -> Result<()> {
    // `Lint/RedundantCopDisableDirective` reports directives that switched off cops with nothing
    // to say, so a run narrowed to a handful of cops could only ever report every directive in the
    // file. RuboCop refuses the combination rather than answering wrongly.
    if cli.only.as_deref().is_some_and(|value| {
        csv(Some(value)).iter().any(|name| {
            matches!(
                name.as_str(),
                "Lint/RedundantCopDisableDirective" | "RedundantCopDisableDirective"
            )
        })
    }) {
        bail!("Lint/RedundantCopDisableDirective cannot be used with --only.");
    }
    if cli.except.as_deref().is_some_and(|value| {
        csv(Some(value))
            .iter()
            .any(|name| is_mandatory_cop(name.as_str()))
    }) {
        bail!("Syntax checking cannot be turned off.");
    }
    if let Some(cache) = &cli.cache
        && !matches!(cache.as_str(), "true" | "false")
    {
        bail!("-C/--cache argument must be true or false");
    }
    if cli.cache.as_deref() == Some("false") && cli.cache_root.is_some() {
        bail!("--cache-root cannot be used with --cache false");
    }
    if cli.display_only_failed
        && !cli
            .formats
            .iter()
            .any(|format| matches!(format.as_str(), "junit" | "ju"))
    {
        bail!("--display-only-failed can only be used with --format junit");
    }
    if cli.display_only_correctable && (cli.autocorrect || cli.autocorrect_all || cli.fix_layout) {
        bail!("--display-only-correctable cannot be combined with autocorrection");
    }
    if cli.lsp && cli.editor_mode {
        bail!("--lsp cannot be combined with --editor-mode");
    }
    Ok(())
}

/// Says out loud that the guard against writing unparsable corrections has been switched off.
///
/// The switch exists so the guard's error rate can be measured -- with it on, the text needed to
/// measure it never reaches disk. **A measurement hatch must not be a silent one:** left set in a
/// shell profile or a CI job it turns a refusal to write into a destructive write, and nothing
/// else in the output would say so.
fn warn_if_syntax_guard_disabled() {
    if std::env::var_os(NO_SYNTAX_GUARD).is_some() {
        eprintln!(
            "Sonicop: {} is set. The guard against writing corrections that do not parse is off, \
             so autocorrection will overwrite files even when the result is a syntax error.",
            NO_SYNTAX_GUARD
        );
    }
}

fn print_deprecation_warnings(cli: &Cli) {
    if cli.deprecated_auto_correct {
        eprintln!("--auto-correct is deprecated; use --autocorrect instead.");
    }
    if cli.deprecated_safe_auto_correct {
        eprintln!("--safe-auto-correct is deprecated; use --autocorrect instead.");
    }
    if cli.deprecated_auto_correct_all {
        eprintln!("--auto-correct-all is deprecated; use --autocorrect-all instead.");
    }
}

fn report_noop_modes(cli: &Cli) {
    if cli.server
        || cli.no_server
        || cli.restart_server
        || cli.start_server
        || cli.stop_server
        || cli.server_status
        || cli.no_detach
    {
        eprintln!(
            "Sonicop: server flags are accepted as no-ops because native startup is immediate."
        );
    }
    if cli.lsp {
        eprintln!("Sonicop: --lsp is accepted; the LSP transport is not implemented yet.");
    }
    if cli.mcp {
        eprintln!("Sonicop: --mcp is accepted; the MCP transport is not implemented yet.");
    }
    if !cli.plugin.is_empty() || !cli.requires.is_empty() {
        eprintln!("Sonicop: Ruby plugins and --require entries are accepted but not executed.");
    }
    if cli.profile || cli.memory {
        eprintln!(
            "Sonicop: profiling flags are accepted; use an external Rust profiler for native traces."
        );
    }
    let _ = (
        cli.only_guide_cops,
        cli.ignore_parent_exclusion,
        cli.raise_cop_error,
        cli.offense_counts,
        cli.no_offense_counts,
        cli.auto_gen_only_exclude,
        cli.no_auto_gen_only_exclude,
        cli.auto_gen_timestamp,
        cli.no_auto_gen_timestamp,
        cli.auto_gen_enforced_style,
        cli.no_auto_gen_enforced_style,
    );
}

fn inspect_fail_fast(
    paths: &[PathBuf],
    configs: &ConfigStore,
    selection: &Selection,
    cache: Option<&ResultCache>,
) -> Result<Vec<FileReport>> {
    let mut reports = Vec::new();
    for path in paths {
        let mut inspected = inspect_files_with_store_cached(
            std::slice::from_ref(path),
            configs,
            selection,
            false,
            cache,
        )?;
        let has_offense = inspected
            .first()
            .is_some_and(|report| !report.offenses.is_empty());
        reports.append(&mut inspected);
        if has_offense {
            break;
        }
    }
    Ok(reports)
}

fn validate_config(config: &Config, ignore_unrecognized: bool) -> Result<()> {
    if !ignore_unrecognized && !config.unrecognized_cop_names().is_empty() {
        bail!(
            "unrecognized cop(s): {}",
            config.unrecognized_cop_names().join(", ")
        );
    }
    Ok(())
}

fn validate_selection(values: &[String], flag: &str, config: &Config) -> Result<()> {
    let known: HashSet<&str> = config.known_cop_names().collect();
    let departments: HashSet<&str> = known
        .iter()
        .map(|name| cop_name::department(name))
        .collect();
    for value in values {
        if !known.contains(value.as_str()) && !departments.contains(value.as_str()) {
            bail!("Unrecognized cop or department for {flag}: {value}.");
        }
    }
    Ok(())
}

/// Tell the user about cops that resolved to enabled but have no implementation here.
///
/// An unimplemented cop is never planned, so a configuration that switches one on — or an explicit
/// `--only` naming it — yields no offenses at all where RuboCop would report some. Reporting 394 of
/// RuboCop's 609 cops is fine as long as nobody mistakes silence for a clean file, so say which
/// checks did not happen.
///
/// The note goes to stderr, never stdout: RuboCop's own `-P/--parallel is being ignored` notice
/// goes to stdout and corrupts its JSON output, which is a mistake worth not repeating.
fn warn_unimplemented_enabled(config: &Config, selection: &Selection) {
    let implemented: HashSet<&str> = rule_names().collect();
    let mut skipped = config
        .known_cop_names()
        .filter(|name| !implemented.contains(name))
        .filter(|&name| {
            let enabled = config.rule_enabled_with_pending(
                name,
                selection.enable_pending,
                selection.disable_pending,
            );
            selection.includes(name, enabled, config.rule_safe(name))
        })
        .collect::<Vec<_>>();
    if skipped.is_empty() {
        return;
    }
    skipped.sort_unstable();
    const SHOWN: usize = 5;
    let listed = skipped[..skipped.len().min(SHOWN)].join(", ");
    let more = skipped.len().saturating_sub(SHOWN);
    let rest = if more > 0 {
        format!(" and {more} more (--debug lists every unimplemented cop)")
    } else {
        String::new()
    };
    let plural = if skipped.len() == 1 { "cop" } else { "cops" };
    eprintln!(
        "warning: {} enabled {plural} not implemented by Sonicop; nothing was checked for {}: {listed}{rest}",
        skipped.len(),
        if skipped.len() == 1 { "it" } else { "them" }
    );
}

fn debug_report(cwd: &Path, config: &Config, targets: usize, parallel: bool) {
    let implemented: HashSet<&str> = rule_names().collect();
    let mut unimplemented = config
        .known_cop_names()
        .filter(|name| !implemented.contains(name))
        .collect::<Vec<_>>();
    unimplemented.sort();
    eprintln!(
        "For {}: configuration from {}, {targets} target(s), parallel={parallel}",
        cwd.display(),
        config.config_path().map_or_else(
            || "built-in defaults".to_owned(),
            |path| path.display().to_string()
        )
    );
    eprintln!(
        "Implemented cops: {}; recognized but not implemented: {}",
        implemented.len(),
        unimplemented.len()
    );
    eprintln!("Unimplemented cops: {}", unimplemented.join(", "));
}

/// The cop names a `--show-cops`/`--show-docs-url` filter selects, sorted. An empty filter names
/// every known cop, which is how RuboCop prints the whole list.
fn selected_cop_names<'a>(filter: &str, config: &'a Config) -> Vec<&'a str> {
    let filters = csv((!filter.is_empty()).then_some(filter));
    let mut names = config
        .known_cop_names()
        .filter(|name| {
            filters.is_empty()
                || filters
                    .iter()
                    .any(|selection| selector_matches(selection, name))
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn show_cops(filter: &str, config: &Config) {
    let implemented: HashSet<&str> = rule_names().collect();
    for name in selected_cop_names(filter, config) {
        println!("{name}:");
        if let Some(description) = config.description(name) {
            println!(
                "  Description: {}",
                description.lines().next().unwrap_or("")
            );
        }
        println!("  Enabled: {}", config.rule_enabled(name));
        println!("  Implemented: {}", implemented.contains(name));
    }
}

fn show_docs_urls(filter: &str, config: &Config) {
    for name in selected_cop_names(filter, config) {
        println!("{name}: {}", docs_url(name));
    }
}

/// RuboCop's `Documentation.url_for`: the page covers the whole department with `/` flattened to
/// `_`, and the fragment is the qualified name reduced to its lowercased letters, so
/// `Layout/LineLength` documents at `cops_layout.html#layoutlinelength`.
fn docs_url(name: &str) -> String {
    let page = cop_name::department(name)
        .replace('/', "_")
        .to_ascii_lowercase();
    let fragment: String = name
        .chars()
        .filter(char::is_ascii_alphabetic)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    format!("https://docs.rubocop.org/rubocop/{RUBOCOP_COMPAT_VERSION}/cops_{page}.html#{fragment}")
}

fn list_enabled_cops(path: &Path, configs: &ConfigStore) -> Result<()> {
    let config = configs.for_path(path)?;
    let mut names = config
        .known_cop_names()
        .filter(|name| config.rule_enabled(name))
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        println!("{name}");
    }
    Ok(())
}

fn generate_todo(reports: &[FileReport], cwd: &Path, cli: &Cli) -> Result<()> {
    let mut output = format!(
        "# This configuration was generated by\n# `sonicop --auto-gen-config` using Sonicop version {VERSION}.\n\n"
    );
    for (cop, offenses) in offenses_by_cop(reports, cwd) {
        // RuboCop drops Lint/Syntax before writing the records (`disabled_config_formatter.rb:69`)
        // because it is not a real cop and cannot be disabled, so a record for it would only be
        // noise the user can never resolve.
        if is_mandatory_cop(cop) {
            continue;
        }
        if !cli.no_offense_counts {
            output.push_str(&format!("# Offense count: {}\n", offenses.offense_count));
        }
        output.push_str(&format!("{cop}:\n"));
        // The limit counts offending files, not offenses, so a single file cannot exclude itself
        // into `Enabled: false`.
        if !cli.no_exclude_limit && offenses.paths.len() > cli.exclude_limit {
            output.push_str("  Enabled: false\n\n");
        } else {
            output.push_str("  Exclude:\n");
            for path in &offenses.paths {
                output.push_str(&format!("    - {}\n", yaml_single_quoted(path)));
            }
            output.push('\n');
        }
    }
    fs::write(cwd.join(".rubocop_todo.yml"), output)?;
    let root_config = cwd.join(".rubocop.yml");
    // `add_inheritance_from_auto_generated_file` reads the file only after `File.exist?` says
    // there is one, so "absent" and "unreadable" are separate answers and only the first means
    // "write a new one". Collapsing them -- `unwrap_or_default()` -- turns every read failure into
    // an empty string, and the prepend below silently becomes a replace: a `.rubocop.yml` that is
    // not valid UTF-8, or that the process cannot open, is overwritten by the one line this would
    // have added to it, with a zero exit code and nothing on stderr. Fail instead.
    let existing = match fs::read_to_string(&root_config) {
        Ok(existing) => existing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to read {}; refusing to replace it with the generated inheritance",
                    root_config.display()
                )
            });
        }
    };
    if !existing.contains(".rubocop_todo.yml") {
        let addition = if existing.is_empty() {
            "inherit_from: .rubocop_todo.yml\n".to_owned()
        } else {
            format!("inherit_from: .rubocop_todo.yml\n{existing}")
        };
        fs::write(root_config, addition)?;
    }
    println!("Generated .rubocop_todo.yml");
    Ok(())
}

fn filter_displayed_offenses(reports: &mut [FileReport], cli: &Cli, fail_level: FailLevel) {
    for report in reports {
        report.offenses.retain(|offense| {
            (!cli.display_only_fail_level_offenses || offense.severity >= fail_level.severity())
                && (!cli.display_only_correctable || offense.is_correctable())
                && (!cli.display_only_safe_correctable
                    || (offense.is_correctable()
                        && offense.corrections.iter().all(|edit| edit.safe)))
        });
    }
}

/// The run-wide state every formatter needs, gathered once so each format only has to supply the
/// reports.
struct RenderRequest<'a> {
    cli: &'a Cli,
    config: &'a Config,
    cwd: &'a Path,
    fail_level: FailLevel,
    /// Destination per format, already paired the way RuboCop pairs `--out` with `--format`.
    outputs: &'a [Option<PathBuf>],
    corrected_count: usize,
    /// How many files discovery produced, which is not how many were inspected once `--fail-fast`
    /// stops the run early. See [`Inspection::Reports`].
    target_count: usize,
    elapsed: f64,
}

fn render_outputs(request: &RenderRequest<'_>, reports: &[FileReport]) -> Result<()> {
    let RenderRequest {
        cli,
        config,
        cwd,
        fail_level,
        outputs,
        corrected_count,
        target_count,
        elapsed,
    } = *request;
    let formats = if cli.formats.is_empty() {
        vec![
            config
                .all_cops_value::<String>("DefaultFormatter")
                .unwrap_or_else(|| "progress".to_owned()),
        ]
    } else {
        cli.formats.clone()
    };
    let display_cop_names = if cli.no_display_cop_names {
        false
    } else {
        cli.display_cop_names || config.display_cop_names()
    };
    let color = !cli.no_color
        && (cli.color || (cli.out.is_empty() && !cli.stderr && io::stdout().is_terminal()));
    for (index, name) in formats.iter().enumerate() {
        let format = Format::parse(name)?;
        let mut rendered = render(
            format,
            reports,
            &FormatOptions {
                cwd,
                config,
                display_cop_names,
                display_style_guide: cli.display_style_guide,
                extra_details: cli.extra_details,
                color,
                corrected_count,
                target_file_count: target_count,
                fail_level: fail_level.severity(),
                safe_autocorrect: cli.correct_mode() == CorrectMode::Safe,
                display_only_failed: cli.display_only_failed,
            },
        )?;
        if cli.display_time {
            rendered.push_str(&format!("Finished in {elapsed:.3} seconds\n"));
        }
        write_output(
            &rendered,
            outputs.get(index).and_then(Option::as_deref),
            cli.stderr,
        )?;
    }
    Ok(())
}

/// `ExecuteRunner::INTEGRATION_FORMATTERS`: the formats whose consumers parse the whole stream, so
/// nothing may be appended to them. Both the short and the long spelling of each.
const INTEGRATION_FORMATTERS: [&str; 6] = ["h", "html", "j", "json", "ju", "junit"];

/// Appends the source `--stdin` was left holding to a formatter's report, the way
/// `ExecuteRunner#maybe_print_corrected_source` (`cli/command/execute_runner.rb:92-102`) does.
///
/// An editor driving `--stdin -a` reads the corrected buffer off the end of the output, which is
/// what the twenty `=` mark the start of. The report still comes first: printing only the buffer
/// leaves nothing to say what was corrected, and with `--format json` it puts raw Ruby where a
/// caller asked for JSON and will try to parse it.
///
/// Two details are upstream's rather than chosen here. The check is against `@options[:format]`,
/// which the option parser assigns per `-f`, so it is the **last** format on the command line that
/// decides -- `-f json -f simple` appends. And `--stderr` moves the separator but not the source:
/// upstream picks the stream for the `puts` and then `print`s to `$stdout` unconditionally, by
/// which point the redirect around the runner has already been undone.
fn print_corrected_stdin(cli: &Cli, corrected: &str) -> Result<()> {
    if cli
        .formats
        .last()
        .is_some_and(|format| INTEGRATION_FORMATTERS.contains(&format.as_str()))
    {
        return Ok(());
    }
    let separator = format!("{}\n", "=".repeat(20));
    if cli.stderr {
        io::stderr().write_all(separator.as_bytes())?;
    } else {
        io::stdout().write_all(separator.as_bytes())?;
    }
    io::stdout().write_all(corrected.as_bytes())?;
    io::stdout().flush()?;
    Ok(())
}

fn init_config(cwd: &Path) -> Result<i32> {
    let path = cwd.join(".rubocop.yml");
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    fs::write(
        &path,
        "# Sonicop / RuboCop configuration\nAllCops:\n  NewCops: enable\n",
    )?;
    println!("Created {}", path.display());
    Ok(0)
}

fn csv(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn write_output(output: &str, path: Option<&Path>, stderr: bool) -> Result<()> {
    if let Some(path) = path {
        fs::write(path, output).with_context(|| format!("failed to write {}", path.display()))?;
    } else if stderr {
        io::stderr().write_all(output.as_bytes())?;
    } else {
        io::stdout().write_all(output.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
