use std::collections::HashMap;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::cop_directives::{Directive, Mode, directives, is_department};

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if !context.source.text().contains("enable") {
        return;
    }
    // `registry.disabled_names(config)`: an `enable` of a cop the configuration switched off has
    // something to undo, so it starts out counted as disabled.
    let mut disabled: HashMap<String, usize> = HashMap::new();
    let parsed = directives(context);
    for directive in &parsed {
        for name in &directive.names {
            if !disabled.contains_key(name)
                && context.setting_of::<bool>(name, "Enabled") == Some(false)
            {
                disabled.insert(name.clone(), 1);
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

/// `handle_enable_all` and `handle_switch`: the names this directive enabled for nothing.
fn extra_names(directive: &Directive, disabled: &mut HashMap<String, usize>) -> Vec<String> {
    if directive.all {
        if directive.mode == Mode::Disable {
            return Vec::new();
        }
        let mut enabled = 0;
        for count in disabled.values_mut() {
            if *count > 0 {
                *count -= 1;
                enabled += 1;
            }
        }
        return if enabled == 0 {
            vec!["all".to_owned()]
        } else {
            Vec::new()
        };
    }
    let mut extras = Vec::new();
    for name in &directive.names {
        if directive.mode == Mode::Disable {
            *disabled.entry(name.clone()).or_insert(0) += 1;
            continue;
        }
        // A cop switched off through its department is switched on again by its own name.
        let key = if disabled.get(name).copied().unwrap_or(0) > 0 {
            Some(name.clone())
        } else {
            name.split('/').next().map(str::to_owned).filter(|department| {
                is_department(department) && disabled.get(department).copied().unwrap_or(0) > 0
            })
        };
        match key {
            Some(key) => {
                if let Some(count) = disabled.get_mut(&key) {
                    *count -= 1;
                }
            }
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
        // The whole directive goes, with the whitespace behind it -- `range_with_surrounding_space`
        // reaches over newlines unless it was told not to.
        let mut end = directive.range.end;
        let bytes = context.source.text().as_bytes();
        while end < bytes.len() && bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        Edit {
            start: directive.range.start,
            end,
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
        if !text.as_bytes().get(after).is_some_and(|byte| {
            byte.is_ascii_alphanumeric() || *byte == b'_'
        }) {
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
