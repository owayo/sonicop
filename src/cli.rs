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
    CorrectMode, CorrectionEncodingError, NO_SYNTAX_GUARD, ResultCache, Selection,
    SharedStdinState, correct_file_with_original, correct_shared_stdin, correction_bytes,
    discover_stdin_targets_with_store, discover_targets_with_store, inspect_files,
    inspect_files_with_store_cached, inspect_shared_stdin, inspect_stdin, is_mandatory_cop,
    native_textmode_bytes, offense_count, stdin_output_bytes, write_correction,
};
use crate::formatter::{
    Format, FormatOptions, offenses_by_cop, render, smart_path, yaml_single_quoted,
};
use crate::rules::rule_names;
use crate::{RUBOCOP_COMPAT_VERSION, VERSION};

#[path = "cache_string_printable_table.rs"]
mod cache_string_printable_table;

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
    // 本家の add_cop_selection_csv_option は、後の指定で集合全体を置き換える。
    #[arg(
        long,
        overrides_with = "only",
        value_name = "COP1,COP2",
        help_heading = "Basic Options"
    )]
    only: Option<String>,
    #[arg(
        long,
        overrides_with = "except",
        value_name = "COP1,COP2",
        help_heading = "Basic Options"
    )]
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
    // 本家は明示した空文字も stdin の対象名として受け取り、値の省略とは区別する。
    #[arg(
        short = 's',
        long,
        value_name = "FILE",
        help_heading = "Basic Options",
        value_parser = clap::builder::TypedValueParser::map(
            clap::builder::OsStringValueParser::new(),
            PathBuf::from,
        ),
    )]
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
    #[arg(short = 'D', long, action = ArgAction::SetTrue, overrides_with = "no_display_cop_names", help_heading = "Output Options")]
    display_cop_names: bool,
    #[arg(
        long,
        overrides_with = "display_cop_names",
        help_heading = "Output Options"
    )]
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
    #[arg(skip)]
    cache_message_options: String,
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
    let matches = match Cli::command().try_get_matches_from(&arguments) {
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
    let mut cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => {
            eprint!("{error}");
            return 2;
        }
    };
    cli.cache_message_options = cache_message_options(&arguments);
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

// Options#option の Hash は初回の挿入位置を保ち、同じキーの値だけを更新する。
fn set_cache_option(options: &mut Vec<(String, String)>, key: &str, value: String) {
    if let Some((_, previous)) = options.iter_mut().find(|(name, _)| name == key) {
        *previous = value;
    } else {
        options.push((key.to_owned(), value));
    }
}

fn cache_nonprintable(character: char) -> bool {
    // 依存 regex の Unicode 分類は U+088F で本家と違うため、実 String#inspect から生成する。
    let codepoint = u32::from(character);
    if codepoint < 128 {
        return codepoint < 32 || codepoint == 127;
    }
    let ranges = cache_string_printable_table::NONPRINTABLE_RANGES;
    let index = ranges.partition_point(|&(start, _)| start <= codepoint);
    index > 0 && codepoint <= ranges[index - 1].1
}

fn cache_inspect_string(value: &str) -> String {
    // cop が使う literal helper の語義を変えず、UTF-8 CLI の String#inspect だけを再現する。
    let mut inspected = String::from("\"");
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => inspected.push_str("\\\""),
            '\\' => inspected.push_str("\\\\"),
            '\n' => inspected.push_str("\\n"),
            '\r' => inspected.push_str("\\r"),
            '\t' => inspected.push_str("\\t"),
            '\u{c}' => inspected.push_str("\\f"),
            '\u{b}' => inspected.push_str("\\v"),
            '\u{8}' => inspected.push_str("\\b"),
            '\u{7}' => inspected.push_str("\\a"),
            '\u{1b}' => inspected.push_str("\\e"),
            '#' if matches!(characters.peek(), Some('{' | '@' | '$')) => inspected.push_str("\\#"),
            other if cache_nonprintable(other) => {
                if u32::from(other) <= 0xFFFF {
                    inspected.push_str(&format!("\\u{:04X}", u32::from(other)));
                } else {
                    inspected.push_str(&format!("\\u{{{:X}}}", u32::from(other)));
                }
            }
            other => inspected.push(other),
        }
    }
    inspected.push('"');
    inspected
}

