use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::config::Config;
use crate::cop_name;
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
        Format::Offenses => Ok(render_offense_counts(reports, options)),
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
            let message = annotate_message(&display_message(offense, options));
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
        // `ProgressFormatter#finished` closes the line of marks, and only a run that found
        // something opens the offence listing after it -- a clean run goes straight to the summary,
        // whose own leading blank line is the one below.
        output.push('\n');
        if reports.iter().any(|report| !report.offenses.is_empty()) {
            output.push_str("\nOffenses:\n\n");
        }
    }
    for report in reports {
        let path = smart_path(&report.path, options.cwd);
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            let message = annotate_message(&display_message(offense, options));
            output.push_str(&format!(
                "{path}:{}:{}: {}: {message}\n",
                location.line,
                location.column,
                offense.severity.code()
            ));
            if let Some((quoted, carets)) = source_excerpt(offense, report) {
                output.push_str(&quoted);
                output.push('\n');
                output.push_str(&carets);
                output.push('\n');
            }
        }
    }
    output.push('\n');
    output.push_str(&summary(reports, options));
    output
}

/// RuboCop's `EmacsStyleFormatter` prints the path it was handed rather than routing it through
/// `smart_path`, and the paths it holds are the absolute ones file discovery produced -- which is
/// the point of the format, since an editor jumps to the file without knowing where the run started.
fn render_emacs(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut output = String::new();
    for report in reports {
        let path = if report.path.is_absolute() {
            report.path.clone()
        } else {
            options.cwd.join(&report.path)
        };
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.push_str(&format!(
                "{}:{}:{}: {}: {}\n",
                path.display(),
                location.line,
                location.column,
                offense.severity.code(),
                // `EmacsStyleFormatter#message` closes with `tr("\n", ' ')`: one offense is one
                // line, which is what makes the format machine-parsable, and a cop whose message
                // spans lines must not break that.
                display_message(offense, options).replace('\n', " ")
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
                github_escape(&annotated_message(offense, options))
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
        // `TapFormatter` is a `ClangStyleFormatter` whose every line is a TAP comment, so it carries
        // the offending line and its carets across as well.
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.push_str(&format!(
                "# {path}:{}:{}: {}: {}\n",
                location.line,
                location.column,
                offense.severity.code(),
                annotate_message(&display_message(offense, options))
            ));
            if let Some((quoted, carets)) = source_excerpt(offense, report) {
                output.push_str(&format!("# {quoted}\n# {carets}\n"));
            }
        }
    }
    // It inherits the summary along with the rest of `ClangStyleFormatter`.
    output.push('\n');
    output.push_str(&summary(reports, options));
    output
}

/// `Offense::NO_LOCATION.to_s`. `PseudoSourceRange` is a plain `Struct`, so it inherits
/// `Struct#to_s` -- an inspection of the singleton's frozen fields, which is a constant.
const PSEUDO_SOURCE_RANGE: &str = "#<struct RuboCop::Cop::Offense::PseudoSourceRange line=1, column=0, source_line=\"\", begin_pos=0, end_pos=0>";

/// RuboCop's `JUnitFormatter`, which reports one `<testcase>` per *cop* per file rather than one
/// per file.
///
/// `file_finished` (`junit_formatter.rb:36-46`) walks `Cop::Registry.all` for every inspected file
/// and emits a test case whether or not that cop found anything, so a two-file run yields 1,218
/// test cases. Its own comment calls this out as inherited from rubocop-junit-formatter and worth
/// narrowing to enabled cops one day; until upstream does, a report that lists only the offending
/// cops is a different document to every JUnit consumer -- a cop that stops firing has to show up
/// as a passing test, not as a test that vanished.
fn render_junit(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut body = String::new();
    // `@offense_count` is accumulated inside the per-cop loop, so an offense from a cop the
    // registry does not know is left out of the failure tally as well as out of the body.
    let mut offense_count = 0usize;
    for report in reports {
        let classname = xml_escape(&junit_classname(&report.path, options.cwd));
        let path = absolute_path(&report.path, options.cwd);
        let mut by_cop: BTreeMap<&str, Vec<&Offense>> = BTreeMap::new();
        for offense in &report.offenses {
            by_cop.entry(offense.cop_name).or_default().push(offense);
        }
        for cop in COP_REGISTRY_ORDER {
            let offenses = by_cop.remove(cop).unwrap_or_default();
            offense_count += offenses.len();
            if offenses.is_empty() {
                body.push_str(&format!(
                    "    <testcase classname='{classname}' name='{cop}'/>\n"
                ));
                continue;
            }
            body.push_str(&format!(
                "    <testcase classname='{classname}' name='{cop}'>\n"
            ));
            for offense in offenses {
                let location = offense.location(&report.source);
                // `FailureElement#text` is `offense.location.to_s`, and a `Parser::Source::Range`
                // names itself after its buffer -- the absolute path RuboCop opened, not the
                // shortened one the `classname` carries. A global offense has no range at all, so
                // what lands here is `Struct#to_s` over the frozen `NO_LOCATION` singleton.
                let text = if is_global_offense(offense) {
                    PSEUDO_SOURCE_RANGE.to_owned()
                } else {
                    format!("{path}:{}:{}", location.line, location.column)
                };
                body.push_str(&format!(
                    "      <failure type='{cop}' message='{}'>\n        {}\n      </failure>\n",
                    xml_escape(&annotated_message(offense, options)),
                    xml_escape(&text)
                ));
            }
            body.push_str("    </testcase>\n");
        }
    }
    format!(
        "<?xml version='1.0'?>\n<testsuites>\n  <testsuite name='rubocop' tests='{}' failures='{offense_count}'>\n{body}  </testsuite>\n</testsuites>\n",
        reports.len()
    )
}

/// The `classname` attribute of a JUnit test case (`junit_formatter.rb:92-97`): the file's path
/// with `.rb` cut off, the run's directory dropped and the separators turned into dots, so
/// `/project/lib/a.rb` reports as `lib.a`. The suffix goes first, which is why a path that is not
/// a `.rb` file keeps its extension in the class name.
fn junit_classname(path: &Path, cwd: &Path) -> String {
    let path = absolute_path(path, cwd);
    let prefix = format!("{}/", absolute_path(cwd, cwd));
    path.strip_suffix(".rb")
        .unwrap_or(&path)
        .replace(&prefix, "")
        .replace('/', ".")
}

/// RuboCop's `HTMLFormatter`, which renders `assets/output.html.erb`.
///
/// The template is reproduced here chunk by chunk in its own order, because its layout is not
/// incidental: ERB emits the literal text around a tag as well as the value inside it, so the runs
/// of indentation that surround `<% ... %>` end up in the document as lines of their own. RuboCop
/// then flattens every all-whitespace line to a bare newline (`html_formatter.rb:56-58`), which is
/// why the report is punctuated by blank lines whose count tracks the loops that produced them.
fn render_html(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    // `ERBContext#initialize` sorts, so the report is ordered by path rather than by the order
    // inspection happened to finish files in.
    let mut files = reports.iter().collect::<Vec<_>>();
    files.sort_by_key(|report| absolute_path(&report.path, options.cwd));

    let mut output = String::from(HTML_HEAD);
    output.push_str(HTML_LOGO);
    output.push_str(
        "\" alt=\"\">\n      <h1 class=\"title\">RuboCop Inspection Report</h1>\n    </div>\n",
    );
    output.push_str("    <div class=\"information\">\n      <div class=\"infobox\">\n        <div class=\"total\">\n          ");
    output.push_str(&plural(reports.len(), "file"));
    output.push_str(" inspected,\n          ");
    output.push_str(&if offense_count(reports) == 0 {
        "no offenses".to_owned()
    } else {
        plural(offense_count(reports), "offense")
    });
    output.push_str(" detected:\n        </div>\n        <ul class=\"offenses-list\">\n          ");
    for report in &files {
        // `next if file.offenses.none?` leaves the run of indentation before it behind, so even a
        // file with nothing to report adds a line to the document.
        output.push_str("\n            ");
        if report.offenses.is_empty() {
            continue;
        }
        let path = relative_path(&report.path, options.cwd);
        output.push_str(&format!(
            "\n            <li>\n              <a href=\"#offense_{path}\">\n                {path} - {}\n              </a>\n            </li>\n          ",
            plural(report.offenses.len(), "offense")
        ));
    }
    output.push_str("\n        </ul>\n      </div>\n    </div>\n    <div id=\"offenses\">\n      ");
    for report in &files {
        output.push_str("\n      ");
        if report.offenses.is_empty() {
            output.push_str("\n      ");
            continue;
        }
        let path = relative_path(&report.path, options.cwd);
        output.push_str(&format!(
            "\n      <div class=\"offense-box\" id=\"offense_{path}\">\n        <div class=\"box-title-placeholder\"><h3>&nbsp;</h3></div>\n        <div class=\"box-title\"><h3>{path} - {}</h3></div>\n        <div class=\"offense-reports\">\n          ",
            plural(report.offenses.len(), "offense")
        ));
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            let severity = offense.severity.as_str();
            output.push_str(&format!(
                "\n          <div class=\"report\">\n            <div class=\"meta\">\n              <span class=\"location\">Line #{}</span> \u{2013}\n              <span class=\"severity {severity}\">{severity}:</span>\n              <span class=\"message\">{}</span>\n            </div>\n            ",
                location.line,
                decorated_message(&annotated_message(offense, options))
            ));
            // The guard is on the raw source line, so a global offense -- whose pseudo range
            // carries an empty one -- shows its message without any code under it.
            let source_line = quoted_source_line(offense, report);
            if !source_line.trim().is_empty() {
                output.push_str(&format!(
                    "\n            <pre><code>{}</code></pre>\n            ",
                    highlighted_source_line(source_line, &location, severity)
                ));
            }
            output.push_str("\n          </div>\n          ");
        }
        output.push_str("\n        </div>\n      </div>\n      ");
        output.push_str("\n      ");
    }
    output.push_str(&format!(
        "\n    </div>\n    <footer>\n      Generated by <a href=\"https://github.com/rubocop/rubocop\">RuboCop</a>\n      <span class=\"version\">{RUBOCOP_COMPAT_FULL_VERSION}</span>\n    </footer>\n  </body>\n</html>\n"
    ));
    blank_out_whitespace_lines(&output)
}

