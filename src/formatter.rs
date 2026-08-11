use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::config::Config;
use crate::diagnostic::{FileReport, Location, Offense, Severity};
use crate::{RUBOCOP_COMPAT_FULL_VERSION, VERSION};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Progress,
    Simple,
    Clang,
    Emacs,
    Json,
    Junit,
    Html,
    Markdown,
    Github,
    Tap,
    Files,
    Fuubar,
    Offenses,
    Worst,
    Quiet,
    Pacman,
    Autogenconf,
}

impl Format {
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "progress" | "p" => Ok(Self::Progress),
            "simple" | "s" => Ok(Self::Simple),
            "clang" | "c" => Ok(Self::Clang),
            "emacs" | "e" => Ok(Self::Emacs),
            "json" | "j" => Ok(Self::Json),
            "junit" | "ju" => Ok(Self::Junit),
            "html" | "h" => Ok(Self::Html),
            "markdown" | "m" => Ok(Self::Markdown),
            "github" | "g" => Ok(Self::Github),
            "tap" | "t" => Ok(Self::Tap),
            "files" | "file-list" | "fi" => Ok(Self::Files),
            "fuubar" | "fu" => Ok(Self::Fuubar),
            "offenses" | "o" => Ok(Self::Offenses),
            "worst" | "w" => Ok(Self::Worst),
            "quiet" | "q" => Ok(Self::Quiet),
            "pacman" | "pa" => Ok(Self::Pacman),
            "autogenconf" | "a" => Ok(Self::Autogenconf),
            _ => bail!(
                "unknown formatter: {value}. Available formatters: progress, simple, clang, emacs, json, junit, html, markdown, github, tap, files, fuubar, offenses, worst, quiet, pacman, autogenconf"
            ),
        }
    }
}

pub struct FormatOptions<'a> {
    pub cwd: &'a Path,
    pub config: &'a Config,
    pub display_cop_names: bool,
    pub display_style_guide: bool,
    pub extra_details: bool,
    pub color: bool,
    pub corrected_count: usize,
    pub fail_level: Severity,
    /// True for `-a`, where RuboCop points at `-A` instead of calling the rest autocorrectable.
    pub safe_autocorrect: bool,
}

pub fn render(
    format: Format,
    reports: &[FileReport],
    options: &FormatOptions<'_>,
) -> Result<String> {
    match format {
        Format::Json => render_json(reports, options),
        Format::Emacs => Ok(render_emacs(reports, options)),
        Format::Github => Ok(render_github(reports, options)),
        Format::Junit => Ok(render_junit(reports, options)),
        Format::Html => Ok(render_html(reports, options)),
        Format::Markdown => Ok(render_markdown(reports, options)),
        Format::Tap => Ok(render_tap(reports, options)),
        Format::Files => Ok(render_files(reports, options.cwd)),
        Format::Offenses => Ok(render_offense_counts(reports)),
        Format::Worst => Ok(render_worst(reports, options.cwd)),
        Format::Autogenconf => Ok(render_autogenconf(reports, options.cwd)),
        Format::Simple => Ok(render_simple(reports, options, false)),
        Format::Quiet => Ok(render_simple(reports, options, true)),
        Format::Clang => Ok(render_clang(reports, options, false)),
        Format::Progress | Format::Fuubar | Format::Pacman => {
            Ok(render_clang(reports, options, true))
        }
    }
}

/// `quiet` is RuboCop's `SimpleTextFormatter` with the summary skipped when nothing was found,
/// which for a clean run leaves no output at all.
fn render_simple(
    reports: &[FileReport],
    options: &FormatOptions<'_>,
    silent_when_clean: bool,
) -> String {
    let mut output = String::new();
    if silent_when_clean && offense_count(reports) == 0 {
        return output;
    }
    for report in reports {
        if report.offenses.is_empty() {
            continue;
        }
        let path = smart_path(&report.path, options.cwd);
        output.push_str(&paint(&format!("== {path} ==\n"), "33", options.color));
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            let message = display_message(offense, options);
            let line = format!(
                "{}:{:>3}:{:>3}: {message}\n",
                offense.severity.code(),
                location.line,
                location.column
            );
            output.push_str(&paint(
                &line,
                severity_color(offense.severity),
                options.color,
            ));
        }
    }
    output.push('\n');
    output.push_str(&summary(reports, options));
    output
}