fn cache_message_options(arguments: &[String]) -> String {
    // Clap が検証した argv の投影だけを作る。引数の解釈・出力先・raw path は変更しない。
    let command = Cli::command();
    let mut options = Vec::new();
    let mut only = Vec::new();
    let mut formatted = false;
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            break;
        }
        let mut events = Vec::new();
        if let Some(long) = argument.strip_prefix("--") {
            let (name, attached) = long
                .split_once('=')
                .map_or((long, None), |(key, value)| (key, Some(value.to_owned())));
            if let Some(definition) = command
                .get_arguments()
                .find(|arg| arg.get_long() == Some(name))
            {
                let value = if definition.get_action().takes_values() {
                    attached.or_else(|| arguments.next().cloned())
                } else {
                    None
                };
                events.push((definition, value));
            }
        } else if let Some(short) = argument.strip_prefix('-') {
            for (index, character) in short.char_indices() {
                let Some(definition) = command
                    .get_arguments()
                    .find(|arg| arg.get_short() == Some(character))
                else {
                    continue;
                };
                if definition.get_action().takes_values() {
                    let rest = &short[index + character.len_utf8()..];
                    let value = if rest.is_empty() {
                        arguments.next().cloned()
                    } else {
                        Some(rest.strip_prefix('=').unwrap_or(rest).to_owned())
                    };
                    events.push((definition, value));
                    break;
                }
                events.push((definition, None));
            }
        }
        for (definition, value) in events {
            let id = definition.get_id().as_str();
            // Ruby に無い display_suppressed は既存 Selection の fingerprint が扱う。
            if id == "display_suppressed" {
                continue;
            }
            if id == "formats" {
                formatted = true;
                continue;
            }
            if id == "out" {
                // 最初の -f 前の output_path は、後の -f でも消さず最終代入だけ保つ。
                if !formatted && let Some(value) = value {
                    set_cache_option(&mut options, "output_path", cache_inspect_string(&value));
                }
                continue;
            }
            let key = match id {
                "no_display_cop_names" => "display_cop_names",
                "no_parallel" => "parallel",
                "no_color" => "color",
                "no_server" => "server",
                "no_offense_counts" => "offense_counts",
                "no_auto_gen_only_exclude" => "auto_gen_only_exclude",
                "no_auto_gen_timestamp" => "auto_gen_timestamp",
                "no_auto_gen_enforced_style" => "auto_gen_enforced_style",
                "requires" => "require",
                other => other,
            };
            // ResultCache::NON_CHANGING の全項目。paths は Option Hash に入らない。
            let excluded = matches!(
                key,
                "color"
                    | "format"
                    | "formatters"
                    | "out"
                    | "debug"
                    | "display_time"
                    | "fail_level"
                    | "fix_layout"
                    | "autocorrect"
                    | "safe_autocorrect"
                    | "autocorrect_all"
                    | "deprecated_auto_correct"
                    | "deprecated_safe_auto_correct"
                    | "deprecated_auto_correct_all"
                    | "cache"
                    | "fail_fast"
                    | "stdin"
                    | "parallel"
            );
            if !excluded {
                let inspected = match key {
                    "only" | "except" => {
                        let value = value.as_deref().unwrap_or_default();
                        let mut names: Vec<String> = if value.is_empty() {
                            vec![String::new()]
                        } else {
                            value.split(',').map(str::to_owned).collect()
                        };
                        while names.last().is_some_and(String::is_empty) && names.len() > 1 {
                            names.pop();
                        }
                        if key == "only" {
                            if !names.iter().any(|name| name == "Lint/Syntax") {
                                names.insert(0, "Lint/Syntax".to_owned());
                            }
                            only = names.clone();
                        }
                        format!(
                            "[{}]",
                            names
                                .iter()
                                .map(|name| cache_inspect_string(name))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                    _ => value.map_or_else(
                        || {
                            (!definition
                                .get_long()
                                .is_some_and(|name| name.starts_with("no-")))
                            .to_string()
                        },
                        |value| cache_inspect_string(&value),
                    ),
                };
                set_cache_option(&mut options, key, inspected);
            }
            if matches!(id, "lint" | "fix_layout") {
                only.push(if id == "lint" { "Lint" } else { "Layout" }.to_owned());
                set_cache_option(
                    &mut options,
                    "only",
                    format!(
                        "[{}]",
                        only.iter()
                            .map(|name| cache_inspect_string(name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
    }
    // Hash#inspect の句読点はすべて同じ連続へ縮む。Unicode /i は手書き分類にしない。
    let inspected = format!(
        "{{{}}}",
        options
            .iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    static NON_LETTERS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new("(?i)[^a-z]+").expect("the literal cache-key pattern is valid")
    });
    NON_LETTERS.replace_all(&inspected, "_").into_owned()
}

fn composed_arguments() -> Result<Vec<String>> {
    // String の CLI へ渡せない名前は拒否する。置換文字へ変えると別ファイルを指してしまう。
    let mut command_line = std::env::args_os();
    let mut arguments = vec![
        command_line
            .next()
            .unwrap_or_else(|| "sonicop".into())
            .into_string()
            .map_err(|_| anyhow::anyhow!("command-line argument 0 is not valid Unicode"))?,
    ];
    let dotfile = std::env::current_dir()?.join(".rubocop");
    if dotfile.is_file() {
        let contents = fs::read_to_string(&dotfile)
            .with_context(|| format!("failed to read {}", dotfile.display()))?;
        // ArgumentsFile の File.read は Windows の text mode で、引用内の CRLF も LF にする。
        #[cfg(windows)]
        let contents = contents.replace("\r\n", "\n");
        arguments.extend(
            split_external_arguments(&contents)
                .with_context(|| format!("failed to parse {}", dotfile.display()))?,
        );
    }
    if let Some(options) = std::env::var_os("RUBOCOP_OPTS") {
        let options = options
            .into_string()
            .map_err(|_| anyhow::anyhow!("RUBOCOP_OPTS is not valid UTF-8"))?;
        arguments
            .extend(split_external_arguments(&options).context("failed to parse RUBOCOP_OPTS")?);
    }
    for (index, argument) in command_line.enumerate() {
        arguments.push(argument.into_string().map_err(|_| {
            anyhow::anyhow!("command-line argument {} is not valid Unicode", index + 1)
        })?);
    }
    Ok(arguments)
}

/// Ruby Shellwords 0.2.2 の実出力に合わせ、POSIX crate の空白・コメント・継続行を補う。
fn split_external_arguments(source: &str) -> Result<Vec<String>> {
    if source.contains('\0') {
        bail!("NUL character");
    }
    let mut input = String::with_capacity(source.len());
    let mut quote = None;
    let mut characters = source.chars();
    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => {
                quote = None;
                input.push(character);
            }
            (None, '\'' | '"') => {
                quote = Some(character);
                input.push(character);
            }
            (Some('\''), _) => input.push(character),
            (_, '\\') => match characters.next() {
                // Ruby の esc.gsub は LF を消さず、DQ の gsub は backslash だけを消す。
                Some('\n') if quote.is_none() => input.push_str("'\\\n'"),
                Some('\n') => input.push('\n'),
                next => {
                    input.push('\\');
                    if let Some(next) = next {
                        input.push(next);
                    }
                }
            },
            (None, '\r' | '\u{b}' | '\u{c}') => input.push(' '),
            // Ruby は # をコメントとして扱わないが、crate は単語の先頭で捨てる。
            (None, '#') => input.push_str("\\#"),
            _ => input.push(character),
        }
    }
    Ok(shell_words::split(&input)?)
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
            "sonicop {VERSION} (RuboCop {RUBOCOP_COMPAT_VERSION} CLI, tree-sitter-ruby v26.10.104, analyzing as Ruby {}) [{} {}]",
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
    let mut selection = build_selection(&cli, &config, correct_mode)?;
    let result_cache = result_cache(&cli, &cwd, &selection, &config)?;
    if !cli.list_target_files {
        warn_unimplemented_enabled(&config, &selection);
    }

    let parallel = !cli.no_parallel && (cli.parallel || cli.stdin.is_none());
    let inspection = inspect_inputs(
        &cli,
        &cwd,
        &configs,
        &mut selection,
        parallel,
        result_cache.as_ref(),
    )?;
    if let Some(cache) = &result_cache {
        cache.prune();
    }
    let (mut reports, target_count, stdin_original, stdin_directory) = match inspection {
        Inspection::Reports {
            reports,
            target_count,
            stdin_original,
        } => (reports, target_count, stdin_original, None),
        Inspection::StdinDirectory { targets, bytes } => {
            (Vec::new(), targets.len(), Some(bytes), Some(targets))
        }
        Inspection::ListedTargets => return Ok(0),
        Inspection::ListedStdin { bytes } => {
            let run = inspect_stdin_directory(
                &[],
                &bytes,
                &configs,
                correct_mode,
                &selection,
                &cli,
                fail_level,
            )?;
            if let Some(corrected) = run.stdin_corrected {
                print_corrected_stdin(&cli, &corrected)?;
            }
            return Ok(if run.had_write_failures { 2 } else { 0 });
        }
    };

    if cli.debug {
        let inspected = stdin_directory.as_ref().map_or(reports.len(), Vec::len);
        debug_report(&cwd, &config, inspected, parallel);
    }
    if cli.auto_gen_config || cli.regenerate_todo {
        if let Some(targets) = &stdin_directory {
            reports = inspect_stdin_directory(
                targets,
                stdin_original.as_deref().unwrap(),
                &configs,
                CorrectMode::None,
                &selection,
                &cli,
                fail_level,
            )?
            .reports;
        }
        generate_todo(&reports, &cwd, &cli)?;
        return Ok(0);
    }
    if cli.disable_uncorrectable {
        eprintln!(
            "Sonicop: --disable-uncorrectable is accepted; todo insertion is not implemented yet."
        );
    }

    let corrections = if let Some(targets) = &stdin_directory {
        inspect_stdin_directory(
            targets,
            stdin_original.as_deref().unwrap(),
            &configs,
            correct_mode,
            &selection,
            &cli,
            fail_level,
        )?
    } else {
        apply_corrections(
            reports,
            stdin_original.as_deref().map(|original| StdinCorrection {
                original,
                anonymous: cli
                    .stdin
                    .as_ref()
                    .is_some_and(|path| path.as_os_str().is_empty()),
                echo: prints_corrected_stdin(&cli),
                shared: None,
            }),
            &configs,
            correct_mode,
            &selection,
        )?
    };
    let CorrectionRun {
        reports: corrected_reports,
        corrected_count,
        stdin_corrected,
        had_errors,
        had_write_failures,
        ..
    } = corrections;
    reports = corrected_reports;

    // 検査や補正を完了できないファイルがあれば、通常の違反がなくても失敗になる。
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

    // 構文や符号化を壊すため書き戻せなかった失敗は、残った通常の違反と区別する。
    Ok(if had_write_failures {
        2
    } else {
        i32::from(failing)
    })
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
        project_index: None,
    })
}

fn message_display_cop_names(cli: &Cli, config: &Config) -> bool {
    // 最後の生 --format を使い、alias j と設定の DefaultFormatter は json 抑止へ含めない。
    if cli.debug {
        true
    } else if cli.no_display_cop_names {
        false
    } else if cli.display_cop_names {
        true
    } else if cli.formats.last().is_some_and(|format| format == "json") {
        false
    } else {
        config.display_cop_names()
    }
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
        && !crate::ProjectIndex::enabled(config)
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
    ResultCache::new(root, config.path_base(), selection, max_files).map(|cache| {
        Some(cache.with_message_options(
            &cli.cache_message_options,
            message_display_cop_names(cli, config),
        ))
    })
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
        stdin_original: Option<Vec<u8>>,
    },
    StdinDirectory {
        targets: Vec<PathBuf>,
        bytes: Vec<u8>,
    },
    ListedStdin {
        bytes: Vec<u8>,
    },
    ListedTargets,
}