/// `html_formatter.rb:56-58`: every line that is nothing but whitespace collapses to a bare
/// newline, so the indentation ERB left behind around its tags does not show up as trailing
/// whitespace throughout the document.
fn blank_out_whitespace_lines(html: &str) -> String {
    html.split_inclusive('\n')
        .map(|line| if line.trim().is_empty() { "\n" } else { line })
        .collect()
}

/// `ERBContext#decorated_message`: the backticked spans of a message become `<code>` elements.
///
/// Only what the backticks enclose is escaped -- RuboCop interpolates the rest of the message into
/// the document as it stands (`html_formatter.rb:82-84`), so a `<` a cop quoted outside backticks
/// reaches the page as markup.
fn decorated_message(message: &str) -> String {
    let mut output = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(open) = rest.find('`') {
        // `/`(.+?)`/` needs at least one character between the two, so an empty pair is left alone.
        let Some(close) = rest[open + 1..].find('`').filter(|close| *close > 0) else {
            break;
        };
        output.push_str(&rest[..open]);
        output.push_str(&format!(
            "<code>{}</code>",
            html_escape(&rest[open + 1..open + 1 + close])
        ));
        rest = &rest[open + close + 2..];
    }
    output.push_str(rest);
    output
}

/// `ERBContext#highlighted_source_line`: the offending line with the part the offense covers
/// wrapped in a `<span class="highlight ...">`, and an ellipsis appended when the range runs past
/// the line. The span is bounded by `Offense#highlighted_area`, the same range `clang` puts its
/// carets under.
fn highlighted_source_line(source_line: &str, location: &Location, severity: &str) -> String {
    let column = location.column.saturating_sub(1);
    let length = if location.last_line == location.start_line {
        location.length
    } else {
        source_line.chars().count().saturating_sub(column)
    };
    let mut characters = source_line.chars();
    let before = characters.by_ref().take(column).collect::<String>();
    let highlighted = characters.by_ref().take(length).collect::<String>();
    let after = characters.collect::<String>();
    format!(
        "{}<span class=\"highlight {severity}\">{}</span>{}{}",
        html_escape(&before),
        html_escape(&highlighted),
        html_escape(&after),
        if location.last_line == location.start_line {
            ""
        } else {
            " <span class=\"extra-code\">...</span>"
        }
    )
}

/// `CGI.escapeHTML`, which differs from the XML escaping the JUnit report uses in spelling the
/// apostrophe as a numeric reference.
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// RuboCop's `MarkdownFormatter` (`markdown_formatter.rb:33-70`): a report headed
/// `# RuboCop Inspection Report`, a one-line summary, then a section per offending file with each
/// offense as a bullet followed by the line it was found on in a fenced block.
fn render_markdown(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut output = String::from("# RuboCop Inspection Report\n\n");
    output.push_str(&format!(
        "{} inspected, {} detected:\n\n",
        plural(reports.len(), "file"),
        // `pluralize(..., no_for_zero: true)`, the wording the summary line alone uses.
        if offense_count(reports) == 0 {
            "no offenses".to_owned()
        } else {
            plural(offense_count(reports), "offense")
        }
    ));
    for report in reports {
        if report.offenses.is_empty() {
            continue;
        }
        output.push_str(&format!(
            "### {} - ({})\n",
            relative_path(&report.path, options.cwd),
            plural(report.offenses.len(), "offense")
        ));
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.push_str(&format!(
                "  * **Line # {} - {}:** {}\n\n",
                location.line,
                offense.severity.as_str(),
                annotated_message(offense, options)
            ));
            let mut code = quoted_source_line(offense, report).to_owned();
            // `write_code` appends an ellipsis for an offense whose range runs past the line it
            // starts on, so the quoted line does not read as the whole of what was flagged.
            if location.last_line != location.start_line {
                code.push_str(" ...");
            }
            // `unless code.blank?`: RuboCop's `String#blank?` (`core_ext/string.rb:15-17`) is
            // `empty? || lstrip.empty?`, so an offense on a blank line quotes nothing at all.
            if !code.trim_start().is_empty() {
                output.push_str(&format!("    ```rb\n    {code}\n    ```\n\n"));
            }
        }
    }
    output
}

/// `FileListFormatter`, which prints the path it was handed rather than a shortened one -- the same
/// absolute path `emacs` reports, since both formats exist to be fed to another program.
fn render_files(reports: &[FileReport], cwd: &Path) -> String {
    let mut output = String::new();
    for report in reports.iter().filter(|report| !report.offenses.is_empty()) {
        output.push_str(&absolute_path(&report.path, cwd));
        output.push('\n');
    }
    output
}

/// `OffenseCountFormatter`: how many offenses each cop found, worst first, then a total.
///
/// `cop_information` (`offense_count_formatter.rb:64-72`) tags a cop that can autocorrect with the
/// safety of doing so. RuboCop asks the cop class, which knows whether it supports autocorrection
/// at all; the registry here carries no such flag, so the run's own offenses stand in for it. The
/// two answers part company only for a cop that can correct but corrected nothing in this run, and
/// closing that gap means recording the capability on `rules::Rule`.
fn render_offense_counts(reports: &[FileReport], options: &FormatOptions<'_>) -> String {
    let mut counts: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
    let mut links: BTreeMap<&str, String> = BTreeMap::new();
    let mut offending_files = 0;
    for report in reports {
        if !report.offenses.is_empty() {
            offending_files += 1;
        }
        for offense in &report.offenses {
            let entry = counts.entry(offense.cop_name).or_default();
            entry.0 += 1;
            entry.1 |= offense.is_correctable();
            if options.display_style_guide {
                links
                    .entry(offense.cop_name)
                    .or_insert_with(|| style_guide_suffix(&annotated_message(offense, options)));
            }
        }
    }
    let mut rows = counts.into_iter().collect::<Vec<_>>();
    // `sort_by { |k, v| [-v, k] }`, so cops tying on count fall back to their name.
    rows.sort_by(|left, right| right.1.0.cmp(&left.1.0).then_with(|| left.0.cmp(right.0)));
    let total = rows.iter().map(|(_, (count, _))| count).sum::<usize>();
    let width = total.to_string().len() + 2;
    let mut output = String::from("\n");
    for (cop, (count, correctable)) in &rows {
        let correctable = if *correctable {
            let safety =
                if options.config.rule_safe(cop) && options.config.rule_safe_autocorrect(cop) {
                    "Safe"
                } else {
                    "Unsafe"
                };
            format!(" [{safety} Correctable]")
        } else {
            String::new()
        };
        output.push_str(&format!(
            "{:<width$}{cop}{correctable}{}\n",
            count.to_string(),
            links.get(cop).map(String::as_str).unwrap_or_default()
        ));
    }
    output.push_str(&format!(
        "--\n{total}  Total in {offending_files} files\n\n"
    ));
    output
}

/// The trailing ` (https://...)` `--display-style-guide` left on a message, which
/// `OffenseCountFormatter` lifts back out with `/ \(http\S+\)\Z/`. The pattern admits no spaces, so
/// a cop that named more than one URL -- they are joined with `, ` -- keeps none of them.
fn style_guide_suffix(message: &str) -> String {
    let Some(open) = message.rfind(" (http") else {
        return String::new();
    };
    let suffix = &message[open..];
    if suffix.ends_with(')') && !suffix[2..suffix.len() - 1].contains(char::is_whitespace) {
        suffix.to_owned()
    } else {
        String::new()
    }
}