fn render_clang(reports: &[FileReport], options: &FormatOptions<'_>, progress: bool) -> String {
    let mut output = String::new();
    if progress {
        output.push_str(&format!("Inspecting {}\n", plural(reports.len(), "file")));
        for report in reports {
            let severity = report.offenses.iter().map(|offense| offense.severity).max();
            output.push(severity.map_or('.', Severity::code));
        }
        output.push_str("\n\n");
        if reports.iter().any(|report| !report.offenses.is_empty()) {
            output.push_str("Offenses:\n\n");
        }
    }
    for report in reports {
        let path = smart_path(&report.path, options.cwd);
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            let message = display_message(offense, options);
            output.push_str(&format!(
                "{path}:{}:{}: {}: {message}\n",
                location.line,
                location.column,
                offense.severity.code()
            ));
            let source_line = offense
                .source_line(&report.source)
                .trim_end_matches(['\r', '\n']);
            output.push_str(source_line);
            output.push('\n');
            output.push_str(&" ".repeat(location.column.saturating_sub(1)));
            output.push_str(&"^".repeat(location.length.max(1)));
            output.push('\n');
        }
    }
    output.push('\n');
    output.push_str(&summary(reports, options));
    output
}

/// RuboCop's `EmacsStyleFormatter` prints the raw target path rather than routing it through
/// `smart_path`, so this one stays unrelativized on purpose.
fn render_emacs(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut output = String::new();
    for report in reports {
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.push_str(&format!(
                "{}:{}:{}: {}: {}\n",
                report.path.display(),
                location.line,
                location.column,
                offense.severity.code(),
                display_message(offense, options)
            ));
        }
    }
    output
}

fn render_github(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut output = String::new();
    for report in reports {
        let path = smart_path(&report.path, options.cwd);
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            let level = if offense.severity >= options.fail_level {
                "error"
            } else {
                "warning"
            };
            // RuboCop separates annotations with a single leading newline and escapes only the
            // message, then closes the stream with one final newline.
            output.push_str(&format!(
                "\n::{level} file={path},line={},col={}::{}",
                location.line,
                location.column,
                github_escape(&display_message(offense, options))
            ));
        }
    }
    output.push('\n');
    output
}

fn render_tap(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut output = format!("1..{}\n", reports.len());
    for (index, report) in reports.iter().enumerate() {
        let path = smart_path(&report.path, options.cwd);
        if report.offenses.is_empty() {
            output.push_str(&format!("ok {} - {path}\n", index + 1));
            continue;
        }
        output.push_str(&format!("not ok {} - {path}\n", index + 1));
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.push_str(&format!(
                "{path}:{}:{}: {}: {}\n",
                location.line,
                location.column,
                offense.severity.code(),
                display_message(offense, options)
            ));
        }
    }
    output
}

