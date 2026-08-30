use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_iter;

const MSG: &str = "Redundant escape inside regexp literal";

/// `ALLOWED_ALWAYS_ESCAPES`: what has to stay escaped wherever it is written.
const ALWAYS_ALLOWED: &[char] = &[' ', '\n', '[', ']', '^', '\\', '#'];

/// `ALLOWED_OUTSIDE_CHAR_CLASS_METACHAR_ESCAPES`.
const OUTSIDE_CLASS: &[char] = &['.', '*', '+', '?', '{', '}', '(', ')', '|', '$'];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("regex") {
        let Some(literal) = Literal::read(context, node) else {
            continue;
        };
        if !literal.parsed {
            continue;
        }
        for escape in literal.escapes() {
            if literal.is_allowed(&escape) {
                continue;
            }
            let start = literal.start + escape.index;
            let end = start + 1 + escape.character.len_utf8();
            offenses.push(context.offense(MSG, start..end).corrected_by(Edit {
                start,
                end: start + 1,
                replacement: String::new(),
                safe: true,
            }));
        }
    }
}

struct Escape {
    /// Where the backslash sits in the pattern.
    index: usize,
    character: char,
    within_character_class: bool,
}

/// One regexp literal, read as the pattern between its delimiters.
struct Literal {
    /// The pattern with every interpolation blanked out, which is what upstream parses.
    pattern: String,
    /// Where the pattern begins in the file.
    start: usize,
    delimiters: [char; 2],
    extended: bool,
    /// Whether upstream's `Regexp::Parser` gets a tree at all. An encoding flag leaves
    /// `parsed_tree` nil, and a cop with no tree reports nothing.
    parsed: bool,
}

impl Literal {
    fn read(context: &RuleContext<'_>, node: Node<'_>) -> Option<Self> {
        let opening = node.child(0)?;
        let closing = node.child(u32::try_from(node.child_count()).ok()?.checked_sub(1)?)?;
        if closing.start_byte() < opening.end_byte() {
            return None;
        }
        let start = opening.end_byte();
        let mut pattern = context.source.slice(start..closing.start_byte()).to_owned();
        // `with_interpolations_blanked`: the code inside `#{}` is no part of the pattern, but its
        // width has to stay so that the offsets still line up.
        let _cursor = node.walk();
        for child in named_children_iter(node, context) {
            if child.kind_str() != "interpolation" {
                continue;
            }
            let range = child.start_byte() - start..child.end_byte() - start;
            pattern.replace_range(range.clone(), &" ".repeat(range.end - range.start));
        }
        Some(Self {
            pattern,
            start,
            delimiters: [
                context.source.node_text(opening).chars().next_back()?,
                context.source.node_text(closing).chars().next()?,
            ],
            extended: context.source.node_text(closing).contains('x'),
            parsed: !context
                .source
                .node_text(closing)
                .chars()
                .skip(1)
                .any(|flag| matches!(flag, 's' | 'e')),
        })
    }

    /// `each_escape`: every backslash sequence, and whether it sits inside a character class.
    fn escapes(&self) -> Vec<Escape> {
        let mut found = Vec::new();
        let bytes = self.pattern.as_bytes();
        let mut index = 0;
        let mut depth = 0usize;
        // Where the innermost character class started, so that a `]` written first reads literally.
        let mut class_start = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\\' => {
                    let Some(character) = self.pattern[index + 1..].chars().next() else {
                        break;
                    };
                    found.push(Escape {
                        index,
                        character,
                        within_character_class: depth > 0,
                    });
                    index += 1 + character.len_utf8();
                }
                // A POSIX bracket expression is one token rather than a nested class.
                b'[' if depth > 0 && self.posix_class_at(index).is_some() => {
                    index = self.posix_class_at(index).unwrap_or(index + 1);
                }
                b'[' => {
                    depth += 1;
                    class_start = index;
                    index += 1;
                }
                b']' if depth > 0 => {
                    let first = class_start
                        + if bytes.get(class_start + 1) == Some(&b'^') {
                            2
                        } else {
                            1
                        };
                    if index > first {
                        depth -= 1;
                    }
                    index += 1;
                }
                // In extended mode a `#` outside a class opens a comment that runs to the line end.
                b'#' if self.extended && depth == 0 => {
                    index += self.pattern[index..]
                        .find('\n')
                        .unwrap_or(bytes.len() - index);
                }
                _ => index += 1,
            }
        }
        found
    }

    /// Where a `[:name:]` written at `index` ends, when one is written there at all.
    fn posix_class_at(&self, index: usize) -> Option<usize> {
        let rest = self.pattern.get(index..)?;
        let inner = rest
            .strip_prefix("[:^")
            .or_else(|| rest.strip_prefix("[:"))?;
        let end = inner.find(":]")?;
        inner[..end]
            .chars()
            .all(|character| character.is_ascii_alphabetic())
            .then(|| index + (rest.len() - inner.len()) + end + 2)
    }

    fn is_allowed(&self, escape: &Escape) -> bool {
        let character = escape.character;
        if character.is_ascii_alphanumeric() {
            return true;
        }
        if ALWAYS_ALLOWED.contains(&character) || self.delimiters.contains(&character) {
            return true;
        }
        // `\#{` would otherwise open an interpolation, and `\#@a` a short one.
        if escape.index > 0
            && matches!(character, '@' | '$')
            && self.pattern.as_bytes().get(escape.index - 1) == Some(&b'#')
        {
            return true;
        }
        if !escape.within_character_class {
            return OUTSIDE_CLASS.contains(&character);
        }
        character == '-' && !self.hyphen_at_an_end_of_the_class(escape.index)
    }

    /// `char_class_begins_or_ends_with_escaped_hyphen?`: a `-` written first or last in a class is
    /// literal already, so escaping it there is redundant.
    fn hyphen_at_an_end_of_the_class(&self, index: usize) -> bool {
        let bytes = self.pattern.as_bytes();
        if bytes.get(index + 2) == Some(&b']') {
            return true;
        }
        if index >= 1 && bytes.get(index - 1) == Some(&b'[') {
            return index < 2 || bytes.get(index - 2) != Some(&b'\\');
        }
        false
    }
}