/// `WorstOffendersFormatter`: the offending files by descending offense count, then a total.
///
/// `report_summary` (`worst_offenders_formatter.rb:36-52`) pads the count to the width of the
/// total plus two, which is what lines the paths up, and brackets the table with a blank line at
/// either end.
fn render_worst(reports: &[FileReport], cwd: &Path) -> String {
    let mut rows = reports
        .iter()
        .filter(|report| !report.offenses.is_empty())
        .map(|report| (report.offenses.len(), relative_path(&report.path, cwd)))
        .collect::<Vec<_>>();
    // `sort_by { |k, v| [-v, k] }`, so files tying on count fall back to their path.
    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let total = rows.iter().map(|(count, _)| count).sum::<usize>();
    let width = total.to_string().len() + 2;
    let mut output = String::from("\n");
    for (count, path) in &rows {
        output.push_str(&format!("{:<width$}{path}\n", count.to_string()));
    }
    output.push_str(&format!("--\n{total}  Total in {} files\n\n", rows.len()));
    output
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

/// The message with the backticks RuboCop's `simple` and `tap` formatters strip.
///
/// Those two are the only ones that call `annotate_message`, so `Useless assignment to variable -
/// `x`.` reads as `- x.` there and keeps its backticks everywhere else.
fn annotate_message(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(open) = rest.find('`') {
        match rest[open + 1..].find('`') {
            Some(close) => {
                out.push_str(&rest[..open]);
                out.push_str(&rest[open + 1..open + 1 + close]);
                rest = &rest[open + close + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// True for an offense filed against the file as a whole rather than against a range inside it.
///
/// `add_global_offense` (`cop/base.rb:190`) hands the offense `Offense::NO_LOCATION`, a pseudo
/// range built over an *empty* buffer that reports line 1, column 1 and a zero length. The cops
/// that use it -- `Naming/FileName`, `Lint/EmptyFile`, `Style/Copyright`, `Bundler/GemFilename`,
/// `Gemspec/RequiredRubyVersion` when the setting is missing entirely -- therefore have no source
/// line to show, which several formatters can see. Sonicop writes such an offense as an empty range
/// at offset zero (`rules/naming/file_name.rs:45-47`), and no real range shares that shape.
fn is_global_offense(offense: &Offense) -> bool {
    offense.start == 0 && offense.end == 0
}

/// The source line a formatter quotes for an offense, with its line terminator removed. A global
/// offense quotes nothing at all: `NO_LOCATION` carries an empty `source_line`, and that is what
/// makes `clang` drop its caret row and `markdown` its fenced block for one.
fn quoted_source_line<'a>(offense: &'a Offense, report: &'a FileReport) -> &'a str {
    if is_global_offense(offense) {
        ""
    } else {
        offense
            .source_line(&report.source)
            .trim_end_matches(['\r', '\n'])
    }
}

/// The two rows `ClangStyleFormatter` prints under an offense: the line it was found on, and a row
/// of carets marking the part of that line it covers.
///
/// Nothing at all when the line holds only whitespace -- `valid_line?`
/// (`clang_style_formatter.rb:33-35`) drops both rather than point at blanks. `Offense#highlighted_area`
/// (`cop/offense.rb:164-167`) rebuilds the range against that single line, so a range carrying on
/// past it is marked only as far as the line goes and the line itself gains an ellipsis. The
/// leading run keeps the tabs of the text before the offense (`to_whitespace`, :53-55), without
/// which the carets drift away from what they point at in a tab-indented file.
fn source_excerpt(offense: &Offense, report: &FileReport) -> Option<(String, String)> {
    let source_line = quoted_source_line(offense, report);
    if source_line.trim_start().is_empty() {
        return None;
    }
    let location = offense.location(&report.source);
    let column = location.column.saturating_sub(1);
    let (quoted, carets) = if location.last_line == location.start_line {
        (source_line.to_owned(), location.length)
    } else {
        (
            format!("{source_line} ..."),
            source_line.chars().count().saturating_sub(column),
        )
    };
    let prefix = source_line.chars().take(column);
    let (tabs, spaces) = prefix.fold((0, 0), |(tabs, spaces), character| {
        if character == '\t' {
            (tabs + 1, spaces)
        } else {
            (tabs, spaces + 1)
        }
    });
    Some((
        quoted,
        format!(
            "{}{}{}",
            "\t".repeat(tabs),
            " ".repeat(spaces),
            "^".repeat(carets)
        ),
    ))
}

/// The status marker RuboCop's text formatters put in front of a message.
///
/// `SimpleTextFormatter#message` and `EmacsStyleFormatter#message` are the only places that build
/// one, so `[Correctable]` and its siblings belong to those two and the formatters that inherit
/// them -- never to `github`, `junit`, `markdown`, `html` or `json`, which report `offense.message`
/// as the cop wrote it.
fn status_marker(offense: &Offense) -> &'static str {
    if offense.suppressed {
        "[Suppressed] "
    } else if offense.corrected {
        "[Corrected] "
    } else if offense.is_correctable() {
        // RuboCop labels an offense it could have fixed but did not, which is how a plain run tells
        // you that `-a` would have handled it.
        "[Correctable] "
    } else {
        ""
    }
}

fn display_message(offense: &Offense, options: &FormatOptions<'_>) -> String {
    format!(
        "{}{}",
        status_marker(offense),
        annotated_message(offense, options)
    )
}

/// `offense.message` as every formatter receives it.
///
/// A cop hands `add_offense` a bare sentence; `MessageAnnotator#annotate`
/// (`cop/message_annotator.rb:58-66`) folds the cop name, the `Details` text and the style guide
/// links into it before the offense is built, so the annotations are part of the message rather
/// than something a formatter adds.
fn annotated_message(offense: &Offense, options: &FormatOptions<'_>) -> String {
    let cop = if options.display_cop_names {
        format!("{}: ", offense.cop_name)
    } else {
        String::new()
    };
    let mut message = format!("{cop}{}", offense.message);
    // `extra_details?` reads the cop's `Details`, which is a note a project writes for its own
    // cops -- none of the 609 built-in ones ships one, so `-E` is silent on a default config.
    // The cop's `Description` is a different key and belongs to `--show-cops`, not to a message.
    if (options.extra_details
        || options
            .config
            .all_cops_value::<bool>("ExtraDetails")
            .unwrap_or(false))
        && let Some(details) = options
            .config
            .cop_value::<String>(offense.cop_name, "Details")
        && !details.is_empty()
    {
        message.push_str(&format!(" {details}"));
    }
    // `display_style_guide?` needs the list to be non-empty, so a cop that names no URL is left
    // without even the brackets.
    let urls = style_guide_urls(offense.cop_name, options);
    if (options.display_style_guide
        || options
            .config
            .all_cops_value::<bool>("DisplayStyleGuide")
            .unwrap_or(false))
        && !urls.is_empty()
    {
        message.push_str(&format!(" ({})", urls.join(", ")));
    }
    message
}

/// The URLs `--display-style-guide` appends: the cop's `StyleGuide` anchor resolved against
/// `StyleGuideBaseURL`, then everything under `References` (and the legacy singular `Reference`),
/// in that order (`cop/message_annotator.rb:68-107`).
fn style_guide_urls(cop_name: &str, options: &FormatOptions<'_>) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(anchor) = options.config.cop_value::<String>(cop_name, "StyleGuide")
        && !anchor.is_empty()
    {
        // `URI.join` against a base that is only an origin leaves a fragment appended to it, which
        // is the shape every `StyleGuide` in the default configuration takes. A cop that names a
        // whole URL keeps it, since joining an absolute reference discards the base.
        let base = options
            .config
            .cop_value::<String>(cop_name::department(cop_name), "StyleGuideBaseURL")
            .or_else(|| options.config.all_cops_value::<String>("StyleGuideBaseURL"))
            .unwrap_or_default();
        if base.is_empty() || anchor.starts_with("http://") || anchor.starts_with("https://") {
            urls.push(anchor);
        } else {
            urls.push(format!("{base}{anchor}"));
        }
    }
    for key in ["References", "Reference"] {
        // Either spelling takes a single URL or a list of them.
        let references = options
            .config
            .cop_value::<Vec<String>>(cop_name, key)
            .or_else(|| {
                options
                    .config
                    .cop_value::<String>(cop_name, key)
                    .map(|url| vec![url])
            })
            .unwrap_or_default();
        urls.extend(references.into_iter().filter(|url| !url.is_empty()));
    }
    urls
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

/// The path RuboCop opened the file under: file discovery expands every target, so the absolute
/// path is what a `Parser::Source::Buffer` is named after and what `Range#to_s` prints.
fn absolute_path(path: &Path, cwd: &Path) -> String {
    let absolute = if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&cwd.join(path))
    };
    absolute.to_string_lossy().replace('\\', "/")
}

/// RuboCop's `PathUtil.relative_path` (`path_util.rb:25-41`), which `markdown` and `html` use in
/// place of `smart_path`. The difference shows on a target outside the run's directory: this one
/// walks back out through `..`, where `smart_path` gives up and prints the absolute path.
fn relative_path(path: &Path, cwd: &Path) -> String {
    let target = normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    });
    let base = normalize(cwd);
    let segments = target.components().collect::<Vec<_>>();
    let base_segments = base.components().collect::<Vec<_>>();
    let shared = segments
        .iter()
        .zip(&base_segments)
        .take_while(|(left, right)| left == right)
        .count();
    // `Pathname#relative_path_from` raises when the two share no root -- a Windows run reporting a
    // file on another drive -- and RuboCop hands the path back untouched.
    if shared == 0 && !base_segments.is_empty() {
        return target.to_string_lossy().replace('\\', "/");
    }
    std::iter::repeat_n("..", base_segments.len() - shared)
        .map(String::from)
        .chain(
            segments[shared..]
                .iter()
                .map(|component| component.as_os_str().to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>()
        .join("/")
}

/// A path with its `.` and `..` segments resolved, the way `File.expand_path` leaves one. Purely
/// lexical, since the caller has an absolute path in hand and RuboCop does not resolve symlinks
/// here either.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
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

/// Every cop RuboCop 1.89.0 registers, in the order `Cop::Registry.all` yields them.
///
/// `JUnitFormatter#file_finished` walks this list for each inspected file and emits a `<testcase>`
/// per cop, so a cop that found nothing takes up as much of the output as one that did, and the
/// order of the entries is part of the output. That order is neither alphabetical nor derivable
/// from the names: it is the order of the `register_cop` calls in RuboCop's department files
/// (`lib/rubocop/cop/{bundler,gemspec,layout,lint,metrics,migration,naming,security,style}.rb`),
/// which is hand-maintained and disagrees with `config/default.yml` in 248 of the 609 positions.
/// `tests/formatter_conformance.rs` pins the set against `config/default.yml` so that a cop added
/// to one and not the other is caught rather than silently dropped from every JUnit report.
pub(crate) const COP_REGISTRY_ORDER: [&str; 609] = [
    "Bundler/DuplicatedGem",
    "Bundler/DuplicatedGroup",
    "Bundler/GemComment",
    "Bundler/GemFilename",
    "Bundler/GemVersion",
    "Bundler/InsecureProtocolSource",
    "Bundler/OrderedGems",
    "Gemspec/AddRuntimeDependency",
    "Gemspec/AttributeAssignment",
    "Gemspec/DependencyVersion",
    "Gemspec/DeprecatedAttributeAssignment",
    "Gemspec/DevelopmentDependencies",
    "Gemspec/DuplicatedAssignment",
    "Gemspec/OrderedDependencies",
    "Gemspec/RequireMFA",
    "Gemspec/RequiredRubyVersion",
    "Gemspec/RubyVersionGlobalsUsage",
    "Layout/AccessModifierIndentation",
    "Layout/ArgumentAlignment",
    "Layout/ArrayAlignment",
    "Layout/AssignmentIndentation",
    "Layout/BeginEndAlignment",
    "Layout/BlockAlignment",
    "Layout/BlockEndNewline",
    "Layout/CaseIndentation",
    "Layout/ClassStructure",
    "Layout/ClosingHeredocIndentation",
    "Layout/ClosingParenthesisIndentation",
    "Layout/CommentIndentation",
    "Layout/ConditionPosition",
    "Layout/DefEndAlignment",
    "Layout/DotPosition",
    "Layout/ElseAlignment",
    "Layout/EmptyComment",
    "Layout/EmptyLineAfterGuardClause",
    "Layout/EmptyLineAfterMagicComment",
    "Layout/EmptyLineAfterMultilineCondition",
    "Layout/EmptyLineBetweenDefs",
    "Layout/EmptyLinesAfterModuleInclusion",
    "Layout/EmptyLinesAroundAccessModifier",
    "Layout/EmptyLinesAroundArguments",
    "Layout/EmptyLinesAroundAttributeAccessor",
    "Layout/EmptyLinesAroundBeginBody",
    "Layout/EmptyLinesAroundBlockBody",
    "Layout/EmptyLinesAroundClassBody",
    "Layout/EmptyLinesAroundExceptionHandlingKeywords",
    "Layout/EmptyLinesAroundMethodBody",
    "Layout/EmptyLinesAroundModuleBody",
    "Layout/EmptyLines",
    "Layout/EndAlignment",
    "Layout/EndOfLine",
    "Layout/ExtraSpacing",
    "Layout/FirstArgumentIndentation",
    "Layout/FirstArrayElementIndentation",
    "Layout/FirstArrayElementLineBreak",
    "Layout/FirstHashElementIndentation",
    "Layout/FirstHashElementLineBreak",
    "Layout/FirstMethodArgumentLineBreak",
    "Layout/FirstMethodParameterLineBreak",
    "Layout/FirstParameterIndentation",
    "Layout/HashAlignment",
    "Layout/HeredocArgumentClosingParenthesis",
    "Layout/HeredocIndentation",
    "Layout/IndentationConsistency",
    "Layout/IndentationStyle",
    "Layout/IndentationWidth",
    "Layout/InitialIndentation",
    "Layout/LeadingCommentSpace",
    "Layout/LeadingEmptyLines",
    "Layout/LineContinuationLeadingSpace",
    "Layout/LineContinuationSpacing",
    "Layout/LineEndStringConcatenationIndentation",
    "Layout/LineLength",
    "Layout/MultilineArrayBraceLayout",
    "Layout/MultilineArrayLineBreaks",
    "Layout/MultilineAssignmentLayout",
    "Layout/MultilineBlockLayout",
    "Layout/MultilineHashBraceLayout",
    "Layout/MultilineHashKeyLineBreaks",
    "Layout/MultilineMethodArgumentLineBreaks",
    "Layout/MultilineMethodCallBraceLayout",
    "Layout/MultilineMethodCallIndentation",
    "Layout/MultilineMethodDefinitionBraceLayout",
    "Layout/MultilineMethodParameterLineBreaks",
    "Layout/MultilineOperationIndentation",
    "Layout/ParameterAlignment",
    "Layout/RedundantLineBreak",
    "Layout/RescueEnsureAlignment",
    "Layout/SingleLineBlockChain",
    "Layout/SpaceAfterColon",
    "Layout/SpaceAfterComma",
    "Layout/SpaceAfterMethodName",
    "Layout/SpaceAfterNot",
    "Layout/SpaceAfterSemicolon",
    "Layout/SpaceAroundBlockParameters",
    "Layout/SpaceAroundEqualsInParameterDefault",
    "Layout/SpaceAroundKeyword",
    "Layout/SpaceAroundMethodCallOperator",
    "Layout/SpaceAroundOperators",
    "Layout/SpaceBeforeBlockBraces",
    "Layout/SpaceBeforeBrackets",
    "Layout/SpaceBeforeComma",
    "Layout/SpaceBeforeComment",
    "Layout/SpaceBeforeFirstArg",
    "Layout/SpaceBeforeSemicolon",
    "Layout/SpaceInLambdaLiteral",
    "Layout/SpaceInsideArrayPercentLiteral",
    "Layout/SpaceInsideArrayLiteralBrackets",
    "Layout/SpaceInsideBlockBraces",
    "Layout/SpaceInsideHashLiteralBraces",
    "Layout/SpaceInsideParens",
    "Layout/SpaceInsidePercentLiteralDelimiters",
    "Layout/SpaceInsideRangeLiteral",
    "Layout/SpaceInsideReferenceBrackets",
    "Layout/SpaceInsideStringInterpolation",
    "Layout/TrailingEmptyLines",
    "Layout/TrailingWhitespace",
    "Lint/AmbiguousAssignment",
    "Lint/AmbiguousBlockAssociation",
    "Lint/AmbiguousOperator",
    "Lint/AmbiguousOperatorPrecedence",
    "Lint/AmbiguousRange",
    "Lint/AmbiguousRegexpLiteral",
    "Lint/ArrayLiteralInRegexp",
    "Lint/AssignmentInCondition",
    "Lint/BigDecimalNew",
    "Lint/BinaryOperatorWithIdenticalOperands",
    "Lint/BooleanSymbol",
    "Lint/CircularArgumentReference",
    "Lint/ConstantDefinitionInBlock",
    "Lint/ConstantOverwrittenInRescue",
    "Lint/ConstantReassignment",
    "Lint/ConstantResolution",
    "Lint/CopDirectiveSyntax",
    "Lint/DataDefineOverride",
    "Lint/Debugger",
    "Lint/DeprecatedClassMethods",
    "Lint/DeprecatedConstants",
    "Lint/DeprecatedOpenSSLConstant",
    "Lint/DeprecatedReference",
    "Lint/DisjunctiveAssignmentInConstructor",
    "Lint/DuplicateBranch",
    "Lint/DuplicateCaseCondition",
    "Lint/DuplicateElsifCondition",
    "Lint/DuplicateHashKey",
    "Lint/DuplicateMagicComment",
    "Lint/DuplicateMatchPattern",
    "Lint/DuplicateMethods",
    "Lint/DuplicateRegexpCharacterClassElement",
    "Lint/DuplicateRequire",
    "Lint/DuplicateRescueException",
    "Lint/DuplicateSetElement",
    "Lint/EachWithObjectArgument",
    "Lint/ElseLayout",
    "Lint/EmptyBlock",
    "Lint/EmptyClass",
    "Lint/EmptyConditionalBody",
    "Lint/EmptyEnsure",
    "Lint/EmptyExpression",
    "Lint/EmptyFile",
    "Lint/EmptyInPattern",
    "Lint/EmptyInterpolation",
    "Lint/EmptyWhen",
    "Lint/EnsureReturn",
    "Lint/SharedMutableDefault",
    "Lint/ErbNewArguments",
    "Lint/FlipFlop",
    "Lint/FloatComparison",
    "Lint/FloatOutOfRange",
    "Lint/FormatParameterMismatch",
    "Lint/HashCompareByIdentity",
    "Lint/HashNewWithKeywordArgumentsAsDefault",
    "Lint/HeredocMethodCallPosition",
    "Lint/IdentityComparison",
    "Lint/ImplicitStringConcatenation",
    "Lint/IncompatibleIoSelectWithFiberScheduler",
    "Lint/IneffectiveAccessModifier",
    "Lint/InheritException",
    "Lint/InterpolationCheck",
    "Lint/ItWithoutArgumentsInBlock",
    "Lint/LambdaWithoutLiteralBlock",
    "Lint/LiteralAsCondition",
    "Lint/LiteralAssignmentInCondition",
    "Lint/LiteralInInterpolation",
    "Lint/Loop",
    "Lint/MissingCopEnableDirective",
    "Lint/MissingSuper",
    "Lint/MixedCaseRange",
    "Lint/MixedRegexpCaptureTypes",
    "Lint/MultipleComparison",
    "Lint/NameTypo",
    "Lint/NestedMethodDefinition",
    "Lint/NestedPercentLiteral",
    "Lint/NextWithoutAccumulator",
    "Lint/NoReturnInBeginEndBlocks",
    "Lint/NonAtomicFileOperation",
    "Lint/NonDeterministicRequireOrder",
    "Lint/NonLocalExitFromIterator",
    "Lint/NumberConversion",
    "Lint/NumberedParameterAssignment",
    "Lint/NumericOperationWithConstantResult",
    "Lint/OrAssignmentToConstant",
    "Lint/OrderedMagicComments",
    "Lint/OutOfRangeRegexpRef",
    "Lint/ParenthesesAsGroupedExpression",
    "Lint/PercentStringArray",
    "Lint/PercentSymbolArray",
    "Lint/RaiseException",
    "Lint/RandOne",
    "Lint/RedundantCopDisableDirective",
    "Lint/RedundantCopEnableDirective",
    "Lint/RedundantDirGlobSort",
    "Lint/RedundantRegexpQuantifiers",
    "Lint/RedundantRequireStatement",
    "Lint/RedundantSafeNavigation",
    "Lint/RedundantSplatExpansion",
    "Lint/RedundantStringCoercion",
    "Lint/RedundantTypeConversion",
    "Lint/RedundantWithIndex",
    "Lint/RedundantWithObject",
    "Lint/RefinementImportMethods",
    "Lint/RegexpAsCondition",
    "Lint/RequireParentheses",
    "Lint/RequireRangeParentheses",
    "Lint/RequireRelativeSelfPath",
    "Lint/RescueException",
    "Lint/RescueType",
    "Lint/ReturnInVoidContext",
    "Lint/SafeNavigationConsistency",
    "Lint/SafeNavigationChain",
    "Lint/SafeNavigationWithEmpty",
    "Lint/ScriptPermission",
    "Lint/SelfAssignment",
    "Lint/SendWithMixinArgument",
    "Lint/ShadowedArgument",
    "Lint/ShadowedException",
    "Lint/ShadowingOuterLocalVariable",
    "Lint/StructNewOverride",
    "Lint/SuppressedException",
    "Lint/SuppressedExceptionInNumberConversion",
    "Lint/SymbolConversion",
    "Lint/Syntax",
    "Lint/ToEnumArguments",
    "Lint/ToJSON",
    "Lint/TopLevelReturnWithArgument",
    "Lint/TrailingCommaInAttributeDeclaration",
    "Lint/TripleQuotes",
    "Lint/UnderscorePrefixedVariableName",
    "Lint/UnescapedBracketInRegexp",
    "Lint/UnexpectedBlockArity",
    "Lint/UnifiedInteger",
    "Lint/UnmodifiedReduceAccumulator",
    "Lint/UnreachableCode",
    "Lint/UnreachableLoop",
    "Lint/UnreachablePatternBranch",
    "Lint/UnusedBlockArgument",
    "Lint/UnusedMethodArgument",
    "Lint/UnusedPrivateMethod",
    "Lint/UriEscapeUnescape",
    "Lint/UriRegexp",
    "Lint/UselessAccessModifier",
    "Lint/UselessAssignment",
    "Lint/UselessConstantScoping",
    "Lint/UselessDefaultValueArgument",
    "Lint/UselessDefined",
    "Lint/UselessElseWithoutRescue",
    "Lint/UselessMethodDefinition",
    "Lint/UselessNumericOperation",
    "Lint/UselessOr",
    "Lint/UselessRescue",
    "Lint/UselessRuby2Keywords",
    "Lint/UselessSetterCall",
    "Lint/UselessTimes",
    "Lint/Void",
    "Metrics/CyclomaticComplexity",
    "Metrics/AbcSize",
    "Metrics/BlockLength",
    "Metrics/BlockNesting",
    "Metrics/ClassLength",
    "Metrics/CollectionLiteralLength",
    "Metrics/MethodLength",
    "Metrics/ModuleLength",
    "Metrics/ParameterLists",
    "Metrics/PerceivedComplexity",
    "Migration/DepartmentName",
    "Naming/AccessorMethodName",
    "Naming/AsciiIdentifiers",
    "Naming/BlockForwarding",
    "Naming/BlockParameterName",
    "Naming/ClassAndModuleCamelCase",
    "Naming/ConstantName",
    "Naming/FileName",
    "Naming/HeredocDelimiterCase",
    "Naming/HeredocDelimiterNaming",
    "Naming/InclusiveLanguage",
    "Naming/MemoizedInstanceVariableName",
    "Naming/MethodName",
    "Naming/MethodParameterName",
    "Naming/BinaryOperatorParameterName",
    "Naming/PredicateMethod",
    "Naming/PredicatePrefix",
    "Naming/RescuedExceptionsVariableName",
    "Naming/VariableName",
    "Naming/VariableNumber",
    "Security/CompoundHash",
    "Security/Eval",
    "Security/IoMethods",
    "Security/JSONLoad",
    "Security/MarshalLoad",
    "Security/Open",
    "Security/YAMLLoad",
    "Style/AccessModifierDeclarations",
    "Style/AccessorGrouping",
    "Style/Alias",
    "Style/AmbiguousEndlessMethodDefinition",
    "Style/AndOr",
    "Style/ArgumentsForwarding",
    "Style/ArrayCoercion",
    "Style/ArrayFirstLast",
    "Style/ArrayIntersect",
    "Style/ArrayIntersectWithSingleElement",
    "Style/ArrayJoin",
    "Style/AsciiComments",
    "Style/Attr",
    "Style/AutoResourceCleanup",
    "Style/BarePercentLiterals",
    "Style/BeginBlock",
    "Style/BisectedAttrAccessor",
    "Style/BitwisePredicate",
    "Style/BlockComments",
    "Style/BlockDelimiters",
    "Style/CaseEquality",
    "Style/CaseLikeIf",
    "Style/CharacterLiteral",
    "Style/ClassAndModuleChildren",
    "Style/ClassCheck",
    "Style/ClassEqualityComparison",
    "Style/ClassMethods",
    "Style/ClassMethodsDefinitions",
    "Style/ClassVars",
    "Style/CollectionCompact",
    "Style/CollectionMethods",
    "Style/CollectionQuerying",
    "Style/ColonMethodCall",
    "Style/ColonMethodDefinition",
    "Style/CombinableDefined",
    "Style/CombinableLoops",
    "Style/CommandLiteral",
    "Style/CommentAnnotation",
    "Style/CommentedKeyword",
    "Style/ComparableBetween",
    "Style/ComparableClamp",
    "Style/ConcatArrayLiterals",
    "Style/ConditionalAssignment",
    "Style/ConstantVisibility",
    "Style/Copyright",
    "Style/DataInheritance",
    "Style/DateTime",
    "Style/DefWithParentheses",
    "Style/DigChain",
    "Style/Dir",
    "Style/DirEmpty",
    "Style/DisableCopsWithinSourceCodeDirective",
    "Style/DocumentationMethod",
    "Style/Documentation",
    "Style/DocumentDynamicEvalDefinition",
    "Style/DoubleCopDisableDirective",
    "Style/DoubleNegation",
    "Style/EachForSimpleLoop",
    "Style/EachWithObject",
    "Style/EmptyBlockParameter",
    "Style/EmptyCaseCondition",
    "Style/EmptyClassDefinition",
    "Style/EmptyElse",
    "Style/EmptyHeredoc",
    "Style/EmptyLambdaParameter",
    "Style/EmptyLiteral",
    "Style/EmptyMethod",
    "Style/EmptyStringInsideInterpolation",
    "Style/EndlessMethod",
    "Style/Encoding",
    "Style/EndBlock",
    "Style/EnvHome",
    "Style/EvalWithLocation",
    "Style/EvenOdd",
    "Style/ExactRegexpMatch",
    "Style/ExpandPathArguments",
    "Style/ExplicitBlockArgument",
    "Style/ExponentialNotation",
    "Style/FetchEnvVar",
    "Style/FileEmpty",
    "Style/FileNull",
    "Style/FileOpen",
    "Style/FileRead",
    "Style/FileTouch",
    "Style/FileWrite",
    "Style/FloatDivision",
    "Style/For",
    "Style/FormatString",
    "Style/FormatStringToken",
    "Style/FrozenStringLiteralComment",
    "Style/GlobalStdStream",
    "Style/GlobalVars",
    "Style/GuardClause",
    "Style/HashAsLastArrayItem",
    "Style/HashConversion",
    "Style/HashEachMethods",
    "Style/HashExcept",
    "Style/HashFetchChain",
    "Style/HashLikeCase",
    "Style/HashLookupMethod",
    "Style/HashSlice",
    "Style/HashSyntax",
    "Style/HashTransformKeys",
    "Style/HashTransformValues",
    "Style/IdenticalConditionalBranches",
    "Style/IfInsideElse",
    "Style/IfUnlessModifier",
    "Style/IfUnlessModifierOfIfUnless",
    "Style/IfWithBooleanLiteralBranches",
    "Style/IfWithSemicolon",
    "Style/ImplicitRuntimeError",
    "Style/InPatternThen",
    "Style/InfiniteLoop",
    "Style/ReduceToHash",
    "Style/InverseMethods",
    "Style/InlineComment",
    "Style/InvertibleUnlessCondition",
    "Style/IpAddresses",
    "Style/ItAssignment",
    "Style/ItBlockParameter",
    "Style/KeywordArgumentsMerging",
    "Style/KeywordParametersOrder",
    "Style/Lambda",
    "Style/LambdaCall",
    "Style/LineEndConcatenation",
    "Style/MagicCommentFormat",
    "Style/MapIntoArray",
    "Style/MapJoin",
    "Style/MapToHash",
    "Style/MapToSet",
    "Style/MethodCallWithoutArgsParentheses",
    "Style/MethodCallWithArgsParentheses",
    "Style/MinMaxComparison",
    "Style/ModuleMemberExistenceCheck",
    "Style/MultilineInPatternThen",
    "Style/NumberedParameters",
    "Style/OneClassPerFile",
    "Style/OpenStructUse",
    "Style/OperatorMethodCall",
    "Style/PartitionInsteadOfDoubleSelect",
    "Style/RedundantArrayConstructor",
    "Style/RedundantArrayFlatten",
    "Style/RedundantAssignment",
    "Style/RedundantConstantBase",
    "Style/RedundantCurrentDirectoryInPath",
    "Style/RedundantDoubleSplatHashBraces",
    "Style/RedundantEach",
    "Style/RedundantFetchBlock",
    "Style/RedundantFileExtensionInRequire",
    "Style/RedundantFilterChain",
    "Style/RedundantFormat",
    "Style/RedundantHeredocDelimiterQuotes",
    "Style/RedundantInitialize",
    "Style/RedundantInterpolationUnfreeze",
    "Style/RedundantLineContinuation",
    "Style/RedundantMinMaxBy",
    "Style/RedundantRegexpArgument",
    "Style/RedundantRegexpConstructor",
    "Style/RedundantSelfAssignment",
    "Style/RedundantSelfAssignmentBranch",
    "Style/RedundantStructKeywordInit",
    "Style/RequireOrder",
    "Style/ReverseFind",
    "Style/SafeNavigationChainLength",
    "Style/SingleLineDoEndBlock",
    "Style/SoleNestedConditional",
    "Style/StaticClass",
    "Style/MapCompactWithConditionalBlock",
    "Style/MethodCalledOnDoEndBlock",
    "Style/MethodDefParentheses",
    "Style/MinMax",
    "Style/MissingElse",
    "Style/MissingRespondToMissing",
    "Style/MixinGrouping",
    "Style/MixinUsage",
    "Style/ModuleFunction",
    "Style/MultilineBlockChain",
    "Style/MultilineIfThen",
    "Style/MultilineIfModifier",
    "Style/MultilineMethodSignature",
    "Style/MultilineMemoization",
    "Style/MultilineTernaryOperator",
    "Style/MultilineWhenThen",
    "Style/MultipleComparison",
    "Style/MutableConstant",
    "Style/NegatedIf",
    "Style/NegatedIfElseCondition",
    "Style/NegatedUnless",
    "Style/NegatedWhile",
    "Style/NegativeArrayIndex",
    "Style/NestedFileDirname",
    "Style/NestedModifier",
    "Style/NestedParenthesizedCalls",
    "Style/NestedTernaryOperator",
    "Style/Next",
    "Style/NilComparison",
    "Style/NilLambda",
    "Style/NonNilCheck",
    "Style/Not",
    "Style/NumberedParametersLimit",
    "Style/NumericLiterals",
    "Style/NumericLiteralPrefix",
    "Style/NumericPredicate",
    "Style/ObjectThen",
    "Style/OneLineConditional",
    "Style/OrAssignment",
    "Style/OptionHash",
    "Style/OptionalArguments",
    "Style/OptionalBooleanParameter",
    "Style/ParallelAssignment",
    "Style/ParenthesesAroundCondition",
    "Style/PercentLiteralDelimiters",
    "Style/PercentQLiterals",
    "Style/PerlBackrefs",
    "Style/PredicateWithKind",
    "Style/PreferredHashMethods",
    "Style/Proc",
    "Style/QuotedSymbols",
    "Style/RaiseArgs",
    "Style/RandomWithOffset",
    "Style/RedundantArgument",
    "Style/RedundantBegin",
    "Style/RedundantCapitalW",
    "Style/RedundantCondition",
    "Style/RedundantConditional",
    "Style/RedundantException",
    "Style/RedundantFreeze",
    "Style/RedundantInterpolation",
    "Style/RedundantParentheses",
    "Style/RedundantPercentQ",
    "Style/RedundantRegexpCharacterClass",
    "Style/RedundantRegexpEscape",
    "Style/RedundantReturn",
    "Style/RedundantSelf",
    "Style/RedundantSort",
    "Style/RedundantSortBy",
    "Style/RedundantStringEscape",
    "Style/RegexpLiteral",
    "Style/RescueModifier",
    "Style/RescueStandardError",
    "Style/ReturnNil",
    "Style/ReturnNilInPredicateMethodDefinition",
    "Style/SafeNavigation",
    "Style/Sample",
    "Style/SelectByKind",
    "Style/SelectByRange",
    "Style/SelectByRegexp",
    "Style/SelfAssignment",
    "Style/Semicolon",
    "Style/Send",
    "Style/SendWithLiteralMethodName",
    "Style/SignalException",
    "Style/SingleArgumentDig",
    "Style/SingleLineBlockParams",
    "Style/SingleLineMethods",
    "Style/SlicingWithRange",
    "Style/SpecialGlobalVars",
    "Style/StabbyLambdaParentheses",
    "Style/StderrPuts",
    "Style/StringChars",
    "Style/StringConcatenation",
    "Style/StringHashKeys",
    "Style/StringLiterals",
    "Style/StringLiteralsInInterpolation",
    "Style/StringMethods",
    "Style/Strip",
    "Style/StructInheritance",
    "Style/SuperArguments",
    "Style/SuperWithArgsParentheses",
    "Style/SwapValues",
    "Style/SymbolArray",
    "Style/SymbolLiteral",
    "Style/SymbolProc",
    "Style/TallyMethod",
    "Style/TernaryParentheses",
    "Style/TopLevelMethodDefinition",
    "Style/TrailingBodyOnClass",
    "Style/TrailingBodyOnMethodDefinition",
    "Style/TrailingBodyOnModule",
    "Style/TrailingCommaInArguments",
    "Style/TrailingCommaInArrayLiteral",
    "Style/TrailingCommaInBlockArgs",
    "Style/TrailingCommaInHashLiteral",
    "Style/TrailingMethodEndStatement",
    "Style/TrailingUnderscoreVariable",
    "Style/TrivialAccessors",
    "Style/UnlessElse",
    "Style/UnlessLogicalOperators",
    "Style/UnpackFirst",
    "Style/VariableInterpolation",
    "Style/WhenThen",
    "Style/WhileUntilDo",
    "Style/WhileUntilModifier",
    "Style/WordArray",
    "Style/YAMLFileRead",
    "Style/YodaCondition",
    "Style/YodaExpression",
    "Style/ZeroLengthPredicate",
];

/// Everything RuboCop's HTML report holds before the first value that depends on the run:
/// `assets/output.html.erb` down to the `src` of the logo, with `assets/output.css.erb` rendered
/// into the `<style>` block. None of it varies -- the stylesheet interpolates only the fixed
/// severity colours -- so it is carried verbatim rather than re-derived.
const HTML_HEAD: &str = r#"<!DOCTYPE html>
<html>
  <head>
    <meta charset='UTF-8' />
    <title>RuboCop Inspection Report</title>
    <style>
      * {
        -webkit-box-sizing: border-box;
        -moz-box-sizing: border-box;
        box-sizing: border-box;
      }

      body, html {
        font-size: 62.5%;
      }
      body {
        background-color: #ecedf0;
        font-family: "Helvetica Neue",Helvetica,Arial,sans-serif;
        margin: 0;
      }
      code {
        font-family: Consolas, "Liberation Mono", Menlo, Courier, monospace;
        font-size: 85%;
      }
      #header {
        background: #f9f9f9;
        color: #333;
        border-bottom: 3px solid #ccc;
        height: 50px;
        padding: 0;
      }
      #header .logo {
        float: left;
        margin: 5px 12px 7px 20px;
        width: 38px;
        height: 38px;
      }
      #header .title {
        display: inline-block;
        float: left;
        height: 50px;
        font-size: 2.4rem;
        letter-spacing: normal;
        line-height: 50px;
        margin: 0;
      }

      .information, #offenses {
        width: 100%;
        padding: 20px;
        color: #333;
      }
      #offenses {
        padding: 0 20px;
      }

      .information .infobox {
        border-left: 3px solid;
        border-radius: 4px;
        background-color: #fff;
        -webkit-box-shadow: 0 1px 1px rgba(0, 0, 0, 0.05);
        box-shadow: 0 1px 1px rgba(0, 0, 0, 0.05);
        padding: 15px;
        border-color: #0088cc;
        font-size: 1.4rem;
      }
      .information .infobox .info-title {
        font-size: 1.8rem;
        line-height: 2.2rem;
        margin: 0 0 0.5em;
      }
      .information .offenses-list li {
        line-height: 1.8rem
      }
      .information .offenses-list {
        padding-left: 20px;
        margin-bottom: 0;
      }

      #offenses .offense-box {
        border-radius: 4px;
        margin-bottom: 20px;
        background-color: #fff;
        -webkit-box-shadow: 0 1px 1px rgba(0, 0, 0, 0.05);
        box-shadow: 0 1px 1px rgba(0, 0, 0, 0.05);
      }
      .fixed .box-title {
        position: fixed;
        top: 0;
        z-index: 10;
        width: 100%;
      }
      .box-title-placeholder {
        display: none;
      }
      .fixed .box-title-placeholder {
        display: block;
      }
      #offenses .offense-box .box-title h3, #offenses .offense-box .box-title-placeholder h3 {
        color: #33353f;
        background-color: #f6f6f6;
        font-size: 2rem;
        line-height: 2rem;
        display: block;
        padding: 15px;
        border-radius: 5px;
        margin: 0;
      }
      #offenses .offense-box .offense-reports  {
        padding: 0 15px;
      }
      #offenses .offense-box .offense-reports .report {
        border-bottom: 1px dotted #ddd;
        padding: 15px 0px;
        position: relative;
        font-size: 1.3rem;
      }
      #offenses .offense-box .offense-reports .report:last-child {
        border-bottom: none;
      }
      #offenses .offense-box .offense-reports .report pre code {
        display: block;
        background: #000;
        color: #fff;
        padding: 10px 15px;
        border-radius: 5px;
        line-height: 1.6rem;
      }
      #offenses .offense-box .offense-reports .report .location {
        font-weight: bold;
      }
      #offenses .offense-box .offense-reports .report .message code {
        padding: 0.3em;
        background-color: rgba(0,0,0,0.07);
        border-radius: 3px;
      }
      .severity {
        text-transform: capitalize;
        font-weight: bold;
      }
      .highlight {
        padding: 2px;
        border-radius: 2px;
        font-weight: bold;
      }

      .severity.refactor {
        color: rgba(237, 156, 40, 1.0);
      }
      .highlight.refactor {
        background-color: rgba(237, 156, 40, 0.6);
        border: 1px solid rgba(237, 156, 40, 0.4);
      }

      .severity.convention {
        color: rgba(237, 156, 40, 1.0);
      }
      .highlight.convention {
        background-color: rgba(237, 156, 40, 0.6);
        border: 1px solid rgba(237, 156, 40, 0.4);
      }

      .severity.warning {
        color: rgba(150, 40, 239, 1.0);
      }
      .highlight.warning {
        background-color: rgba(150, 40, 239, 0.6);
        border: 1px solid rgba(150, 40, 239, 0.4);
      }

      .severity.error {
        color: rgba(210, 50, 45, 1.0);
      }
      .highlight.error {
        background-color: rgba(210, 50, 45, 0.6);
        border: 1px solid rgba(210, 50, 45, 0.4);
      }

      .severity.fatal {
        color: rgba(210, 50, 45, 1.0);
      }
      .highlight.fatal {
        background-color: rgba(210, 50, 45, 0.6);
        border: 1px solid rgba(210, 50, 45, 0.4);
      }

      footer {
        margin-bottom: 20px;
        margin-right: 20px;
        font-size: 1.3rem;
        color: #777;
        text-align: right;
      }
      .extra-code {
        color: #ED9C28
      }


    </style>
    <script>
    (function() {
      // floating headers. requires classList support.
      if (!('classList' in document.createElement("_"))) return;

      var loaded = false,
        boxes,
        boxPositions;

      window.onload = function() {
        var scrollY = window.scrollY;
        boxes = document.querySelectorAll('.offense-box');
        boxPositions = [];
        for (var i = 0; i < boxes.length; i++)
          // need to add scrollY because the page might be somewhere other than the top when loaded.
          boxPositions[i] = boxes[i].getBoundingClientRect().top + scrollY;
        loaded = true;
      };

      window.onscroll = function() {
        if (!loaded) return;
        var i,
          idx,
          scrollY = window.scrollY;
        for (i = 0; i < boxPositions.length; i++) {
          if (scrollY <= boxPositions[i] - 1) {
            idx = i;
            break;
          }
        }
        if (typeof idx == 'undefined') idx = boxes.length;
        if (idx > 0)
          boxes[idx - 1].classList.add('fixed');
        for (i = 0; i < boxes.length; i++) {
          if (i < idx) continue;
          boxes[i].classList.remove('fixed');
        }
      };
    })();
    </script>
  </head>
  <body>
    <div id="header">
      <img class="logo" src="data:image/png;base64,"#;