fn render_junit(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let tests = reports.len();
    let failures = reports
        .iter()
        .filter(|report| !report.offenses.is_empty())
        .count();
    let mut output = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"rubocop\" tests=\"{tests}\" failures=\"{failures}\">\n"
    );
    for report in reports {
        let path = smart_path(&report.path, options.cwd);
        output.push_str(&format!("  <testcase name=\"{}\">", xml_escape(&path)));
        if report.offenses.is_empty() {
            output.push_str("</testcase>\n");
            continue;
        }
        let failures = report
            .offenses
            .iter()
            .map(|offense| {
                let location = offense.location(&report.source);
                format!(
                    "{}:{}: {}",
                    location.line,
                    location.column,
                    display_message(offense, options)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        output.push_str(&format!(
            "<failure message=\"offenses\">{}</failure></testcase>\n",
            xml_escape(&failures)
        ));
    }
    output.push_str("</testsuite>\n");
    output
}

fn render_html(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut output =
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Sonicop</title></head><body><h1>Sonicop report</h1><ul>"
            .to_owned();
    for report in reports {
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.push_str(&format!(
                "<li><code>{}:{}:{}</code> {}</li>",
                xml_escape(&smart_path(&report.path, options.cwd)),
                location.line,
                location.column,
                xml_escape(&display_message(offense, options))
            ));
        }
    }
    output.push_str("</ul></body></html>\n");
    output
}

fn render_markdown(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut output =
        "# Sonicop report\n\n| File | Line | Column | Severity | Message |\n|---|---:|---:|---|---|\n"
            .to_owned();
    for report in reports {
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                smart_path(&report.path, options.cwd).replace('|', "\\|"),
                location.line,
                location.column,
                offense.severity.as_str(),
                display_message(offense, options).replace('|', "\\|")
            ));
        }
    }
    output
}

fn render_files(reports: &[FileReport], cwd: &Path) -> String {
    let mut output = String::new();
    for report in reports.iter().filter(|report| !report.offenses.is_empty()) {
        output.push_str(&smart_path(&report.path, cwd));
        output.push('\n');
    }
    output
}

fn render_offense_counts(reports: &[FileReport]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for offense in reports.iter().flat_map(|report| &report.offenses) {
        *counts.entry(offense.cop_name).or_default() += 1;
    }
    let mut rows = counts.into_iter().collect::<Vec<_>>();
    rows.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    rows.into_iter()
        .map(|(cop, count)| format!("{count:>6}  {cop}\n"))
        .collect()
}

fn render_worst(reports: &[FileReport], cwd: &Path) -> String {
    let mut rows = reports
        .iter()
        .filter(|report| !report.offenses.is_empty())
        .map(|report| (report.offenses.len(), smart_path(&report.path, cwd)))
        .collect::<Vec<_>>();
    rows.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    rows.into_iter()
        .map(|(count, path)| format!("{count:>6}  {path}\n"))
        .collect()
}

/// One cop's offenses as the config-generating outputs need them. RuboCop's
/// `DisabledConfigFormatter` keeps two separate tallies (`disabled_config_formatter.rb:57-63`):
/// every offense feeds `# Offense count:` (:165), while the exclude limit is weighed against the
/// *files* those offenses came from (:161, :237). Collapsing the two into one number silently
/// changes both outputs, so they are modelled apart here.
#[derive(Default)]
pub(crate) struct CopOffenses {
    pub(crate) offense_count: usize,
    /// Paths relative to the run's working directory, in the sorted and deduplicated form
    /// RuboCop writes its `Exclude` list from.
    pub(crate) paths: BTreeSet<String>,
}

/// Group offenses by cop for `--auto-gen-config` and the `autogenconf` formatter. Cops come out in
/// name order, matching RuboCop's `@cops_with_offenses.sort`.
pub(crate) fn offenses_by_cop(
    reports: &[FileReport],
    cwd: &Path,
) -> BTreeMap<&'static str, CopOffenses> {
    let mut by_cop: BTreeMap<&'static str, CopOffenses> = BTreeMap::new();
    for report in reports {
        let path = smart_path(&report.path, cwd);
        for offense in &report.offenses {
            let entry = by_cop.entry(offense.cop_name).or_default();
            entry.offense_count += 1;
            // Every offense in a report shares the one path, which `insert` alone would clone
            // again for each of them.
            if !entry.paths.contains(&path) {
                entry.paths.insert(path.clone());
            }
        }
    }
    by_cop
}

