use std::collections::HashMap;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::support::final_pos;

use super::cop_directives::{Directive, Mode, directives, is_department};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if !context.source.text().contains("enable") {
        return;
    }
    let mut disabled = Counters {
        // `inject_disabled_cops_directives` gives every cop the configuration switched off an
        // outstanding disable, so an `enable all` always has one of them to undo. Only whether the
        // set is empty matters, and the run's selection decides that as much as the configuration:
        // `--only Foo` leaves a registry of one enabled cop and nothing to undo.
        config_pool: context.run_disables_a_cop(),
        ..Counters::default()
    };
    let parsed = directives(context);
    // `registry.disabled_names(config)`: an `enable` of a cop the configuration switched off has
    // something to undo, so it starts out counted as disabled.
    //
    // Ask for the resolved state rather than the literal `Enabled` value. RuboCop ships 159 cops
    // as `Enabled: pending`, which is neither `true` nor `false` but resolves to off, so reading
    // the literal as a boolean missed every one of them and their `enable` looked redundant.
    // Homebrew pairs a `disable`/`enable` around such cops in ten files, and autocorrect deleted
    // the `enable` line that the upstream run keeps.
    for directive in &parsed {
        for name in &directive.names {
            if !disabled.named.contains_key(name) && !context.cop_enabled(name) {
                disabled.named.insert(name.clone(), 1);
            }
        }
    }
    for directive in &parsed {
        if !directive.comment_only_line {
            continue;
        }
        let extras = extra_names(directive, &mut disabled);
        for name in &extras {
            register(directive, name, &extras, context, offenses);
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
    /// Whether the cops the configuration switched off still have their injected disable
    /// outstanding. `handle_enable_all` lowers every positive counter, so the first `enable all`
    /// spends them all at once.
    config_pool: bool,
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
        if self.config_pool {
            self.config_pool = false;
            enabled = true;
        }
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

/// `exclude_lint_department_cops`: the two cops `all` never stands for.
fn reached_by_all(name: &str) -> bool {
    name != "Lint/RedundantCopDisableDirective" && name != "Lint/Syntax"
}

/// `handle_enable_all` and `handle_switch`: the names this directive enabled for nothing.
fn extra_names(directive: &Directive, disabled: &mut Counters) -> Vec<String> {
    if directive.all {
        if directive.mode == Mode::Disable {
            disabled.blanket += 1;
            return Vec::new();
        }
        return if disabled.take_all() {
            Vec::new()
        } else {
            vec!["all".to_owned()]
        };
    }
    let mut extras = Vec::new();
    for name in &directive.names {
        if directive.mode == Mode::Disable {
            disabled.add(name);
            continue;
        }
        // A cop switched off through its department is switched on again by its own name.
        let key = if disabled.covers(name) {
            Some(name.clone())
        } else {
            name.split('/')
                .next()
                .map(str::to_owned)
                .filter(|department| is_department(department) && disabled.covers(department))
        };
        match key {
            Some(key) => disabled.take(&key),
            None => extras.push(name.clone()),
        }
    }
    extras
}

fn register(
    directive: &Directive,
    name: &str,
    extras: &[String],
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    // A cop reached through its department is reported as that department.
    let reported = directive.department_of(name).unwrap_or(name);
    let text = context.source.slice(directive.comment.clone());
    let Some(offset) = find_name(text, reported) else {
        return;
    };
    let start = directive.comment.start + offset;
    let range = start..start + reported.len();
    let label = if reported == "all" {
        "all cops".to_owned()
    } else {
        reported.to_owned()
    };
    let edit = if directive.names.len() == extras.len() || directive.all {
        // The whole directive goes, with the whitespace behind it --
        // `range_with_surrounding_space(side: :right)`, which reaches over the line breaks that
        // follow but stops there. The indentation of the next line belongs to that line's code.
        Edit {
            start: directive.range.start,
            end: final_pos(
                context.source.text(),
                directive.range.end,
                true, false,
                true,
                false,
            ),
            replacement: String::new(),
            safe: true,
        }
    } else {
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