/// `assets/logo.png` as `base64_encoded_logo_image` emits it: `[image].pack('m')`, which is
/// Base64 broken into 60-character lines. It sits inside the `src` attribute of the header image,
/// newlines and all.
const HTML_LOGO: &str = r#"iVBORw0KGgoAAAANSUhEUgAAAEwAAABMCAYAAADHl1ErAAAKQWlDQ1BJQ0Mg
UHJvZmlsZQAASA2dlndUU9kWh8+9N73QEiIgJfQaegkg0jtIFQRRiUmAUAKG
hCZ2RAVGFBEpVmRUwAFHhyJjRRQLg4Ji1wnyEFDGwVFEReXdjGsJ7601896a
/cdZ39nnt9fZZ+9917oAUPyCBMJ0WAGANKFYFO7rwVwSE8vE9wIYEAEOWAHA
4WZmBEf4RALU/L09mZmoSMaz9u4ugGS72yy/UCZz1v9/kSI3QyQGAApF1TY8
fiYX5QKUU7PFGTL/BMr0lSkyhjEyFqEJoqwi48SvbPan5iu7yZiXJuShGlnO
Gbw0noy7UN6aJeGjjAShXJgl4GejfAdlvVRJmgDl9yjT0/icTAAwFJlfzOcm
oWyJMkUUGe6J8gIACJTEObxyDov5OWieAHimZ+SKBIlJYqYR15hp5ejIZvrx
s1P5YjErlMNN4Yh4TM/0tAyOMBeAr2+WRQElWW2ZaJHtrRzt7VnW5mj5v9nf
Hn5T/T3IevtV8Sbsz55BjJ5Z32zsrC+9FgD2JFqbHbO+lVUAtG0GQOXhrE/v
IADyBQC03pzzHoZsXpLE4gwnC4vs7GxzAZ9rLivoN/ufgm/Kv4Y595nL7vtW
O6YXP4EjSRUzZUXlpqemS0TMzAwOl89k/fcQ/+PAOWnNycMsnJ/AF/GF6FVR
6JQJhIlou4U8gViQLmQKhH/V4X8YNicHGX6daxRodV8AfYU5ULhJB8hvPQBD
IwMkbj96An3rWxAxCsi+vGitka9zjzJ6/uf6Hwtcim7hTEEiU+b2DI9kciWi
LBmj34RswQISkAd0oAo0gS4wAixgDRyAM3AD3iAAhIBIEAOWAy5IAmlABLJB
PtgACkEx2AF2g2pwANSBetAEToI2cAZcBFfADXALDIBHQAqGwUswAd6BaQiC
8BAVokGqkBakD5lC1hAbWgh5Q0FQOBQDxUOJkBCSQPnQJqgYKoOqoUNQPfQj
dBq6CF2D+qAH0CA0Bv0BfYQRmALTYQ3YALaA2bA7HAhHwsvgRHgVnAcXwNvh
SrgWPg63whfhG/AALIVfwpMIQMgIA9FGWAgb8URCkFgkAREha5EipAKpRZqQ
DqQbuY1IkXHkAwaHoWGYGBbGGeOHWYzhYlZh1mJKMNWYY5hWTBfmNmYQM4H5
gqVi1bGmWCesP3YJNhGbjS3EVmCPYFuwl7ED2GHsOxwOx8AZ4hxwfrgYXDJu
Na4Etw/XjLuA68MN4SbxeLwq3hTvgg/Bc/BifCG+Cn8cfx7fjx/GvyeQCVoE
a4IPIZYgJGwkVBAaCOcI/YQRwjRRgahPdCKGEHnEXGIpsY7YQbxJHCZOkxRJ
hiQXUiQpmbSBVElqIl0mPSa9IZPJOmRHchhZQF5PriSfIF8lD5I/UJQoJhRP
ShxFQtlOOUq5QHlAeUOlUg2obtRYqpi6nVpPvUR9Sn0vR5Mzl/OX48mtk6uR
a5Xrl3slT5TXl3eXXy6fJ18hf0r+pvy4AlHBQMFTgaOwVqFG4bTCPYVJRZqi
lWKIYppiiWKD4jXFUSW8koGStxJPqUDpsNIlpSEaQtOledK4tE20Otpl2jAd
Rzek+9OT6cX0H+i99AllJWVb5SjlHOUa5bPKUgbCMGD4M1IZpYyTjLuMj/M0
5rnP48/bNq9pXv+8KZX5Km4qfJUilWaVAZWPqkxVb9UU1Z2qbapP1DBqJmph
atlq+9Uuq43Pp893ns+dXzT/5PyH6rC6iXq4+mr1w+o96pMamhq+GhkaVRqX
NMY1GZpumsma5ZrnNMe0aFoLtQRa5VrntV4wlZnuzFRmJbOLOaGtru2nLdE+
pN2rPa1jqLNYZ6NOs84TXZIuWzdBt1y3U3dCT0svWC9fr1HvoT5Rn62fpL9H
v1t/ysDQINpgi0GbwaihiqG/YZ5ho+FjI6qRq9Eqo1qjO8Y4Y7ZxivE+41sm
sImdSZJJjclNU9jU3lRgus+0zwxr5mgmNKs1u8eisNxZWaxG1qA5wzzIfKN5
m/krCz2LWIudFt0WXyztLFMt6ywfWSlZBVhttOqw+sPaxJprXWN9x4Zq42Oz
zqbd5rWtqS3fdr/tfTuaXbDdFrtOu8/2DvYi+yb7MQc9h3iHvQ732HR2KLuE
fdUR6+jhuM7xjOMHJ3snsdNJp9+dWc4pzg3OowsMF/AX1C0YctFx4bgccpEu
ZC6MX3hwodRV25XjWuv6zE3Xjed2xG3E3dg92f24+ysPSw+RR4vHlKeT5xrP
C16Il69XkVevt5L3Yu9q76c+Oj6JPo0+E752vqt9L/hh/QL9dvrd89fw5/rX
+08EOASsCegKpARGBFYHPgsyCRIFdQTDwQHBu4IfL9JfJFzUFgJC/EN2hTwJ
NQxdFfpzGC4sNKwm7Hm4VXh+eHcELWJFREPEu0iPyNLIR4uNFksWd0bJR8VF
1UdNRXtFl0VLl1gsWbPkRoxajCCmPRYfGxV7JHZyqffS3UuH4+ziCuPuLjNc
lrPs2nK15anLz66QX8FZcSoeGx8d3xD/iRPCqeVMrvRfuXflBNeTu4f7kufG
K+eN8V34ZfyRBJeEsoTRRJfEXYljSa5JFUnjAk9BteB1sl/ygeSplJCUoykz
qdGpzWmEtPi000IlYYqwK10zPSe9L8M0ozBDuspp1e5VE6JA0ZFMKHNZZruY
jv5M9UiMJJslg1kLs2qy3mdHZZ/KUcwR5vTkmuRuyx3J88n7fjVmNXd1Z752
/ob8wTXuaw6thdauXNu5Tnddwbrh9b7rj20gbUjZ8MtGy41lG99uit7UUaBR
sL5gaLPv5sZCuUJR4b0tzlsObMVsFWzt3WazrWrblyJe0fViy+KK4k8l3JLr
31l9V/ndzPaE7b2l9qX7d+B2CHfc3em681iZYlle2dCu4F2t5czyovK3u1fs
vlZhW3FgD2mPZI+0MqiyvUqvakfVp+qk6oEaj5rmvep7t+2d2sfb17/fbX/T
AY0DxQc+HhQcvH/I91BrrUFtxWHc4azDz+ui6rq/Z39ff0TtSPGRz0eFR6XH
wo911TvU1zeoN5Q2wo2SxrHjccdv/eD1Q3sTq+lQM6O5+AQ4ITnx4sf4H++e
DDzZeYp9qukn/Z/2ttBailqh1tzWibakNml7THvf6YDTnR3OHS0/m/989Iz2
mZqzymdLz5HOFZybOZ93fvJCxoXxi4kXhzpXdD66tOTSna6wrt7LgZevXvG5
cqnbvfv8VZerZ645XTt9nX297Yb9jdYeu56WX+x+aem172296XCz/ZbjrY6+
BX3n+l37L972un3ljv+dGwOLBvruLr57/17cPel93v3RB6kPXj/Mejj9aP1j
7OOiJwpPKp6qP6391fjXZqm99Oyg12DPs4hnj4a4Qy//lfmvT8MFz6nPK0a0
RupHrUfPjPmM3Xqx9MXwy4yX0+OFvyn+tveV0auffnf7vWdiycTwa9HrmT9K
3qi+OfrW9m3nZOjk03dp76anit6rvj/2gf2h+2P0x5Hp7E/4T5WfjT93fAn8
8ngmbWbm3/eE8/syOll+AAAACXBIWXMAAAsTAAALEwEAmpwYAAAEJGlUWHRY
TUw6Y29tLmFkb2JlLnhtcAAAAAAAPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9i
ZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNS40LjAiPgogICA8cmRm
OlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjIt
cmRmLXN5bnRheC1ucyMiPgogICAgICA8cmRmOkRlc2NyaXB0aW9uIHJkZjph
Ym91dD0iIgogICAgICAgICAgICB4bWxuczp0aWZmPSJodHRwOi8vbnMuYWRv
YmUuY29tL3RpZmYvMS4wLyIKICAgICAgICAgICAgeG1sbnM6ZXhpZj0iaHR0
cDovL25zLmFkb2JlLmNvbS9leGlmLzEuMC8iCiAgICAgICAgICAgIHhtbG5z
OmRjPSJodHRwOi8vcHVybC5vcmcvZGMvZWxlbWVudHMvMS4xLyIKICAgICAg
ICAgICAgeG1sbnM6eG1wPSJodHRwOi8vbnMuYWRvYmUuY29tL3hhcC8xLjAv
Ij4KICAgICAgICAgPHRpZmY6UmVzb2x1dGlvblVuaXQ+MTwvdGlmZjpSZXNv
bHV0aW9uVW5pdD4KICAgICAgICAgPHRpZmY6Q29tcHJlc3Npb24+NTwvdGlm
ZjpDb21wcmVzc2lvbj4KICAgICAgICAgPHRpZmY6WFJlc29sdXRpb24+NzI8
L3RpZmY6WFJlc29sdXRpb24+CiAgICAgICAgIDx0aWZmOk9yaWVudGF0aW9u
PjE8L3RpZmY6T3JpZW50YXRpb24+CiAgICAgICAgIDx0aWZmOllSZXNvbHV0
aW9uPjcyPC90aWZmOllSZXNvbHV0aW9uPgogICAgICAgICA8ZXhpZjpQaXhl
bFhEaW1lbnNpb24+NzY8L2V4aWY6UGl4ZWxYRGltZW5zaW9uPgogICAgICAg
ICA8ZXhpZjpDb2xvclNwYWNlPjE8L2V4aWY6Q29sb3JTcGFjZT4KICAgICAg
ICAgPGV4aWY6UGl4ZWxZRGltZW5zaW9uPjc2PC9leGlmOlBpeGVsWURpbWVu
c2lvbj4KICAgICAgICAgPGRjOnN1YmplY3Q+CiAgICAgICAgICAgIDxyZGY6
U2VxLz4KICAgICAgICAgPC9kYzpzdWJqZWN0PgogICAgICAgICA8eG1wOk1v
ZGlmeURhdGU+MjAxNDowOToyMyAyMjowOToxNDwveG1wOk1vZGlmeURhdGU+
CiAgICAgICAgIDx4bXA6Q3JlYXRvclRvb2w+UGl4ZWxtYXRvciAzLjIuMTwv
eG1wOkNyZWF0b3JUb29sPgogICAgICA8L3JkZjpEZXNjcmlwdGlvbj4KICAg
PC9yZGY6UkRGPgo8L3g6eG1wbWV0YT4KkpvroQAABkNJREFUeAHtm0uIHFUY
hWveM8ZpHN/RYCIS34mKGGSMMmLA6CKILmQwBOJCEcwqKzELceVWN+IyIgQM
4gt84SMqBpNBkBiJGkEFMRFNMmSSzEzm5fmG7qEpbt1bt7q6XuOBQ3Xfuvf/
z/nr3qrq6u6OIH9cLAlrxGvr20e0vVsE34rviL+Lv9W3J7XNDR05Zb5deR8U
7xfXiVeKnaIN89p5XPxB/EL8WPxerCxqcrZd3CdOiwstkhj7RGISuzLol5On
xSNiq0WKGk9scpCr1LhP6veLUUbTbicXOUuHHil+QZwS0y6KKx45yY2GUuAy
qXxXdBlr9340oKXQWC11Y2K7ixE3PlrQVEhcI1WHxLhmsuqHJrQVCtx8cpOZ
VRF886ANjYVAl1TsFX1NZN0fjWjNHTulIGvzSfOhNVfcpuwTYlIDWY9DK5pz
AdP7EzFr063mQ3MuS/PREharUWy0Zwruog+IDQFl26I9008Cm0tcrMbBxYM3
XM+gogI+GbWjRO2ZebhKRTklNo5UWbd4wIsXksywEWW4yCtLMTvjYcRXWpKC
bfJNUuD+3l58C9Yt83cWuAC+0vCCp7aBT/3nxbKet8K68eL1JMO3unzLtFtk
WwVQwKp4Kebx8KnurbLwjLhBHCimHW9VkxpxUHxVPOw92jJgi/bxjXP4HFCV
93jDYypYrSj/iFUpTpQPPK4RrYhzW7FVES61RqnGTjw+4bISp2B3uIJUaL/T
a5yC9VaoIC4rTq9xCjbrylKh/U6vroLdpGKsr1BBXFbwiudE4NP8j2LUVaWq
7XiOfBpjm2HbNPBmcbkBz3g3wnanf1ojBo2j6o1DnZ3B6IpasL6nb/Hng0y5
IgKT8+Khmelgz9nTwal53lnB13E1Uw9bwaz+r+jqCvZcsjIY7hsIZhdXrSl8
sdq69Tl7//RkMHriWPD33JxLnLE2vk8rlpLsGBwKhvsHggn30Voak/+LhUXN
aN81/m8iObZzWGRABg339QfTC9ZJGDk+zx1oRnsi4xKeaBxlmlJi45zNsxox
cqMZ7UkPdeKCvX3uTNCrkpWpaGhFM9qTFizxOWy3rjZru3uD7RfWgoGOcpRt
UjPrlYnxAO1JYXN6QEF5WGgFtxQ39vQGXbZI1gjZ7JzTlPpp5vzirUWMjGPq
4/QejjOiBmbuciTevcH57TWRu7zlUjS84jnRuV3jFr+z+1Lb5VKwr+qe8W6E
q5I87jhpHFnNxhOyZX3E4yoYZcnl13o5HQ+n1zgFS34Nzsl1C2mdXuMU7PMW
BJRtaCpeecTDfUnVT/x4tD7O8jn616nz1xUuGt7w6ITP/Xmfoj0s8n/sFSIz
Low4Szw8Jov33F+FgfezIn+r+UDkH77/I+0K+MywRm7+0P6hGF7vM2rbIh4V
i4S1EvOeGP6ZOY+hHxKPi7GR5GkFYxDBsmwGS5RlWzSg6QYxPDlYjt7+vQfU
q2G6G6bNdF6rD1naPKZXHNk0wEx/yxEITWgLzzCTB0eoBBV2RrR3eEC7Xxcv
sHeLvXdUPcfFz2KPaLFjllc1foD7hphWsbBOLGISOxMkXZJJxLEMuWAALuEY
PcebBKBQW0XOT8Qk9ndiIbFKqlgGnBuayS+SbxGj0KUdz4sUaUdUJ492YhCL
mMSOAppMv/zGA17ajqQFawhj+YSvWI19PltixFmKqRYsyyXZKEZaS4fZnVas
hjbnNo+ChUVtVMP1omvWUaCfxW/EUuFqqTX9m21W7es8nTyl/oxrPhfaXtOX
MT5AkykHHvDSdgwpw1+iyRgfjeLiHnU8I5ri2NoYw9i4QJMpHh7w0nZw7zYm
mkS8GTP7SvX7JSKGKW64jbHEiIO96hQez3s8ZHYf+nKECKb+NtEGzpvviyYT
Pm3EcJ2D0WJajuTBQ2bYqEz8wMpkcErtu8TLxTC4X3pJNI1L0kYs0z0YudGA
FlNctOPBG64rU1RARH4kborqoPZj4mGx8cUCuVaJG8Q0cVDB/hQpDKiJ/C/K
tmQ/1f7NovNXdeqTGu5SJB6RmI5gkdvQjPZc8KyyFrk4Jm1ozhXPKTvPzE3i
itSGRrQWAo9LxR9ikQrUrAVtaCwUOMm+KB4Vm8Xm+RotaLJdALQ7Xwwq/b3i
TvFXMeuCkZPcaEBLqvgPBhCuiZo8+sAAAAAASUVORK5CYII=
"#;

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
