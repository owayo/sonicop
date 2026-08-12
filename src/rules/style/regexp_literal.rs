use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG_USE_SLASHES: &str = "Use `//` around regular expression.";
const MSG_USE_PERCENT_R: &str = "Use `%r` around regular expression.";

/// The delimiter pairs whose nesting has to balance before a slash literal can become a `%r` one.
const PAIRS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style: String = context
        .setting("EnforcedStyle")
        .unwrap_or_else(|| "slashes".to_owned());
    let allow_inner_slashes: bool = context.setting("AllowInnerSlashes").unwrap_or(false);
    let preferred = preferred_delimiters(context);
    let omit_parentheses = context
        .setting_of::<String>("Style/MethodCallWithArgsParentheses", "EnforcedStyle")
        .is_some_and(|value| value == "omit_parentheses");

    for node in context.nodes_of("regex") {
        let Some(literal) = Literal::new(node, context) else {
            continue;
        };
        let slashes = literal.opener == "/";
        // A slash literal whose body nests the preferred delimiters unevenly cannot be rewritten as
        // a `%r` one at all, so upstream says nothing about it.
        if slashes && delimiters_conflict(&literal.literal_text, preferred) {
            continue;
        }

        let inner_slash = !allow_inner_slashes && literal.literal_text.contains('/');
        let message = if slashes {
            let allowed = (style == "slashes" && !inner_slash)
                || (style == "mixed" && literal.single_line() && !inner_slash);
            (!allowed).then_some(MSG_USE_PERCENT_R)
        } else {
            let allowed = (style == "slashes" && inner_slash)
                || style == "percent_r"
                || (style == "mixed" && !literal.single_line())
                || inner_slash
                || omits_parentheses(node, &literal, omit_parentheses);
            (!allowed).then_some(MSG_USE_SLASHES)
        };
        let Some(message) = message else {
            continue;
        };

        let (opening, closing) = match slashes {
            true => (format!("%r{}", preferred.0), preferred.1.to_string()),
            false => ("/".to_owned(), "/".to_owned()),
        };
        // `correct_delimiters` rewrites the two delimiters and `correct_inner_slashes` each slash
        // inside them, all as separate replacements: the parts of the body they do not name stay
        // available to the other cops running in the same pass.
        let before = inner_slash_for(literal.opener);
        let after = inner_slash_for(&opening);
        let mut edits = vec![Edit {
            start: literal.begin.start,
            end: literal.begin.end,
            replacement: opening,
            safe: true,
        }];
        if before != after {
            let body_start = literal.begin.end;
            edits.extend(
                literal
                    .body_text
                    .match_indices(before)
                    .map(|(offset, _)| Edit {
                        start: body_start + offset,
                        end: body_start + offset + before.len(),
                        replacement: after.to_owned(),
                        safe: true,
                    }),
            );
        }
        edits.push(Edit {
            start: literal.close.start,
            end: literal.close.end,
            replacement: closing,
            safe: true,
        });
        offenses.push(
            context
                .offense(message, node.byte_range())
                .corrected_by_all(edits),
        );
    }
}

/// One regexp literal split the way `node_body` reads it.
struct Literal<'a> {
    /// `loc.begin.source`: `/`, `%r{`, `%r/` and so on.
    opener: &'a str,
    /// The span of that opener, which the correction replaces whole.
    begin: std::ops::Range<usize>,
    /// The span of the closing delimiter alone, without any options that follow it.
    close: std::ops::Range<usize>,
    /// Everything between the delimiters, which is what the inner-slash correction rewrites.
    body_text: &'a str,
    /// The same with the interpolations taken out, which is what the cop's tests read.
    literal_text: String,
}

impl<'a> Literal<'a> {
    fn new(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<Self> {
        if node.child_count() < 2 {
            return None;
        }
        let begin = node.child(0)?;
        let close = node.child(node.child_count().saturating_sub(1) as u32)?;
        if begin.id() == close.id() {
            return None;
        }
        let text = context.source.text();
        let close_len = context.source.node_text(close).chars().next()?.len_utf8();
        let close = close.start_byte()..close.start_byte() + close_len;

        let mut literal_text = String::new();
        let mut start = begin.end_byte();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() != "interpolation" {
                continue;
            }
            literal_text.push_str(&text[start..child.start_byte()]);
            start = child.end_byte();
        }
        literal_text.push_str(&text[start..close.start]);

        Some(Self {
            opener: context.source.node_text(begin),
            body_text: &text[begin.end_byte()..close.start],
            literal_text,
            begin: begin.byte_range(),
            close,
        })
    }

    fn single_line(&self) -> bool {
        !self.body_text.contains('\n')
    }
}

/// `preferred_delimiters`: the `%r` entry of `Style/PercentLiteralDelimiters`.
fn preferred_delimiters(context: &RuleContext<'_>) -> (char, char) {
    let configured: HashMap<String, String> = context
        .setting_of("Style/PercentLiteralDelimiters", "PreferredDelimiters")
        .unwrap_or_default();
    let value = configured
        .get("%r")
        .or_else(|| configured.get("default"))
        .map_or("{}", String::as_str);
    let mut characters = value.chars();
    (
        characters.next().unwrap_or('{'),
        characters.next().unwrap_or('}'),
    )
}

/// `percent_r_delimiters_conflict?`: whether the body would leave the `%r` delimiters unbalanced.
fn delimiters_conflict(body: &str, preferred: (char, char)) -> bool {
    let (opening, closing) = preferred;
    if !PAIRS.contains(&preferred) {
        return false;
    }
    let mut depth: isize = 0;
    let mut characters = body.chars();
    while let Some(character) = characters.next() {
        // `\\.` swallows the escaped character, so an escaped delimiter does not count.
        if character == '\\' {
            characters.next();
        } else if character == opening {
            depth += 1;
        } else if character == closing {
            depth -= 1;
            if depth < 0 {
                return true;
            }
        }
    }
    depth != 0
}

/// `allowed_omit_parentheses_with_percent_r_literal?`: a `%r` literal handed to a method without
/// parentheses, where a slash literal would read as division.
fn omits_parentheses(node: Node<'_>, literal: &Literal<'_>, omit_parentheses: bool) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if !is_call(parent) {
        return false;
    }
    literal.literal_text.starts_with([' ', '=']) || omit_parentheses
}

/// Whether upstream would see the literal's parent as a `send`. tree-sitter puts the arguments of a
/// call in a list of their own, so the call is one level further out there, and it spells `super`
/// as a call even though upstream gives that a node type of its own.
fn is_call(parent: Node<'_>) -> bool {
    match parent.kind() {
        "call" | "unary" | "binary" | "element_reference" => true,
        "argument_list" => parent.parent().is_some_and(|grandparent| {
            grandparent.kind() == "call"
                && grandparent
                    .child_by_field_name("method")
                    .is_some_and(|method| method.kind() != "super")
        }),
        _ => false,
    }
}

/// `inner_slash_for`: how a slash is written inside a literal opened this way.
fn inner_slash_for(opening: &str) -> &'static str {
    match opening {
        "/" | "%r/" => "\\/",
        _ => "/",
    }
}
