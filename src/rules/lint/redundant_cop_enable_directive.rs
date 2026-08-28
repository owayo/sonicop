use std::collections::{HashMap, HashSet};

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::support::{self, Side};

use super::cop_directives::{ALL, Directive, Mode, directives, reached_by_all};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // `processed_source.blank?`, which is `ast.nil?`: a file of nothing but comments parses to no
    // tree at all, and this cop returns before it reads a single directive.
    if crate::rules::support::source_is_blank(context) || !context.source.text().contains("enable") {
        return;
    }
    // `extra_enabled_comments` seeds the counters from `registry.disabled_names(config)`: a cop the
    // configuration switched off has an outstanding disable for an `enable` to undo.
    //
    // The list is the run's, not the configuration's alone. `disabled_names` walks the *mobilized*
    // registry, so `--only Foo` leaves one enabled cop in it and nothing to undo, and `--except`
    // takes its cops out of the reckoning entirely. The engine settles it once for the run; asking
    // the configuration cop by cop instead reported every `--only` run's pending cops as disabled.
    let mut disabled = Counters::default();
    for name in context.disabled_cops() {
        disabled.add(name);
    }
    // `current_offense_locations`: `add_offense` keeps one offense per range, so the hundred-odd
    // names a department stands for report that department once -- and the first of them is the one
    // whose correction runs.
    let mut reported = HashSet::new();
    for directive in directives(context) {
        if !directive.comment_only_line {
            continue;
        }
        let names = directive.parsed_names();
        let extras = extra_names(&directive, &names, &mut disabled);
        // `match?(cop_names)`: the directive goes as a whole only when it undid nothing at all.
        let whole = directive.matches(&extras);
        for name in &extras {
            register(&directive, name, whole, context, offenses, &mut reported);
        }
    }
}

/// What `CommentConfig` counts each cop's outstanding disables in.
///
/// `handle_switch` reads `directive.cop_names`, which for `# rubocop:disable all` is
/// `all_cop_names` -- so a blanket disable raises the counter of every cop that exists. Walking
/// the registry to write that down would say nothing the one count they share does not, so the
/// blanket disables are kept apart here and read as part of every name's counter.
#[derive(Default)]
struct Counters {
    blanket: usize,
    named: HashMap<String, i64>,
}

impl Counters {
    /// Whether the cop has an outstanding disable for an `enable` to undo.
    fn covers(&self, name: &str) -> bool {
        let blanket = i64::try_from(self.blanket).unwrap_or(i64::MAX);
        let blanket = if reached_by_all(name) { blanket } else { 0 };
        blanket + self.named.get(name).copied().unwrap_or(0) > 0
    }

    fn add(&mut self, name: &str) {
        *self.named.entry(name.to_owned()).or_insert(0) += 1;
    }

    fn take(&mut self, name: &str) {
        *self.named.entry(name.to_owned()).or_insert(0) -= 1;
    }

    /// `handle_enable_all`: every counter that was positive comes down by one. Whether any of them
    /// was is the whole question -- an `enable all` that lowered nothing had nothing to undo.
    fn take_all(&mut self) -> bool {
        let blanket = self.blanket > 0;
        if blanket {
            self.blanket -= 1;
        }
        let mut enabled = blanket;
        for (name, count) in &mut self.named {
            // A name the blanket covers has already come down with it.
            if blanket && reached_by_all(name) {
                continue;
            }
            if *count > 0 {
                *count -= 1;
                enabled = true;
            }
        }
        enabled
    }
}

/// `handle_enable_all` and `handle_switch`: the names this directive enabled for nothing.
///
/// `names` is the directive's expanded list (`Directive::parsed_names`), which is what upstream
/// counts over -- a department is a hundred names here, not one.
fn extra_names<'n>(
    directive: &Directive,
    names: &[&'n str],
    disabled: &mut Counters,
) -> Vec<&'n str> {
    if directive.all {
        if directive.mode == Mode::Disable {
            disabled.blanket += 1;
            return Vec::new();
        }
        return if disabled.take_all() {
            Vec::new()
        } else {
            vec![ALL]
        };
    }
    let mut extras = Vec::new();
    for name in names {
        if directive.mode == Mode::Disable {
            disabled.add(name);
        } else if disabled.covers(name) {
            // `names[name] -= 1`: the cop's own counter comes down, **never its department's**. A
            // `# rubocop:disable Layout` raises one counter per Layout cop upstream, so enabling one
            // of them by name leaves the rest disabled -- lowering the department instead released
            // all hundred at once and the next `enable` of any of them looked redundant.
            disabled.take(name);
        } else {
            extras.push(*name);
        }
    }
    extras
}

fn register(
    directive: &Directive,
    name: &str,
    whole: bool,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    reported: &mut HashSet<(usize, usize)>,
) {
    // A cop reached through its department is reported as that department.
    let display = directive.department_of(name).unwrap_or(name);
    let text = context.source.slice(directive.comment.clone());
    let Some(offset) = find_name(text, display) else {
        return;
    };
    let start = directive.comment.start + offset;
    let range = start..start + display.len();
    // `current_offense_locations.add?(range)`: the second offense at a range is dropped, corrector
    // and all. Every cop of a department reports the department's own range, so this is what keeps
    // one `# rubocop:enable Layout` from being reported a hundred times.
    if !reported.insert((range.start, range.end)) {
        return;
    }
    let label = if display == ALL {
        "all cops".to_owned()
    } else {
        display.to_owned()
    };
    let edit = if whole {
        // `range_with_surrounding_space(directive.range, side: :right)`: the whole directive and the
        // line breaks behind it. The indentation of the next line belongs to that line's code.
        let span = support::range_with_surrounding_space(
            directive.range.clone(),
            context.source.text(),
            Side::Right,
            false,
            true,
            false,
        );
        Edit {
            start: span.start,
            end: span.end,
            replacement: String::new(),
            safe: true,
        }
    } else {
        // `range_with_comma`: the name with the comma that joined it to its neighbours, or -- when
        // it has none -- the comment and nothing else. **The comment's newline stays**, so a
        // directive that undid something leaves a blank line behind. That is upstream's output, not
        // a step left undone.
        range_with_comma(directive, &range, context)
    };
    offenses.push(
        context
            .offense(format!("Unnecessary enabling of {label}."), range)
            .corrections_anchored_at(directive.comment.clone())
            .corrected_by(edit),
    );
}

/// `comment.text.index(/name(?!\w)/)`.
fn find_name(text: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = text[from..].find(name) {
        let at = from + offset;
        let after = at + name.len();
        if !text
            .as_bytes()
            .get(after)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// `range_with_comma`: the name and the comma that joined it to its neighbours.
fn range_with_comma(
    directive: &Directive,
    range: &std::ops::Range<usize>,
    context: &RuleContext<'_>,
) -> Edit {
    let bytes = context.source.text().as_bytes();
    let mut start = range.start;
    while start > directive.comment.start && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    let mut end = range.end;
    while end < directive.comment.end && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    if start > directive.comment.start && bytes[start - 1] == b',' {
        return Edit {
            start: start - 1,
            end: range.end,
            replacement: String::new(),
            safe: true,
        };
    }
    if end < directive.comment.end && bytes[end] == b',' {
        let mut after = end + 1;
        if bytes.get(after) == Some(&b' ') {
            after += 1;
        }
        return Edit {
            start: range.start,
            end: after,
            replacement: String::new(),
            safe: true,
        };
    }
    Edit {
        start: directive.comment.start,
        end: directive.comment.end,
        replacement: String::new(),
        safe: true,
    }
}
