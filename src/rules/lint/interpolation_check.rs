use tree_sitter::{Node, Parser};

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "Interpolation in single quoted string detected. Use double quoted strings if \
                   you need interpolation.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut parser = None;
    for node in context.nodes_of("string") {
        let source = context.source.node_text(node);
        // A heredoc's body reaches upstream as part of the heredoc node, which the cop refuses to
        // touch, and so does anything written inside one.
        if !holds_interpolation(source) || context.in_heredoc(node.byte_range()) {
            continue;
        }
        // Upstream sees a string that already interpolates as a `dstr`, and only inspects those
        // written in single quotes -- which never interpolate. So anything holding an
        // interpolation node is a double-quoted string, and says what it means.
        if interpolated(node) {
            continue;
        }
        let quoted = quote(source);
        if !interpolates(&quoted, &mut parser) {
            continue;
        }
        // `%{` cannot follow an expression: Ruby reads the `%` as the operator and the braces as a
        // hash, so `'a ' \` + `%{b}` becomes `'a ' % {b}`. Upstream writes it anyway and the file
        // stops parsing. The offense still stands -- the string does hold an interpolation -- but
        // there is no rewrite to offer, since the only other spelling would have to escape the
        // quotes the `%{}` was chosen to avoid.
        if quoted.starts_with("%{") && follows_an_expression(context, node.start_byte()) {
            offenses.push(context.offense(MSG, node.byte_range()));
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()).corrected_by(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: quoted,
            safe: true,
        }));
    }
}

/// Whether the text before `start` ends an expression, which is what makes a following `%` an
/// operator rather than the opening of a literal.
///
/// Whitespace and a backslash continuing the line are skipped: the continuation joins the two
/// lines, so what matters is the last thing Ruby read, not what the previous line looked like.
fn follows_an_expression(context: &RuleContext<'_>, start: usize) -> bool {
    let before = &context.source.text()[..start];
    let trimmed =
        before.trim_end_matches(|character: char| character.is_whitespace() || character == '\\');
    let Some(last) = trimmed.chars().last() else {
        // A literal opening the file is unambiguous.
        return false;
    };
    // A closing delimiter or a quote ends a value, and a value takes an operator next.
    if matches!(last, ')' | ']' | '}' | '"' | '\'' | '`') {
        return true;
    }
    // **A bare name does not end a value -- it can be a method that takes an argument**, and Ruby
    // reads `it %{...}` as passing the literal, not as `it % {...}`. Reading every word as a value
    // withheld the rewrite from `it 'a #{b}'`, which is the shape every RSpec description has:
    // upstream corrected it, this cop stopped offering to, and the `-A` comparison split on five
    // files. Only the words that genuinely close an expression count.
    let word: String = trimmed
        .chars()
        .rev()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    let word: String = word.chars().rev().collect();
    match word.chars().next() {
        // A numeric literal is a value.
        Some(character) if character.is_ascii_digit() => true,
        Some(_) => matches!(word.as_str(), "end" | "self" | "nil" | "true" | "false"),
        None => false,
    }
}

/// `/(?<!\\)#\{.*\}/`: a `#{` that is not escaped, closed on the same line. `.` does not cross a
/// newline upstream, so a brace opened on one line and closed on the next is not interpolation
/// waiting to happen.
fn holds_interpolation(source: &str) -> bool {
    source.lines().any(|line| {
        let bytes = line.as_bytes();
        (0..line.len().saturating_sub(1)).any(|index| {
            bytes[index] == b'#'
                && bytes[index + 1] == b'{'
                && (index == 0 || bytes[index - 1] != b'\\')
                && bytes[index + 2..].contains(&b'}')
        })
    })
}

fn interpolated(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind_str() == "interpolation")
}

/// The string rewritten as one that does interpolate: the single quotes around it become double
/// ones, or `%{...}` when the text itself holds a `"` that would otherwise end the literal early.
///
/// Only a leading and a trailing `'` are rewritten, which is what leaves `%q(#{x})` untouched --
/// and a literal that comes back unchanged is the same flat text it was, so nothing is reported.
fn quote(source: &str) -> String {
    let (open, close) = if source.contains('"') {
        ("%{", "}")
    } else {
        ("\"", "\"")
    };
    let mut quoted = String::with_capacity(source.len() + 2);
    let rest = match source.strip_prefix('\'') {
        Some(rest) => {
            quoted.push_str(open);
            rest
        }
        None => source,
    };
    match rest.strip_suffix('\'') {
        Some(body) => {
            quoted.push_str(body);
            quoted.push_str(close);
        }
        None => quoted.push_str(rest),
    }
    quoted
}

/// `valid_syntax?`: whether the rewritten literal parses, and parses as a string that interpolates
/// rather than as the same flat text it started as. `%q(#{x})` reads its braces literally, so
/// rewriting its quotes leaves it exactly as it was.
fn interpolates(quoted: &str, parser: &mut Option<Parser>) -> bool {
    let parser = parser.get_or_insert_with(|| {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .expect("the grammar the whole run is parsed with");
        parser
    });
    let Some(tree) = parser.parse(quoted, None) else {
        return false;
    };
    let root = tree.root_node();
    if root.has_error() {
        return false;
    }
    let mut interpolated = false;
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        match current.kind_str() {
            "interpolation" => interpolated = true,
            "character" if !valid_character(quoted, current) => return false,
            _ => {}
        }
        let mut cursor = current.walk();
        stack.extend(current.named_children(&mut cursor));
    }
    interpolated
}

/// Whether a character literal's Unicode escape is one Ruby accepts. The grammar takes `?\u123`,
/// but Ruby wants four hex digits, or a braced escape naming exactly one code point.
fn valid_character(source: &str, node: Node<'_>) -> bool {
    let Some(escape) = source[node.byte_range()].strip_prefix("?\\u") else {
        return true;
    };
    match escape.strip_prefix('{') {
        Some(braced) => braced.strip_suffix('}').is_some_and(|digits| {
            (1..=6).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
        }),
        None => escape.len() == 4 && escape.bytes().all(|byte| byte.is_ascii_hexdigit()),
    }
}