fn inspect_inputs(
    cli: &Cli,
    cwd: &Path,
    configs: &ConfigStore,
    selection: &mut Selection,
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
        *selection = selection.with_project_index(&targets, configs)?;
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
            stdin_original: None,
        });
    };

    if !cli.paths.is_empty() {
        bail!("--stdin requires exactly one path supplied as its argument and no file arguments");
    }
    // TargetFinder の File.expand_path('') と同じ名前で設定と path 依存の cop を評価する。
    let anonymous_stdin = stdin_path.as_os_str().is_empty();
    let stdin_path = if anonymous_stdin {
        cwd.to_path_buf()
    } else {
        stdin_path.clone()
    };
    // 本家の stdin.binmode.read と同じくバイトで読み、拒否や未補正時の原本も保持する。
    let mut bytes = Vec::new();
    io::stdin()
        .read_to_end(&mut bytes)
        .context("failed to read source from stdin")?;
    // 空名は #107 の仮想ファイル。cwd に展開してもディレクトリ探索へ変えない。
    if !anonymous_stdin && stdin_path.is_dir() {
        let mut targets = discover_stdin_targets_with_store(&stdin_path, cwd, configs)?;
        *selection = selection.with_project_index(&targets, configs)?;
        if cli.fail_fast {
            targets.sort_by_key(|path| {
                std::cmp::Reverse(
                    fs::metadata(path)
                        .and_then(|metadata| metadata.modified())
                        .ok(),
                )
            });
        }
        if cli.list_target_files {
            for path in targets {
                let line = format!("{}\n", smart_path(&path, cwd));
                io::stdout().write_all(&native_textmode_bytes(line.as_bytes()))?;
            }
            return Ok(Inspection::ListedStdin { bytes });
        }
        return Ok(Inspection::StdinDirectory { targets, bytes });
    }
    *selection = selection.with_project_index(std::slice::from_ref(&stdin_path), configs)?;
    let target_config = if anonymous_stdin {
        anonymous_stdin_config(configs, &stdin_path)?
    } else {
        configs.for_path(&stdin_path)?
    };
    let report = inspect_stdin(stdin_path, bytes.clone(), &target_config, selection)?;
    Ok(Inspection::Reports {
        reports: vec![report],
        target_count: 1,
        stdin_original: Some(bytes),
    })
}

