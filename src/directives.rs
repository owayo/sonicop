use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::cop_name::department;
use crate::diagnostic::Offense;
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
        let comment_starts: HashSet<usize> =
            comment_ranges.iter().map(|range| range.start).collect();

        for line_number in 1..=source.line_count() {
            let line = source.line(line_number);
            let before = current.clone();
            let directive = find_comment_start(line)
                .filter(|start| comment_starts.contains(&(source.line_start(line_number) + start)))
                .and_then(|_| parse_directive(line));

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

fn parse_directive(line: &str) -> Option<Directive> {
    let comment_start = find_comment_start(line)?;
    let comment = &line[comment_start..];
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
    if arguments.is_empty() {
        return None;
    }
    let cops = arguments
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    Some(Directive {
        action,
        cops,
        push_operations: Vec::new(),
        reason,
        inline,
    })
}

fn find_comment_start(line: &str) -> Option<usize> {
    let mut escaped = false;
    let mut quote = None;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
            continue;
        }
        if character == '#' && quote.is_none() {
            return Some(index);
        }
    }
    None
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