fn render_autogenconf(reports: &[FileReport], cwd: &Path) -> String {
    let mut output = String::new();
    for (cop, offenses) in offenses_by_cop(reports, cwd) {
        output.push_str(&format!("{cop}:\n  Exclude:\n"));
        for path in &offenses.paths {
            output.push_str(&format!("    - {}\n", yaml_single_quoted(path)));
        }
    }
    output
}

#[derive(Serialize)]
struct JsonOutput<'a> {
    metadata: Metadata,
    files: Vec<JsonFile<'a>>,
    summary: Summary,
}

#[derive(Serialize)]
struct Metadata {
    rubocop_version: &'static str,
    sonicop_version: &'static str,
    ruby_engine: &'static str,
    ruby_version: &'static str,
    ruby_patchlevel: &'static str,
    ruby_platform: &'static str,
}

#[derive(Serialize)]
struct JsonFile<'a> {
    path: String,
    offenses: Vec<JsonOffense<'a>>,
}

#[derive(Serialize)]
struct JsonOffense<'a> {
    severity: Severity,
    message: String,
    cop_name: &'static str,
    corrected: bool,
    correctable: bool,
    location: Location,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    suppressed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    justification: Option<&'a str>,
}

#[derive(Serialize)]
struct Summary {
    offense_count: usize,
    target_file_count: usize,
    inspected_file_count: usize,
}

fn render_json(reports: &[FileReport], options: &FormatOptions<'_>) -> Result<String> {
    let files = reports
        .iter()
        .map(|report| JsonFile {
            path: smart_path(&report.path, options.cwd),
            offenses: report
                .offenses
                .iter()
                .map(|offense| JsonOffense {
                    severity: offense.severity,
                    message: offense.message.clone(),
                    cop_name: offense.cop_name,
                    corrected: offense.corrected,
                    correctable: offense.is_correctable(),
                    location: offense.location(&report.source),
                    suppressed: offense.suppressed,
                    justification: offense.justification.as_deref(),
                })
                .collect(),
        })
        .collect();
    let offense_count = reports.iter().map(|report| report.offenses.len()).sum();
    let output = JsonOutput {
        metadata: Metadata {
            rubocop_version: RUBOCOP_COMPAT_FULL_VERSION,
            sonicop_version: VERSION,
            ruby_engine: "sonicop",
            ruby_version: "n/a",
            ruby_patchlevel: "0",
            ruby_platform: std::env::consts::OS,
        },
        files,
        summary: Summary {
            offense_count,
            target_file_count: reports.len(),
            inspected_file_count: reports.len(),
        },
    };
    Ok(serde_json::to_string(&output)?)
}

fn display_message(offense: &Offense, options: &FormatOptions<'_>) -> String {
    let status = if offense.suppressed {
        "[Suppressed] "
    } else if offense.corrected {
        "[Corrected] "
    } else {
        ""
    };
    let cop = if options.display_cop_names {
        format!("{}: ", offense.cop_name)
    } else {
        String::new()
    };
    let mut message = format!("{status}{cop}{}", offense.message);
    if options.display_style_guide
        && let Some(anchor) = options
            .config
            .cop_value::<String>(offense.cop_name, "StyleGuide")
    {
        let base = options
            .config
            .all_cops_value::<String>("StyleGuideBaseURL")
            .unwrap_or_else(|| "https://rubystyle.guide".to_owned());
        message.push_str(&format!(" ({base}{anchor})"));
    }
    if options.extra_details
        && let Some(description) = options.config.description(offense.cop_name)
    {
        message.push_str(&format!(" {}", description.trim()));
    }
    message
}

/// RuboCop's `@total_offense_count`: every offense the formatter was handed, corrected and
/// suppressed ones included. The exit code applies its own, narrower predicate.
fn offense_count(reports: &[FileReport]) -> usize {
    reports.iter().map(|report| report.offenses.len()).sum()
}