fn anonymous_stdin_config(configs: &ConfigStore, path: &Path) -> Result<std::sync::Arc<Config>> {
    // 空名は cwd という仮想ファイル。ConfigStore#for_file は、その親から設定を探す。
    configs.for_path(path.parent().unwrap_or(path))
}

struct CorrectionRun {
    reports: Vec<FileReport>,
    corrected_count: usize,
    /// Team#autocorrect は書き換え時だけ stdin バッファを更新するため、
    /// 補正がなければ本家と同じく読み込んだ元のバッファを出力する。
    stdin_corrected: Option<Vec<u8>>,
    /// 最後の対象が無変更でも、途中の補正による物理 CRLF 復元を二重に変換しない。
    stdin_rewritten: bool,
    stdin_rollback: bool,
    had_errors: bool,
    had_write_failures: bool,
}

struct StdinCorrection<'a> {
    original: &'a [u8],
    anonymous: bool,
    echo: bool,
    shared: Option<&'a mut SharedStdinState>,
}

/// Runner と Team が共有する本文は logical のまま次へ渡し、echo は最後に一度だけ作る。
fn inspect_stdin_directory(
    targets: &[PathBuf],
    original: &[u8],
    configs: &ConfigStore,
    correct_mode: CorrectMode,
    selection: &Selection,
    cli: &Cli,
    fail_level: FailLevel,
) -> Result<CorrectionRun> {
    let mut current = original.to_vec();
    let mut shared = SharedStdinState::default();
    let mut result = CorrectionRun {
        reports: Vec::with_capacity(targets.len()),
        corrected_count: 0,
        stdin_corrected: None,
        stdin_rewritten: false,
        stdin_rollback: false,
        had_errors: false,
        had_write_failures: false,
    };
    for path in targets {
        let config = configs.for_path(path)?;
        let report = inspect_shared_stdin(path.clone(), &current, &shared, &config, selection)?;
        let before = shared.clone();
        let mut run = apply_corrections(
            vec![report],
            Some(StdinCorrection {
                original: &current,
                anonymous: false,
                echo: false,
                shared: Some(&mut shared),
            }),
            configs,
            correct_mode,
            selection,
        )?;
        if run.had_write_failures {
            shared = before;
        }
        if let Some(corrected) = run.stdin_corrected {
            current = corrected;
        }
        let failed = cli.fail_fast && {
            let mut displayed = run.reports.clone();
            filter_displayed_offenses(&mut displayed, cli, fail_level);
            fail_level.failing(&displayed) || run.had_errors
        };
        result.corrected_count += run.corrected_count;
        result.stdin_rewritten |= run.stdin_rewritten;
        result.stdin_rollback |= run.stdin_rollback;
        result.had_errors |= run.had_errors;
        result.had_write_failures |= run.had_write_failures;
        result.reports.append(&mut run.reports);
        if failed {
            break;
        }
    }
    if result.stdin_rollback || result.had_write_failures {
        return rollback_stdin_directory(result, original, configs, selection);
    }
    if correct_mode != CorrectMode::None {
        // 対象0でも本家は受け取った本文を返す。JSON等の抑制時は計画も拒否も増やさない。
        if cfg!(windows) && prints_corrected_stdin(cli) {
            let path = result.reports.last().map_or_else(
                || cli.stdin.as_deref().unwrap(),
                |report| report.path.as_path(),
            );
            let config = configs.for_path(path)?;
            current = match planned_stdin_echo(
                &current,
                original,
                &config,
                result.stdin_rewritten,
                selection,
            ) {
                Ok(bytes) => bytes,
                Err(error) => {
                    report_stdin_echo_failure(path, &error);
                    return rollback_stdin_directory(result, original, configs, selection);
                }
            };
        }
        result.stdin_corrected = Some(current);
    }
    Ok(result)
}

