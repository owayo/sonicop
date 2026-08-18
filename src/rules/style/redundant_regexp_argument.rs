use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `RESTRICT_ON_SEND`.
const RESTRICTED: [&str; 11] = [
    "byteindex",
    "byterindex",
    "gsub",
    "gsub!",
    "partition",
    "rpartition",
    "scan",
    "split",
    "start_with?",
    "sub",
    "sub!",
];

/// `STR_SPECIAL_CHARS`: the escapes a double-quoted string keeps a meaning for, so the backslash in
/// front of them survives the move out of the regexp.
const STR_SPECIAL_CHARS: [char; 25] = [
    'a', 'c', 'C', 'e', 'f', 'M', 'n', '"', '\'', '\\', 't', 'b', 'f', 'r', 'u', 'v', 'x', '0',
    '1', '2', '3', '4', '5', '6', '7',
];

/// `LITERAL_REGEX`'s first branch: the characters that mean only themselves in a pattern.
const LITERAL_CHARS: &str = "-,\"'!#%&<>=;:`~/";

/// `LITERAL_REGEX`'s second branch, negated: after a backslash, these still mean something.
const NON_LITERAL_ESCAPES: &str = "AbBdDgGhHkpPRwWXsSzZ0123456789";

/// A regexp argument that only ever matches one fixed string.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let double_quotes = context
        .setting_of::<String>("Style/StringLiterals", "EnforcedStyle")
        .is_some_and(|style| style == "double_quotes");
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        if !RESTRICTED.contains(&context.source.node_text(selector)) {
            continue;
        }
        let Some(argument) = node
            .field("arguments")
            .and_then(|arguments| super::nodes::children(arguments).into_iter().next())
        else {
            continue;
        };
        if argument.kind_str() != "regex" {
            continue;
        }
        let source = context.source.node_text(argument);
        let Some(content) = regexp_content(argument, context) else {
            continue;
        };
        // `regexp_node.regopt.children.empty?`: no options may be written after the delimiter.
        if !source.ends_with('/') {
            continue;
        }
        // `regexp_node.content == ' '`: `split(/ /)` is not the same as `split(' ')`.
        if content == " " {
            continue;
        }
        if !deterministic(source) {
            continue;
        }
        let prefer = preferred_argument(content, double_quotes);
        offenses.push(
            context
                .offense(
                    format!("Use string `{prefer}` as argument instead of regexp `{source}`."),
                    argument.byte_range(),
                )
                .corrected_by(Edit {
                    start: argument.start_byte(),
                    end: argument.end_byte(),
                    replacement: prefer,
                    safe: true,
                }),
        );
    }
}

/// The pattern between the delimiters, for a literal written with slashes.
fn regexp_content<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let source = context.source.node_text(node);
    source
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
}

/// `DETERMINISTIC_REGEX`: every character of the source is one that stands for itself, or an escape
/// that does. The source includes the delimiters, and `/` is one of the literal characters.
fn deterministic(source: &str) -> bool {
    // `\Z` lets a single trailing newline through.
    let source = crate::rules::support::chomp(source);
    if source.is_empty() {
        return false;
    }
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' {
            let Some(next) = characters.next() else {
                return false;
            };
            if NON_LITERAL_ESCAPES.contains(next) {
                return false;
            }
            continue;
        }
        let literal = character.is_alphanumeric()
            || character == '_'
            || character.is_whitespace()
            || LITERAL_CHARS.contains(character);
        if !literal {
            return false;
        }
    }
    true
}

/// `preferred_argument`, which picks the quotes the string can be written in.
fn preferred_argument(content: &str, double_quotes: bool) -> String {
    let mut new_argument = replacement(content);
    let quote = if new_argument.contains('"') {
        new_argument = new_argument.replace('\'', "\\'").replace("\\\"", "\"");
        '\''
    } else if new_argument.contains("\\'") {
        new_argument = escape_bare_quotes(&new_argument);
        '\''
    } else if new_argument.contains('\'') {
        new_argument = new_argument.replace('\'', "\\'");
        '\''
    } else if new_argument.contains('\\') || double_quotes {
        // A backslash left in the text needs the double-quoted form to keep its meaning;
        // otherwise `Style/StringLiterals` decides.
        '"'
    } else {
        '\''
    };
    format!("{quote}{new_argument}{quote}")
}

/// `new_argument.gsub!(/(?<!\\)((?:\\\\)*)'/) { "#{$1}\\'" }`: a quote that is not already escaped
/// gets a backslash, counting the run of backslashes in front of it.
fn escape_bare_quotes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut backslashes = 0_usize;
    for character in value.chars() {
        match character {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '\'' if backslashes % 2 == 0 => {
                out.push_str("\\'");
                backslashes = 0;
            }
            other => {
                backslashes = 0;
                out.push(other);
            }
        }
    }
    out
}

/// `replacement`: the pattern with the backslashes that meant something only inside a regexp taken
/// off.
fn replacement(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut characters = content.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        let Some(next) = characters.next() else {
            // A trailing backslash stands alone, and `delete!('\\')` takes it out.
            break;
        };
        if STR_SPECIAL_CHARS.contains(&next) {
            out.push('\\');
        }
        out.push(next);
    }
    out
}
