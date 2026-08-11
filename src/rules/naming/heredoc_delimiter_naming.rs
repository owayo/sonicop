use regex::Regex;

use super::support::heredocs;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Use meaningful heredoc delimiters.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let forbidden: Vec<Regex> = context
        .setting::<Vec<serde_yaml_ng::Value>>("ForbiddenDelimiters")
        .unwrap_or_default()
        .iter()
        .filter_map(super::support::ruby_regex)
        .collect();

    for heredoc in heredocs(context) {
        let delimiter = heredoc.delimiter(context.source);
        // `meaningful_delimiters?` asks two questions of the delimiter: that it holds a word
        // character at all -- Ruby's `\w`, so ASCII only -- and that no forbidden pattern reaches
        // it.
        if delimiter
            .contains(|character: char| character.is_ascii_alphanumeric() || character == '_')
            && !forbidden.iter().any(|pattern| pattern.is_match(delimiter))
        {
            continue;
        }
        // A heredoc with no body has no terminator the parser tracks separately, so the offense
        // falls back to the opening delimiter.
        let range = if heredoc.empty {
            heredoc.opening
        } else {
            heredoc.heredoc_end
        };
        offenses.push(context.offense(MSG, range));
    }
}