fn rollback_stdin_directory(
    mut run: CorrectionRun,
    original: &[u8],
    configs: &ConfigStore,
    selection: &Selection,
) -> Result<CorrectionRun> {
    // 返す全体が原本へ戻るため、途中の共有本文に対する corrected 記録も残さない。
    run.reports = run
        .reports
        .into_iter()
        .map(|report| {
            let config = configs.for_path(&report.path)?;
            inspect_stdin(report.path, original.to_vec(), &config, selection)
        })
        .collect::<Result<_>>()?;
    run.corrected_count = 0;
    run.stdin_corrected = Some(original.to_vec());
    run.stdin_rollback = true;
    run.had_errors = true;
    run.had_write_failures = true;
    Ok(run)
}

enum StdinEchoFailure {
    Transport(anyhow::Error),
    Syntax,
}

fn planned_stdin_echo(
    bytes: &[u8],
    original: &[u8],
    config: &Config,
    rewritten: bool,
    selection: &Selection,
) -> std::result::Result<Vec<u8>, StdinEchoFailure> {
    let bytes =
        stdin_output_bytes(bytes, config, rewritten).map_err(StdinEchoFailure::Transport)?;
    if bytes.as_ref() != original
        && !selection.skip_syntax_guard
        && std::env::var_os(NO_SYNTAX_GUARD).is_none()
        && crate::prism::would_invalidate(original, &bytes, config.target_ruby_version())
    {
        return Err(StdinEchoFailure::Syntax);
    }
    Ok(bytes.into_owned())
}