fn summary(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let offense_count = offense_count(reports);
    let correctable_count = reports
        .iter()
        .flat_map(|report| &report.offenses)
        .filter(|offense| offense.is_correctable() && !offense.corrected)
        .count();
    let offenses = if offense_count == 0 {
        "no offenses".to_owned()
    } else {
        plural(offense_count, "offense")
    };
    let mut output = format!(
        "{} inspected, {offenses} detected",
        plural(reports.len(), "file")
    );
    if options.corrected_count > 0 {
        output.push_str(&format!(
            ", {} corrected",
            plural(options.corrected_count, "offense")
        ));
    }
    if correctable_count > 0 {
        if options.safe_autocorrect {
            output.push_str(&format!(
                ", {} can be corrected with `rubocop -A`",
                plural(correctable_count, "more offense")
            ));
        } else {
            output.push_str(&format!(
                ", {} autocorrectable",
                plural(correctable_count, "offense")
            ));
        }
    }
    output.push('\n');
    output
}

/// The path as RuboCop prints it: relative to the run's directory, with `/` separators on every
/// platform. Ruby normalizes separators, so a Windows run that emitted `lib\a.rb` would not match
/// upstream output nor the `Include`/`Exclude` patterns users copy out of it.
pub(crate) fn smart_path(path: &Path, cwd: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn github_escape(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// A value as a YAML single-quoted scalar. Doubling `'` is the only escape such a scalar has, and
/// leaving it out lets a path containing one close the scalar early, so the generated config no
/// longer parses. RuboCop interpolates the path unescaped (`disabled_config_formatter.rb:283`)
/// and emits the broken YAML; Sonicop keeps its own output loadable instead.
pub(crate) fn yaml_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn severity_color(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "90",
        Severity::Refactor | Severity::Convention => "33",
        Severity::Warning => "35",
        Severity::Error | Severity::Fatal => "31",
    }
}

fn paint(text: &str, color: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{color}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{offenses_by_cop, render_autogenconf, yaml_single_quoted};
    use crate::diagnostic::{FileReport, Offense, Severity};
    use crate::source::SourceFile;

    fn report(path: PathBuf, cops: &[&'static str]) -> FileReport {
        FileReport {
            source: SourceFile::new(path.clone(), "value = 1\n".to_owned()),
            path,
            offenses: cops
                .iter()
                .map(|cop| Offense::new(cop, Severity::Convention, "offense", 0, 1))
                .collect(),
        }
    }

    #[test]
    fn counts_every_offense_but_lists_each_file_once() {
        let cwd = Path::new("project");
        let reports = [
            report(
                cwd.join("a.rb"),
                &[
                    "Layout/TrailingWhitespace",
                    "Layout/TrailingWhitespace",
                    "Style/FrozenStringLiteralComment",
                ],
            ),
            report(cwd.join("b.rb"), &["Layout/TrailingWhitespace"]),
        ];

        let by_cop = offenses_by_cop(&reports, cwd);

        let trailing = &by_cop["Layout/TrailingWhitespace"];
        assert_eq!(trailing.offense_count, 3);
        assert_eq!(
            trailing
                .paths
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["a.rb", "b.rb"]
        );
        assert_eq!(by_cop["Style/FrozenStringLiteralComment"].offense_count, 1);
    }

    #[test]
    fn single_quoted_scalars_double_an_embedded_quote() {
        assert_eq!(yaml_single_quoted("plain.rb"), "'plain.rb'");
        assert_eq!(yaml_single_quoted("it's/a.rb"), "'it''s/a.rb'");
    }

    #[test]
    fn autogenconf_escapes_quotes_in_excluded_paths() {
        let cwd = Path::new("project");
        let reports = [report(cwd.join("it's.rb"), &["Layout/TrailingWhitespace"])];

        assert_eq!(
            render_autogenconf(&reports, cwd),
            "Layout/TrailingWhitespace:\n  Exclude:\n    - 'it''s.rb'\n"
        );
    }
}
