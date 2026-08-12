use std::collections::HashMap;
use std::ops::Range;

use crate::cop_name::department;
use crate::diagnostic::Offense;
use crate::engine::is_mandatory_cop;
use crate::source::SourceFile;

#[derive(Clone, Debug, Default)]
struct Snapshot {
    all: bool,
    all_reason: Option<String>,
    cops: HashMap<String, Option<String>>,
}

#[derive(Debug, Default)]
pub struct DirectiveState {
    line_states: Vec<Snapshot>,
}

impl DirectiveState {
    pub fn parse(source: &SourceFile, comment_ranges: &[Range<usize>]) -> Self {
        let mut current = Snapshot::default();
        let mut stack = Vec::new();
        let mut line_states = Vec::with_capacity(source.line_count());
        // Where the first comment written on each line begins, as an offset into that line. Only
        // the parse knows this: a `#` can open a comment, an interpolation or nothing at all
        // depending on what it stands inside of.
        let mut comment_starts: HashMap<usize, usize> = HashMap::new();
        for range in comment_ranges {
            let (line_number, _) = source.line_column(range.start);
            let column = range.start - source.line_start(line_number);
            comment_starts
                .entry(line_number)
                .and_modify(|first| *first = (*first).min(column))
                .or_insert(column);
        }

        for line_number in 1..=source.line_count() {
            let line = source.line(line_number);
            let before = current.clone();
            let directive = comment_starts
                .get(&line_number)
                .and_then(|&start| parse_directive(line, start));

            if let Some(directive) = directive {
                if directive.inline && matches!(directive.action, Action::Disable) {
                    let mut line_only = before;
                    apply_disable(&mut line_only, &directive.cops, directive.reason);
                    line_states.push(line_only);
                    continue;
                }

                match directive.action {
                    Action::Disable => {
                        apply_disable(&mut current, &directive.cops, directive.reason);
                    }
                    Action::Enable => apply_enable(&mut current, &directive.cops),
                    Action::Push => {
                        stack.push(current.clone());
                        for (enable, cop) in directive.push_operations {
                            if enable {
                                apply_enable(&mut current, &[cop]);
                            } else {
                                apply_disable(&mut current, &[cop], directive.reason.clone());
                            }
                        }
                    }
                    Action::Pop => {
                        if let Some(snapshot) = stack.pop() {
                            current = snapshot;
                        }
                    }
                }
            }

            line_states.push(current.clone());
        }

        Self { line_states }
    }

    pub fn suppression(&self, offense: &Offense, source: &SourceFile) -> Option<Option<String>> {
        // `DirectiveComment` drops `Lint/Syntax` from the cop list of every directive -- named,
        // by department and by `all` (`#parsed_cop_names`, `#exclude_lint_department_cops`) -- so
        // a file cannot turn off the report that it does not parse.
        if is_mandatory_cop(offense.cop_name) {
            return None;
        }
        let (line, _) = source.line_column(offense.start);
        let state = self.line_states.get(line.saturating_sub(1))?;
        if let Some(reason) = state.cops.get(offense.cop_name) {
            return Some(reason.clone());
        }
        if let Some(reason) = state.cops.get(department(offense.cop_name)) {
            return Some(reason.clone());
        }
        state.all.then(|| state.all_reason.clone())
    }
}

fn apply_disable(state: &mut Snapshot, cops: &[String], reason: Option<String>) {
    if cops.iter().any(|cop| cop == "all") {
        state.all = true;
        state.all_reason = reason;
    } else {
        state
            .cops
            .extend(cops.iter().cloned().map(|cop| (cop, reason.clone())));
    }
}

