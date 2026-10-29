use super::*;

/// FormatterSet の各 callback を独立した出力境界として扱う。
/// reports の prefix を毎回renderせず、file本文と最後の一覧をそれぞれ一度だけ走査する。
pub(crate) struct Lifecycle<'a> {
    format: Format,
    options: FormatOptions<'a>,
    bar: String,
    bar_length: usize,
    severest: Option<Severity>,
}

impl<'a> Lifecycle<'a> {
    pub(crate) fn new(format: Format, options: &FormatOptions<'a>) -> Self {
        Self {
            format,
            options: options.clone(),
            bar: String::new(),
            bar_length: 0,
            severest: None,
        }
    }

    pub(crate) fn started(&mut self, output: &mut impl Output) -> Result<()> {
        match self.format {
            Format::Progress | Format::Pacman | Format::Autogenconf => output.puts(&format!(
                "Inspecting {}",
                plural(self.options.target_file_count, "file")
            )),
            Format::Tap => output.puts(&format!("1..{}", self.options.target_file_count)),
            Format::Fuubar => {
                // bar幅による確保失敗も、他formatterのstartedとの実順序に置く。
                let width = fuubar_bar_width();
                self.bar.try_reserve_exact(width.saturating_add(12))?;
                self.bar.push_str("Progress: |");
                self.bar.extend(std::iter::repeat_n('=', width));
                self.bar.push('|');
                if self.options.color {
                    output.write("\x1b[32m")?;
                }
                output.write("\n")?;
                self.increment_bar(output, 0)?;
                if self.options.color {
                    output.write("\x1b[0m")?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn file_started(&mut self, _index: usize, _output: &mut impl Output) -> Result<()> {
        // 現在の Pacman は非TTYで上流IO#winsizeを実行できず、既存Progress互換を保つ。
        // callback自体は省かず、正常なfile_startedを持つformatterの追加先を固定する。
        Ok(())
    }

    pub(crate) fn file_finished(
        &mut self,
        index: usize,
        report: &FileReport,
        output: &mut impl Output,
    ) -> Result<()> {
        match self.format {
            Format::Simple | Format::Quiet => emit_simple_file(output, report, &self.options),
            Format::Clang => emit_clang_file(output, report, &self.options, false),
            Format::Tap => emit_tap_file(output, report, index, &self.options),
            Format::Emacs => emit_emacs_file(output, report, &self.options),
            Format::Files if !report.offenses.is_empty() => output.write(&format!(
                "{}\n",
                absolute_path(&report.path, self.options.cwd)
            )),
            Format::Progress | Format::Pacman | Format::Autogenconf => {
                let severity = report.offenses.iter().map(|offense| offense.severity).max();
                let mark = severity.map_or('.', Severity::code).to_string();
                if self.format == Format::Autogenconf {
                    let color = match severity {
                        Some(Severity::Info) => "38;5;188",
                        Some(severity) => severity_color(severity),
                        None => "32",
                    };
                    output.write(&paint(&mark, color, self.options.color))
                } else {
                    output.write(&mark)
                }
            }
            Format::Fuubar => {
                if let Some(severity) = report
                    .offenses
                    .iter()
                    .filter(|offense| !offense.corrected)
                    .map(|offense| offense.severity)
                    .max()
                {
                    self.severest = Some(self.severest.map_or(severity, |old| old.max(severity)));
                }
                if !report.offenses.is_empty() {
                    output.write("\n")?;
                    self.bar_length = 0;
                    emit_fuubar_file(output, report, &self.options)?;
                }
                if self.options.color {
                    output.write(&format!(
                        "\x1b[{}m",
                        self.severest.map_or("32", fuubar_severity_color)
                    ))?;
                }
                self.increment_bar(output, index + 1)?;
                if self.options.color {
                    output.write("\x1b[0m")?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn finished(
        &mut self,
        reports: &[FileReport],
        corrected_count: usize,
        output: &mut impl Output,
    ) -> Result<()> {
        self.options.corrected_count = corrected_count;
        match self.format {
            Format::Progress | Format::Pacman => {
                output.puts("")?;
                if offense_count(reports) > 0 {
                    output.puts("")?;
                    output.puts("Offenses:")?;
                    output.puts("")?;
                    for report in reports {
                        emit_clang_file(output, report, &self.options, false)?;
                    }
                }
                output.puts("")?;
                output.puts(&summary(reports, &self.options))
            }
            Format::Simple | Format::Clang | Format::Tap | Format::Quiet => {
                if self.format != Format::Quiet || offense_count(reports) > 0 {
                    output.puts("")?;
                    output.puts(&summary(reports, &self.options))?;
                }
                Ok(())
            }
            Format::Fuubar | Format::Autogenconf => {
                output.puts("")?;
                if self.format == Format::Autogenconf {
                    output.puts("")?;
                }
                output.puts(&fuubar_summary(reports, &self.options))
            }
            Format::Json => output.owned(render_json(reports, &self.options)?),
            Format::Html => output.owned(render_html(reports, &self.options)),
            Format::Junit => emit_junit(output, reports, &self.options),
            Format::Github => emit_github(output, reports, &self.options),
            Format::Markdown => emit_markdown(output, reports, &self.options),
            Format::Offenses => emit_offense_counts(output, reports, &self.options),
            Format::Worst => emit_worst(output, reports, self.options.cwd),
            Format::Emacs | Format::Files => Ok(()),
        }
    }

    fn increment_bar(&mut self, output: &mut impl Output, completed: usize) -> Result<()> {
        let total = self.options.target_file_count;
        let percentage = if total == 0 {
            100
        } else {
            ((completed as u128 * 100) / total as u128).min(100) as usize
        };
        let finished = completed >= total;
        let length = 11 + (self.bar.len() - 12) * percentage / 100 + usize::from(finished);
        output.write(self.bar.get(self.bar_length..length).unwrap_or(""))?;
        self.bar_length = length;
        if finished {
            output.write("\n")?;
        }
        Ok(())
    }
}

fn emit_simple_file(
    output: &mut impl Output,
    report: &FileReport,
    options: &FormatOptions<'_>,
) -> Result<()> {
    if report.offenses.is_empty() {
        return Ok(());
    }
    let path = smart_path(&report.path, options.cwd);
    // 本家の puts は色の reset 後に LF を渡す。
    output.puts(&paint(&format!("== {path} =="), "33", options.color))?;
    for offense in &report.offenses {
        let location = offense.location(&report.source);
        let message = simple_message(offense, options);
        let severity = paint(
            &offense.severity.code().to_string(),
            severity_color(offense.severity),
            options.color,
        );
        output.write(&format!(
            "{severity}:{:>3}:{:>3}: {message}\n",
            location.line, location.column
        ))?;
    }
    Ok(())
}

fn emit_clang_file(
    output: &mut impl Output,
    report: &FileReport,
    options: &FormatOptions<'_>,
    tap: bool,
) -> Result<()> {
    let path = paint(&smart_path(&report.path, options.cwd), "36", options.color);
    let prefix = if tap { "# " } else { "" };
    for offense in &report.offenses {
        let location = offense.location(&report.source);
        // TapFormatter は status / backtick の色を override して外す。
        let message = if tap {
            annotate_message(&display_message(offense, options), false)
        } else {
            simple_message(offense, options)
        };
        let severity = paint(
            &offense.severity.code().to_string(),
            severity_color(offense.severity),
            options.color,
        );
        output.write(&format!(
            "{prefix}{path}:{}:{}: {severity}: {message}\n",
            location.line, location.column
        ))?;
        if let Some((quoted, carets)) = source_excerpt(offense, report) {
            output.puts(&format!("{prefix}{quoted}"))?;
            output.puts(&format!("{prefix}{carets}"))?;
        }
    }
    Ok(())
}

fn emit_tap_file(
    output: &mut impl Output,
    report: &FileReport,
    index: usize,
    options: &FormatOptions<'_>,
) -> Result<()> {
    let status = if report.offenses.is_empty() {
        "ok"
    } else {
        "not ok"
    };
    output.puts(&format!(
        "{status} {} - {}",
        index + 1,
        smart_path(&report.path, options.cwd)
    ))?;
    emit_clang_file(output, report, options, true)
}

fn emit_emacs_file(
    output: &mut impl Output,
    report: &FileReport,
    options: &FormatOptions<'_>,
) -> Result<()> {
    let path = absolute_path(&report.path, options.cwd);
    for offense in &report.offenses {
        let location = offense.location(&report.source);
        output.write(&format!(
            "{}:{}:{}: {}: {}\n",
            path,
            location.line,
            location.column,
            offense.severity.code(),
            display_message(offense, options).replace('\n', " ")
        ))?;
    }
    Ok(())
}

fn emit_fuubar_file(
    output: &mut impl Output,
    report: &FileReport,
    options: &FormatOptions<'_>,
) -> Result<()> {
    let path = paint(&smart_path(&report.path, options.cwd), "36", options.color);
    for offense in &report.offenses {
        let location = offense.location(&report.source);
        let severity = paint(
            &offense.severity.code().to_string(),
            fuubar_severity_color(offense.severity),
            options.color,
        );
        let status = status_marker(offense);
        let mut message = if status.is_empty() {
            String::new()
        } else {
            paint(
                status,
                if offense.corrected { "32" } else { "33" },
                options.color,
            )
        };
        let annotated = annotated_message(offense, options);
        let mut rest = annotated.as_str();
        while let Some(open) = rest.find('`') {
            let Some(close) = rest[open + 1..].find('`') else {
                break;
            };
            message.push_str(&rest[..open]);
            message.push_str(&paint(
                &rest[open + 1..open + close + 1],
                "33",
                options.color,
            ));
            rest = &rest[open + close + 2..];
        }
        message.push_str(rest);
        output.write(&format!(
            "{path}:{}:{}: {severity}: {message}\n",
            location.line, location.column
        ))?;
        if let Some((quoted, carets)) = source_excerpt(offense, report) {
            let quoted = if location.last_line > location.start_line {
                quoted
                    .strip_suffix(" ...")
                    .map(|line| format!("{line} {}", paint("...", "33", options.color)))
                    .unwrap_or(quoted)
            } else {
                quoted
            };
            output.puts(&quoted)?;
            output.puts(&carets)?;
        }
    }
    Ok(())
}

fn emit_github(
    output: &mut impl Output,
    reports: &[FileReport],
    options: &FormatOptions<'_>,
) -> Result<()> {
    for report in reports {
        let path = smart_path(&report.path, options.cwd);
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            let level = if offense.severity >= options.fail_level {
                "error"
            } else {
                "warning"
            };
            output.write(&format!(
                "\n::{level} file={path},line={},col={}::{}",
                location.line,
                location.column,
                github_escape(&annotated_message(offense, options))
            ))?;
        }
    }
    output.puts("")
}

fn emit_junit(
    output: &mut impl Output,
    reports: &[FileReport],
    options: &FormatOptions<'_>,
) -> Result<()> {
    let registered: BTreeSet<_> = COP_REGISTRY_ORDER.iter().copied().collect();
    let count = reports
        .iter()
        .flat_map(|report| &report.offenses)
        .filter(|offense| registered.contains(offense.cop_name))
        .count();
    output.puts("<?xml version='1.0'?>")?;
    output.puts("<testsuites>")?;
    output.puts(&format!(
        "  <testsuite name='rubocop' tests='{}' failures='{count}'>",
        reports.len()
    ))?;
    for report in reports {
        let classname = xml_escape(&junit_classname(&report.path, options.cwd));
        let path = absolute_path(&report.path, options.cwd);
        let mut by_cop: BTreeMap<&str, Vec<&Offense>> = BTreeMap::new();
        for offense in &report.offenses {
            by_cop.entry(offense.cop_name).or_default().push(offense);
        }
        for cop in COP_REGISTRY_ORDER {
            let offenses = by_cop.remove(cop).unwrap_or_default();
            if offenses.is_empty() {
                if !options.display_only_failed {
                    output.puts(&format!(
                        "    <testcase classname='{classname}' name='{cop}'/>"
                    ))?;
                }
                continue;
            }
            output.puts(&format!(
                "    <testcase classname='{classname}' name='{cop}'>"
            ))?;
            for offense in offenses {
                let location = offense.location(&report.source);
                let text = if is_global_offense(offense) {
                    PSEUDO_SOURCE_RANGE.to_owned()
                } else {
                    format!("{path}:{}:{}", location.line, location.column)
                };
                output.puts(&format!(
                    "      <failure type='{cop}' message='{}'>",
                    xml_escape(&annotated_message(offense, options))
                ))?;
                output.puts(&format!("        {}", xml_escape(&text)))?;
                output.puts("      </failure>")?;
            }
            output.puts("    </testcase>")?;
        }
    }
    output.puts("  </testsuite>")?;
    output.puts("</testsuites>")
}

fn emit_markdown(
    output: &mut impl Output,
    reports: &[FileReport],
    options: &FormatOptions<'_>,
) -> Result<()> {
    output.write("# RuboCop Inspection Report\n\n")?;
    let count = offense_count(reports);
    let count = if count == 0 {
        "no offenses".to_owned()
    } else {
        plural(count, "offense")
    };
    output.write(&format!(
        "{} inspected, {count} detected:\n\n",
        plural(reports.len(), "file")
    ))?;
    for report in reports.iter().filter(|report| !report.offenses.is_empty()) {
        output.write(&format!(
            "### {} - ({})\n",
            relative_path(&report.path, options.cwd),
            plural(report.offenses.len(), "offense")
        ))?;
        for offense in &report.offenses {
            let location = offense.location(&report.source);
            output.write(&format!(
                "  * **Line # {} - {}:** {}\n\n",
                location.line,
                offense.severity.as_str(),
                annotated_message(offense, options)
            ))?;
            let mut code = quoted_source_line(offense, report).to_owned();
            if location.last_line != location.start_line {
                code.push_str(" ...");
            }
            if !code.trim_start().is_empty() {
                output.write(&format!("    ```rb\n    {code}\n    ```\n\n"))?;
            }
        }
    }
    Ok(())
}

fn emit_offense_counts(
    output: &mut impl Output,
    reports: &[FileReport],
    options: &FormatOptions<'_>,
) -> Result<()> {
    let mut counts: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
    let mut links: BTreeMap<&str, String> = BTreeMap::new();
    let mut offending_files = 0;
    for report in reports {
        offending_files += usize::from(!report.offenses.is_empty());
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
    rows.sort_by(|left, right| right.1.0.cmp(&left.1.0).then_with(|| left.0.cmp(right.0)));
    let total = rows.iter().map(|(_, (count, _))| count).sum::<usize>();
    let width = total.to_string().len() + 2;
    output.puts("")?;
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
        output.puts(&format!(
            "{:<width$}{cop}{correctable}{}",
            count.to_string(),
            links.get(cop).map(String::as_str).unwrap_or_default()
        ))?;
    }
    output.puts("--")?;
    output.puts(&format!("{total}  Total in {offending_files} files"))?;
    output.puts("")
}

fn emit_worst(output: &mut impl Output, reports: &[FileReport], cwd: &Path) -> Result<()> {
    let mut rows = reports
        .iter()
        .filter(|report| !report.offenses.is_empty())
        .map(|report| {
            let path = relative_path(&report.path, cwd);
            (
                report.offenses.len(),
                if path.is_empty() {
                    ".".to_owned()
                } else {
                    path
                },
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let total = rows.iter().map(|(count, _)| count).sum::<usize>();
    let width = total.to_string().len() + 2;
    output.puts("")?;
    for (count, path) in &rows {
        output.puts(&format!("{:<width$}{path}", count.to_string()))?;
    }
    output.puts("--")?;
    output.puts(&format!("{total}  Total in {} files", rows.len()))?;
    output.puts("")
}
