use std::collections::HashSet;
use std::ops::Range;

use super::support::{whitespace_after, whitespace_before};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::source::is_protected;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let ranges = context.protected_ranges();
    let text = context.source.text();
    let bytes = text.as_bytes();
    let percent_literal_parens = percent_literal_parens(text, ranges);
    for index in 0..bytes.len() {
        if is_protected(index, ranges) || percent_literal_parens.contains(&index) {
            continue;
        }
        match bytes[index] {
            b'(' => {
                let spaces = whitespace_after(text, index + 1);
                // RuboCop compares two neighbouring tokens, so the space only counts when a
                // token follows it on the same line. A line break ends the line and a comment
                // means one follows, and neither leaves anything to report. An empty pair of
                // parentheses is reported here, from its opening side, and skipped at the
                // closing one so that the pair yields a single offense.
                let followed_by_a_token = !matches!(
                    bytes.get(spaces.end),
                    None | Some(b'\r' | b'\n') | Some(b'#')
                );
                if !spaces.is_empty() && followed_by_a_token {
                    offenses.push(paren_space_offense(context, spaces));
                }
            }
            b')' => {
                let spaces = whitespace_before(text, index);
                let starts_after_line_break =
                    spaces.start > 0 && matches!(bytes[spaces.start - 1], b'\r' | b'\n');
                if !spaces.is_empty()
                    && !starts_after_line_break
                    && bytes.get(spaces.start.wrapping_sub(1)) != Some(&b'(')
                {
                    offenses.push(paren_space_offense(context, spaces));
                }
            }
            _ => {}
        }
    }
}

fn percent_literal_parens(text: &str, protected: &[Range<usize>]) -> HashSet<usize> {
    let bytes = text.as_bytes();
    let mut parens = HashSet::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' || is_protected(index, protected) {
            index += 1;
            continue;
        }
        let opening = if bytes.get(index + 1) == Some(&b'(') {
            index + 1
        } else if bytes.get(index + 1).is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(index + 2) == Some(&b'(')
        {
            index + 2
        } else {
            index += 1;
            continue;
        };

        let mut depth = 1;
        let mut cursor = opening + 1;
        while cursor < bytes.len() {
            if bytes[cursor] == b'\\' {
                cursor = (cursor + 2).min(bytes.len());
                continue;
            }
            match bytes[cursor] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        parens.insert(opening);
                        parens.insert(cursor);
                        index = cursor;
                        break;
                    }
                }
                _ => {}
            }
            cursor += 1;
        }
        index += 1;
    }
    parens
}

fn paren_space_offense(context: &RuleContext<'_>, spaces: Range<usize>) -> Offense {
    context
        .offense("Space inside parentheses detected.", spaces.clone())
        .corrected_by(Edit {
            start: spaces.start,
            end: spaces.end,
            replacement: String::new(),
            safe: true,
        })
}