fn report_stdin_echo_failure(path: &Path, failure: &StdinEchoFailure) {
    match failure {
        StdinEchoFailure::Transport(error) => {
            eprintln!(
                "refusing to print corrected stdin {}: {error:#}",
                path.display()
            );
        }
        StdinEchoFailure::Syntax => {
            eprintln!(
                "Autocorrection was not written to {} because it introduced a syntax error.",
                path.display()
            );
        }
    }
}

fn apply_corrections(
    reports: Vec<FileReport>,
    mut stdin: Option<StdinCorrection<'_>>,
    configs: &ConfigStore,
    correct_mode: CorrectMode,
    selection: &Selection,
) -> Result<CorrectionRun> {
    let stdin_original = stdin.as_ref().map(|input| input.original);
    let anonymous_stdin = stdin.as_ref().is_some_and(|input| input.anonymous);
    let stdin_echo = stdin.as_ref().is_some_and(|input| input.echo);
    let mut corrected_count = 0;
    let mut stdin_corrected = None;
    let mut stdin_rewritten = false;
    let mut stdin_rollback = false;
    let mut had_errors = false;
    let mut had_write_failures = false;
    let mut corrected_reports = Vec::with_capacity(reports.len());

    for report in reports {
        let original_stdin_report = stdin_original.map(|_| report.clone());
        let path = report.path.clone();
        let target_config = if anonymous_stdin {
            anonymous_stdin_config(configs, &path)?
        } else {
            configs.for_path(&path)?
        };
        // 通常の lint は再読込しない。補正だけ、Ruby が実際に読む符号化を保護に渡す。
        let disk_original = if stdin_original.is_none() && correct_mode != CorrectMode::None {
            Some(fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?)
        } else {
            None
        };
        let outcome =
            if let Some(shared) = stdin.as_mut().and_then(|input| input.shared.as_deref_mut()) {
                correct_shared_stdin(
                    report,
                    correct_mode,
                    &target_config,
                    selection,
                    stdin_original.unwrap(),
                    shared,
                )?
            } else {
                correct_file_with_original(
                    report,
                    correct_mode,
                    &target_config,
                    selection,
                    stdin_original.or(disk_original.as_deref()),
                    stdin_original.is_none(),
                )?
            };
        stdin_rewritten |= stdin_original.is_some() && outcome.rewritten;
        stdin_rollback |= outcome.rollback.is_some();
        if let Some(message) = &outcome.infinite_loop {
            // 本家は反復が収束しないファイルでも最後の補正を書き、残りの処理を続ける。
            eprintln!("{message}");
            had_errors = true;
        }
        // ディスクに残る原本は有効な Ruby なので、存在しない構文違反を報告しない。
        // 補正失敗を終了コードへ伝え、書き戻さなかった実行を CI の成功にしない。
        if let Some(message) = &outcome.rollback {
            eprintln!("{message}");
            had_errors = true;
            had_write_failures = true;
        }
        if let Some(original) = stdin_original {
            if correct_mode != CorrectMode::None {
                let bytes = if outcome.rewritten {
                    match correction_bytes(
                        &outcome.text,
                        original,
                        Some(outcome.introduced_non_ascii),
                    ) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            eprintln!("refusing to rewrite {}: {error:#}", path.display());
                            had_errors = true;
                            had_write_failures = true;
                            stdin_corrected = Some(original.to_vec());
                            corrected_reports.extend(original_stdin_report);
                            continue;
                        }
                    }
                } else {
                    original.to_vec()
                };
                // Windows の transport が加える変更を検査し、Unix は既存の補正判定を使う。
                if stdin_echo && cfg!(windows) && outcome.rollback.is_none() {
                    let bytes = match planned_stdin_echo(
                        &bytes,
                        original,
                        &target_config,
                        outcome.rewritten,
                        selection,
                    ) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            report_stdin_echo_failure(&path, &error);
                            had_errors = true;
                            had_write_failures = true;
                            stdin_corrected = Some(original.to_vec());
                            corrected_reports.extend(original_stdin_report);
                            continue;
                        }
                    };
                    stdin_corrected = Some(bytes);
                } else {
                    stdin_corrected = Some(bytes);
                }
            }
        } else if outcome.rewritten
            && let Err(error) = write_correction(&path, &outcome)
        {
            if !error.is::<CorrectionEncodingError>() {
                return Err(error);
            }
            // 符号化の拒否は書き込み前なので、原本を再検査して未適用の診断を返せる。
            // I/O エラーをここで扱うと、途中まで書いた内容を原本と誤認する。
            eprintln!("{error:#}");
            had_errors = true;
            had_write_failures = true;
            corrected_reports.extend(inspect_files(
                std::slice::from_ref(&path),
                &target_config,
                selection,
                false,
            )?);
            continue;
        }
        corrected_count += outcome.corrected_count;
        corrected_reports.push(outcome.report);
    }

    Ok(CorrectionRun {
        reports: corrected_reports,
        corrected_count,
        stdin_corrected,
        stdin_rewritten,
        stdin_rollback,
        had_errors,
        had_write_failures,
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
                quoted_yaml_description(description.lines().next().unwrap_or(""))
            );
        }
        // NewCops による実行時の選択と、カタログの pending は別物。
        // pending の解決だけを切り替え、部門や EnabledByDefault の上書きは保持する。
        let enabled = match (
            config.rule_enabled_with_pending(name, true, false),
            config.rule_enabled_with_pending(name, false, true),
        ) {
            (true, false) => "pending",
            (true, true) => "true",
            _ => "false",
        };
        println!("  Enabled: {enabled}");
        println!("  Implemented: {}", implemented.contains(name));
    }
}