fn apply_enable(state: &mut Snapshot, cops: &[String]) {
    if cops.iter().any(|cop| cop == "all") {
        state.all = false;
        state.all_reason = None;
        state.cops.clear();
    } else {
        for cop in cops {
            state.cops.remove(cop);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Disable,
    Enable,
    Push,
    Pop,
}

#[derive(Debug)]
struct Directive {
    action: Action,
    cops: Vec<String>,
    push_operations: Vec<(bool, String)>,
    reason: Option<String>,
    inline: bool,
}

fn parse_directive(line: &str, comment_start: usize) -> Option<Directive> {
    let comment = line.get(comment_start..)?;
    let marker_end = marker_end(comment)?;
    let command = comment[marker_end..].trim_start();
    let mode_end = command.find(char::is_whitespace).unwrap_or(command.len());
    let mode = &command[..mode_end];
    let remainder = command[mode_end..].trim();
    let (arguments, reason) =
        remainder
            .split_once("--")
            .map_or((remainder, None), |(arguments, reason)| {
                let reason = reason.trim();
                (
                    arguments.trim(),
                    (!reason.is_empty()).then(|| reason.to_owned()),
                )
            });
    let action = match mode {
        "disable" | "todo" => Action::Disable,
        "enable" => Action::Enable,
        "push" => Action::Push,
        "pop" => Action::Pop,
        _ => return None,
    };
    let inline = !line[..comment_start].trim().is_empty();

    if matches!(action, Action::Push) {
        let push_operations = arguments
            .split_whitespace()
            .filter_map(|specification| {
                let (operator, name) = specification.split_at_checked(1)?;
                matches!(operator, "+" | "-").then(|| (operator == "+", name.to_owned()))
            })
            .collect();
        return Some(Directive {
            action,
            cops: Vec::new(),
            push_operations,
            reason,
            inline,
        });
    }
    if matches!(action, Action::Pop) {
        return Some(Directive {
            action,
            cops: Vec::new(),
            push_operations: Vec::new(),
            reason,
            inline,
        });
    }
    let cops = cop_list(arguments);
    if cops.is_empty() {
        return None;
    }
    Some(Directive {
        action,
        cops,
        push_operations: Vec::new(),
        reason,
        inline,
    })
}

/// The cop names a `disable`/`enable`/`todo` directive lists.
///
/// `DirectiveComment::DIRECTIVE_COMMENT_REGEXP` matches `(all|COP(?:\s*,\s*COP)*)` and is not
/// anchored, so it stops at the first word that is not a cop name and leaves the rest of the
/// comment as prose: `# rubocop:disable Lint/UselessAssignment kept for the closure` disables the
/// cop it names. Reading the whole remainder as one name would silently ignore such a directive.
fn cop_list(arguments: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = arguments.trim_start();
    loop {
        let Some(length) = cop_name_length(rest) else {
            return names;
        };
        names.push(rest[..length].to_owned());
        let after = rest[length..].trim_start();
        match after.strip_prefix(',') {
            Some(next) => rest = next.trim_start(),
            None => return names,
        }
    }
}

/// The length of the cop name `text` starts with, following `COP_NAME_PATTERN`: one or more
/// `[A-Za-z]\w+` segments separated by slashes.
fn cop_name_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    loop {
        let start = index;
        if !bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            index += 1;
        }
        // The pattern is `[A-Za-z]\w+`, so a one-character segment does not match.
        if index - start < 2 {
            return None;
        }
        if bytes.get(index) != Some(&b'/') {
            return Some(index);
        }
        index += 1;
    }
}

fn marker_end(comment: &str) -> Option<usize> {
    let bytes = comment.as_bytes();
    let mut index = 0;
    if bytes.get(index) != Some(&b'#') {
        return None;
    }
    index += 1;
    skip_ascii_whitespace(bytes, &mut index);
    if !comment[index..].starts_with("rubocop") {
        return None;
    }
    index += "rubocop".len();
    skip_ascii_whitespace(bytes, &mut index);
    if bytes.get(index) != Some(&b':') {
        return None;
    }
    Some(index + 1)
}

fn skip_ascii_whitespace(bytes: &[u8], index: &mut usize) {
    while bytes
        .get(*index)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        *index += 1;
    }
}

#[cfg(test)]
mod tests {
    use crate::diagnostic::{Offense, Severity};
    use crate::source::SourceFile;

    use super::DirectiveState;

    #[test]
    fn handles_block_inline_reason_and_push_pop_directives() {
        let source = SourceFile::new(
            "test.rb",
            "# rubocop:disable Layout -- legacy\n\
             a = 1  \n\
             # rubocop:enable Layout\n\
             b = 2  # rubocop:disable Layout/TrailingWhitespace\n\
             # rubocop:push -Layout/TrailingWhitespace\n\
             c = 3  \n\
             # rubocop:pop\n\
             d = 4  \n"
                .to_owned(),
        );
        let comment_ranges: Vec<_> = (1..=source.line_count())
            .filter_map(|line_number| {
                let line = source.line(line_number);
                let local_start = line.find('#')?;
                let start = source.line_start(line_number) + local_start;
                Some(start..start + line[local_start..].trim_end().len())
            })
            .collect();
        let directives = DirectiveState::parse(&source, &comment_ranges);
        for (line, expected) in [(2, true), (4, true), (6, true), (8, false)] {
            let offense = Offense::new(
                "Layout/TrailingWhitespace",
                Severity::Convention,
                "test",
                source.line_start(line),
                source.line_start(line) + 1,
            );
            assert_eq!(
                directives.suppression(&offense, &source).is_some(),
                expected
            );
        }
        let offense = Offense::new(
            "Layout/TrailingWhitespace",
            Severity::Convention,
            "test",
            source.line_start(2),
            source.line_start(2) + 1,
        );
        assert_eq!(
            directives.suppression(&offense, &source),
            Some(Some("legacy".to_owned()))
        );
    }
}
