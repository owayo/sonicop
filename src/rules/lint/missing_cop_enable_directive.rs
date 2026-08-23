use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::cop_directives::{Mode, directives, is_department};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `MaxRangeSize` is `.inf` by default, which makes every bounded range acceptable and leaves
    // only the disables that run to the end of the file. A finite one bounds the closed ranges too.
    let max_range = context
        .setting::<serde_yaml_ng::Value>("MaxRangeSize")
        .and_then(|value| value.as_f64())
        .filter(|value| value.is_finite());
    let parsed = directives(context);
    // `# rubocop:disable all` turns this cop off too, so its own offense never survives.
    let mut open: Vec<usize> = Vec::new();
    // The line each open range was closed on, which `acceptable_range?` measures against.
    let mut closed: Vec<(usize, usize)> = Vec::new();
    // `push` saves the open ranges; the matching `pop` closes everything opened since.
    let mut saved: Vec<Vec<usize>> = Vec::new();
    for (index, directive) in parsed.iter().enumerate() {
        // `analyze_cop`: a directive sharing its line with code, or sitting behind prose inside a
        // comment, goes to `analyze_single_line`. A `disable` there covers its own line and
        // nothing more, so the range it leaves is closed and `acceptable_range?` lets it through;
        // an `enable` there closes nothing.
        if !directive.comment_only_line || directive.single_line {
            continue;
        }
        match directive.mode {
            Mode::Disable if !directive.all => open.push(index),
            Mode::Disable => {}
            // A range opened by `# rubocop:push` is closed by `# rubocop:pop`, not by an `enable`.
            Mode::Push => saved.push(open.clone()),
            Mode::Pop => {
                let line = context.source.line_column(directive.comment.start).0;
                if let Some(restored) = saved.pop() {
                    for &disabled in &open {
                        if !restored.contains(&disabled) {
                            closed.push((disabled, line));
                        }
                    }
                    open = restored;
                }
            }
            Mode::Enable => {
                let line = context.source.line_column(directive.comment.start).0;
                if directive.all {
                    closed.extend(open.drain(..).map(|disabled| (disabled, line)));
                    continue;
                }
                let mut still_open = Vec::with_capacity(open.len());
                for &disabled in &open {
                    let closes = parsed[disabled].names.iter().any(|name| {
                        directive.names.iter().any(|enabled| {
                            enabled == name
                                || (is_department(enabled) && name.starts_with(enabled.as_str()))
                                || (is_department(name) && enabled.starts_with(name.as_str()))
                        })
                    });
                    match closes {
                        true => closed.push((disabled, line)),
                        false => still_open.push(disabled),
                    }
                }
                open = still_open;
            }
        }
    }

    let mut ranges: Vec<(usize, Option<usize>)> = closed
        .into_iter()
        .map(|(index, line)| (index, Some(line)))
        .chain(open.into_iter().map(|index| (index, None)))
        .collect();
    ranges.sort_by_key(|(index, _)| *index);

    for (index, end) in ranges {
        let directive = &parsed[index];
        // `acceptable_range?`: `line_range.max - line_range.min < max_range + 2`. With the default
        // `.inf` every closed range passes and every open one fails, which is the shape this cop
        // had before `MaxRangeSize` was read at all.
        let start = context.source.line_column(directive.comment.start).0;
        let span = match end {
            Some(end) => end.saturating_sub(start) as f64,
            None => f64::INFINITY,
        };
        if span < max_range.unwrap_or(f64::INFINITY) + 2.0 {
            continue;
        }
        // `acceptable_range?`: a cop the configuration switched off is not expected to be
        // re-enabled, so the range it leaves open to the end of the file is acceptable. Upstream
        // reads that off `registry.enabled?`, which is the configuration and nothing else -- a cop
        // an `enable` directive put back on duty for this file still counts as switched off here.
        // The ranges are kept per cop upstream, so a directive naming one switched off and one left
        // on is reported under the one left on.
        // `message` names the first cop of the range, which for a department is the department.
        let Some(name) = directive
            .names
            .iter()
            .find(|name| is_department(name) || context.cop_enabled(name))
        else {
            continue;
        };
        let kind = if is_department(name) {
            "department"
        } else {
            "cop"
        };
        // `MSG` when the range is unbounded, `MSG_BOUND` when `MaxRangeSize` gave it one.
        let message = match max_range {
            Some(max) => format!(
                "Re-enable {name} {kind} within {} lines after disabling it.",
                match max.fract() == 0.0 {
                    true => format!("{}", max as i64),
                    false => format!("{max}"),
                }
            ),
            None => format!("Re-enable {name} {kind} with `# rubocop:enable` after disabling it."),
        };
        offenses.push(context.offense(message, directive.comment.clone()));
    }
}