fn quoted_yaml_description(description: &str) -> String {
    use std::fmt::Write as _;

    // JSON の引用で no などの暗黙変換を防ぎ、YAML 1.1 が拒否・正規化する文字も逃がす。
    let quoted = serde_json::to_string(description).expect("string descriptions must serialize");
    let mut escaped = String::with_capacity(quoted.len());
    for character in quoted.chars() {
        if matches!(
            character,
            '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}' | '\u{fffe}' | '\u{ffff}'
        ) {
            write!(&mut escaped, "\\u{:04X}", u32::from(character))
                .expect("writing to a string cannot fail");
        } else {
            escaped.push(character);
        }
    }
    escaped
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
    let display_cop_names = message_display_cop_names(cli, config);
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

/// `ExecuteRunner::INTEGRATION_FORMATTERS`: 全体を機械が読むため本文を追加しない形式。
/// 短い別名も本家と同じく最後の --format に対して判定する。
const INTEGRATION_FORMATTERS: [&str; 6] = ["h", "html", "j", "json", "ju", "junit"];

fn prints_corrected_stdin(cli: &Cli) -> bool {
    !cli.formats
        .last()
        .is_some_and(|format| INTEGRATION_FORMATTERS.contains(&format.as_str()))
}

/// `ExecuteRunner#maybe_print_corrected_source` と同じ順で report・区切り・本文を返す。
/// --stderr は区切りだけを移す。本家の本文 print は runner の redirect 解除後に行われる。
/// 本文は encoding と構文を検査した最終 bytes なので、ここでは改行を変換しない。
fn print_corrected_stdin(cli: &Cli, corrected: &[u8]) -> Result<()> {
    if !prints_corrected_stdin(cli) {
        return Ok(());
    }
    let separator = native_textmode_bytes(b"====================\n");
    if cli.stderr {
        io::stderr().write_all(&separator)?;
    } else {
        io::stdout().write_all(&separator)?;
    }
    io::stdout().write_all(corrected)?;
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
    let output = native_textmode_bytes(output.as_bytes());
    if let Some(path) = path {
        fs::write(path, &output).with_context(|| format!("failed to write {}", path.display()))?;
    } else if stderr {
        io::stderr().write_all(&output)?;
    } else {
        io::stdout().write_all(&output)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Cli, native_textmode_bytes};
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn result_cache_option_projection_matches_recorded_rubocop() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/cached_message_annotations_windows.json"
        ))
        .unwrap();
        // 新しい CLI フラグは relevant / 除外 / 未実装等の判断を明記してから追加する。
        let mut classified: Vec<_> = fixture["cli_option_policy"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|values| {
                values
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap())
            })
            .collect();
        classified.sort_unstable();
        let command = Cli::command();
        let mut defined: Vec<_> = command
            .get_arguments()
            .map(|argument| argument.get_id().as_str())
            .collect();
        defined.sort_unstable();
        assert_eq!(defined, classified);
        for case in fixture["option_keys"].as_array().unwrap() {
            let arguments: Vec<String> = std::iter::once("sonicop".to_owned())
                .chain(
                    case["arguments"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned()),
                )
                .collect();
            Cli::command().try_get_matches_from(&arguments).unwrap();
            assert_eq!(
                super::cache_message_options(&arguments),
                case["normalized"],
                "{arguments:?}"
            );
        }
        for case in fixture["output_path_keys"].as_array().unwrap() {
            let mut arguments = vec!["sonicop".to_owned()];
            if let Some(path) = case["value"].as_str() {
                arguments.extend(["--out".to_owned(), path.to_owned()]);
            }
            arguments.extend(["--format".to_owned(), "json".to_owned()]);
            // 空 out も含む 19 件は Options API の記録で、CLI の受理は主張しない。
            assert_eq!(
                super::cache_message_options(&arguments),
                case["normalized"],
                "{arguments:?}"
            );
        }
        let inspect = &fixture["inspect_keys"];
        assert_eq!(inspect["unicode_version"], "17.0.0");
        assert_eq!(inspect["default_external"], "UTF-8");
        for case in inspect["cases"].as_array().unwrap() {
            let value = case["value"].as_str().unwrap();
            assert_eq!(
                super::cache_inspect_string(value),
                case["inspected"],
                "{value:?}"
            );
            let arguments = vec![
                "sonicop".to_owned(),
                "--out".to_owned(),
                value.to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
            ];
            assert_eq!(
                super::cache_message_options(&arguments),
                case["normalized"],
                "{value:?}"
            );
        }
        // 単文字 inspect を全非 ASCII scalar で取得した区間。分類の依存更新による差も拒む。
        let ranges: Vec<_> = inspect["nonprintable_ranges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|range| {
                (
                    range[0].as_u64().unwrap() as u32,
                    range[1].as_u64().unwrap() as u32,
                )
            })
            .collect();
        let mut index = 0;
        let mut checked = 0;
        for codepoint in 128..=0x10FFFF {
            let Some(character) = char::from_u32(codepoint) else {
                continue;
            };
            while index < ranges.len() && ranges[index].1 < codepoint {
                index += 1;
            }
            let escaped = index < ranges.len() && ranges[index].0 <= codepoint;
            assert_eq!(
                super::cache_nonprintable(character),
                escaped,
                "U+{codepoint:04X}"
            );
            checked += 1;
        }
        assert_eq!(checked, 1_111_936);
    }

    #[test]
    fn external_arguments_match_recorded_ruby_shellwords() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/shellwords_options.json"))
                .unwrap();
        let decode = |hex: &str| {
            (0..hex.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 43);
        for case in cases {
            let source = String::from_utf8(decode(case["source_hex"].as_str().unwrap())).unwrap();
            let found = super::split_external_arguments(&source);
            if case["reference"]["error_class"].is_string() {
                assert!(found.is_err(), "{}", case["id"]);
            } else {
                let expected: Vec<_> = case["reference"]["words_hex"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|word| decode(word.as_str().unwrap()))
                    .collect();
                let actual: Vec<_> = found.unwrap().into_iter().map(String::into_bytes).collect();
                assert_eq!(actual, expected, "{}", case["id"]);
            }
        }
    }

    #[test]
    fn compact_json_textmode_borrows_the_original_bytes() {
        // compact JSON は生の LF を持たない。本家と同じ bytes を複写せず一度で書く。
        let json = br#"{"files":[],"summary":{"offense_count":0}}"#;
        assert!(matches!(
            native_textmode_bytes(json),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
