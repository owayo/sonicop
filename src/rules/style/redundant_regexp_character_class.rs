//! A character class holding one thing is that thing.
//!
//! Upstream hands the cop a `Regexp::Parser` tree and asks it for the sets of one expression. The
//! scan here answers the same question without building the tree: what a set holds is decided by
//! where its `]` falls and how many elements stand before it, and every construct that moves either
//! -- an escape, a nested set, a POSIX class, a range, an intersection -- is read out on the way
//! past. A pattern the parser upstream would refuse yields no offense at all, so the constructs it
//! rejects (`\x{...}`, an unknown POSIX class, an unterminated set) stop the scan instead.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::named_children_of;

/// `REQUIRES_ESCAPE_OUTSIDE_CHAR_CLASS_CHARS`: what a character class is the escape for.
const REQUIRES_ESCAPE: [char; 10] = ['.', '*', '+', '?', '{', '}', '(', ')', '|', '$'];

/// The names `[:name:]` may spell. Any other name is the `UnknownPosixClassError` that leaves the
/// pattern unparsed.
const POSIX_CLASSES: [&str; 14] = [
    "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
    "space", "upper", "word", "xdigit",
];

/// `/\s/`, which is narrower than Unicode whitespace.
const REGEXP_WHITESPACE: [char; 6] = [' ', '\t', '\r', '\n', '\u{c}', '\u{b}'];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("regex") {
        let Some(pattern) = Pattern::read(node, context) else {
            continue;
        };
        let Some(classes) = pattern.classes() else {
            continue;
        };
        for class in classes {
            let Some(range) = pattern.redundant(&class) else {
                continue;
            };
            let source = context.source.slice(range.clone());
            let element = replacement(source);
            offenses.push(
                context
                    .offense(
                        format!(
                            "Redundant single-element character class, `{source}` can be replaced \
                             with `{element}`."
                        ),
                        range.clone(),
                    )
                    .corrected_by(Edit {
                        start: range.start,
                        end: range.end,
                        replacement: element,
                        safe: true,
                    }),
            );
        }
    }
}

/// `without_character_class`: the class without its brackets, with the one element that would
/// otherwise open an interpolation escaped.
fn replacement(source: &str) -> String {
    let inner = &source[1..source.len() - 1];
    match source {
        "[#]" => format!("\\{inner}"),
        _ => inner.to_owned(),
    }
}

/// One regexp literal's pattern, as characters paired with where each of them starts.
///
/// `with_interpolations_blanked` puts a space in place of every character of an interpolation so
/// that what is left lines up with the source; the same substitution here keeps every offset the
/// scan produces addressable in the file.
struct Pattern {
    chars: Vec<char>,
    /// The byte each character starts at, with the end of the pattern appended so that the span of
    /// the last character can be named.
    offsets: Vec<usize>,
    extended: bool,
}

/// What a character class holds, counted the way `expressions.size` counts it.
enum Element {
    /// A literal, a type, an escape or a property: the kinds a redundant class can be reduced to.
    Simple(Range<usize>),
    /// A range, a nested set, an intersection or a POSIX class -- the `set`, `posixclass` and
    /// `nonposixclass` types the cop turns down.
    Compound,
}

struct Class {
    /// The index of the `[` and of the `]` that closes it.
    open: usize,
    close: usize,
    negative: bool,
    elements: Vec<Element>,
}

impl Pattern {
    fn read(node: Node<'_>, context: &RuleContext<'_>) -> Option<Self> {
        let last = u32::try_from(node.child_count()).ok()?.checked_sub(1)?;
        let (opening, closing) = (node.child(0)?, node.child(last)?);
        if closing.start_byte() < opening.end_byte() {
            return None;
        }
        // The closing token carries the delimiter and then the flags, and a delimiter may itself be
        // an `x`.
        let flags = &context.source.node_text(closing)[1..];
        let start = opening.end_byte();
        let end = closing.start_byte();
        let _cursor = node.walk();
        let interpolations: Vec<Range<usize>> = named_children_of(node, context)
            .into_iter()
            .filter(|child| child.kind_str() == "interpolation")
            .map(|child| child.byte_range())
            .collect();
        let mut chars = Vec::new();
        let mut offsets = Vec::new();
        for (offset, character) in context.source.slice(start..end).char_indices() {
            let byte = start + offset;
            offsets.push(byte);
            let interpolated = interpolations
                .iter()
                .any(|interpolation| interpolation.contains(&byte));
            chars.push(if interpolated { ' ' } else { character });
        }
        offsets.push(end);
        Some(Self {
            chars,
            offsets,
            extended: flags.contains('x'),
        })
    }

    fn at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    /// Every character class in the pattern, or `None` for a pattern `Regexp::Parser` would refuse.
    fn classes(&self) -> Option<Vec<Class>> {
        let mut found = Vec::new();
        let mut index = 0;
        while index < self.chars.len() {
            index = match self.at(index) {
                Some('\\') => {
                    // `\x{...}` is the one escape the parser rejects wherever it stands.
                    if self.at(index + 1) == Some('x') && self.at(index + 2) == Some('{') {
                        return None;
                    }
                    index + 2
                }
                // In free-spacing mode a `#` runs to the end of the line.
                Some('#') if self.extended => self.line_end(index),
                // `(?#...)` is a comment however the pattern was written.
                Some('(') if self.at(index + 1) == Some('?') && self.at(index + 2) == Some('#') => {
                    self.group_comment_end(index)
                }
                Some('[') => self.scan_class(index, &mut found)?,
                _ => index + 1,
            };
        }
        Some(found)
    }

    fn line_end(&self, index: usize) -> usize {
        self.chars[index..]
            .iter()
            .position(|character| *character == '\n')
            .map_or(self.chars.len(), |offset| index + offset)
    }

    fn group_comment_end(&self, index: usize) -> usize {
        let mut index = index + 3;
        while index < self.chars.len() {
            match self.at(index) {
                Some('\\') => index += 2,
                Some(')') => return index + 1,
                _ => index += 1,
            }
        }
        self.chars.len()
    }

    /// One `[...]`, and every class nested in it. A set that never closes is the parser's
    /// `PrematureEndError`.
    fn scan_class(&self, open: usize, found: &mut Vec<Class>) -> Option<usize> {
        let mut index = open + 1;
        let negative = self.at(index) == Some('^');
        if negative {
            index += 1;
        }
        let mut elements: Vec<Element> = Vec::new();
        let mut intersection = false;
        loop {
            match self.at(index)? {
                ']' => {
                    found.push(Class {
                        open,
                        close: index,
                        negative,
                        // An intersection is one expression of type `set`, whatever it separates.
                        elements: match intersection {
                            true => vec![Element::Compound],
                            false => elements,
                        },
                    });
                    return Some(index + 1);
                }
                '&' if self.at(index + 1) == Some('&') => {
                    intersection = true;
                    index += 2;
                }
                // A `-` between two elements folds them into one range. Written first or last it is
                // an element of its own.
                '-' if !elements.is_empty() && self.at(index + 1) != Some(']') => {
                    let (next, _) = self.scan_element(index + 1, found)?;
                    index = next;
                    if let Some(last) = elements.last_mut() {
                        *last = Element::Compound;
                    }
                }
                _ => {
                    let (next, element) = self.scan_element(index, found)?;
                    elements.push(element);
                    index = next;
                }
            }
        }
    }

    /// One member of a character class.
    fn scan_element(&self, index: usize, found: &mut Vec<Class>) -> Option<(usize, Element)> {
        match self.at(index)? {
            '[' => {
                if let Some(end) = self.posix_class_end(index)? {
                    return Some((end, Element::Compound));
                }
                Some((self.scan_class(index, found)?, Element::Compound))
            }
            '\\' => {
                let end = self.escape_end(index)?;
                Some((end, Element::Simple(index..end)))
            }
            _ => Some((index + 1, Element::Simple(index..index + 1))),
        }
    }

    /// Where `[:name:]` ends, `None` when the brackets open a nested set instead, and a refused
    /// pattern when the name is not one the parser knows.
    fn posix_class_end(&self, index: usize) -> Option<Option<usize>> {
        if self.at(index + 1) != Some(':') {
            return Some(None);
        }
        let mut cursor = index + 2;
        if self.at(cursor) == Some('^') {
            cursor += 1;
        }
        let name_start = cursor;
        while self.at(cursor).is_some_and(char::is_alphanumeric) {
            cursor += 1;
        }
        if self.at(cursor) != Some(':') || self.at(cursor + 1) != Some(']') {
            return Some(None);
        }
        let name: String = self.chars[name_start..cursor].iter().collect();
        match POSIX_CLASSES.contains(&name.as_str()) {
            true => Some(Some(cursor + 2)),
            false => None,
        }
    }

    /// Where the escape beginning at `index` ends.
    fn escape_end(&self, index: usize) -> Option<usize> {
        match self.at(index + 1)? {
            // `\x{...}` is rejected; `\xHH` takes at most two hexadecimal digits.
            'x' => match self.at(index + 2) {
                Some('{') => None,
                _ => Some(index + 2 + self.hex_digits(index + 2, 2)),
            },
            // `\u{...}` lists codepoints, `\uHHHH` names one.
            'u' => match self.at(index + 2) {
                Some('{') => {
                    let close = self.brace_end(index + 2)?;
                    match close == index + 3 {
                        true => None,
                        false => Some(close + 1),
                    }
                }
                _ => Some(index + 2 + self.hex_digits(index + 2, 4)),
            },
            'p' | 'P' => match self.at(index + 2) {
                Some('{') => Some(self.brace_end(index + 2)? + 1),
                _ => Some(index + 2),
            },
            // `\cX`, `\C-X` and `\M-X` each govern whatever follows, which may be another escape.
            'c' => self.escape_target_end(index + 2),
            'C' | 'M' if self.at(index + 2) == Some('-') => self.escape_target_end(index + 3),
            // An octal escape takes at most three digits, `\0` included.
            '0'..='7' => Some(index + 1 + self.octal_digits(index + 1, 3)),
            _ => Some(index + 2),
        }
    }

    fn escape_target_end(&self, index: usize) -> Option<usize> {
        match self.at(index)? {
            '\\' => self.escape_end(index),
            _ => Some(index + 1),
        }
    }

    fn brace_end(&self, open: usize) -> Option<usize> {
        self.chars[open..]
            .iter()
            .position(|character| *character == '}')
            .map(|offset| open + offset)
    }

    fn hex_digits(&self, index: usize, limit: usize) -> usize {
        (0..limit)
            .take_while(|offset| {
                self.at(index + offset)
                    .is_some_and(|c| c.is_ascii_hexdigit())
            })
            .count()
    }

    fn octal_digits(&self, index: usize, limit: usize) -> usize {
        (0..limit)
            .take_while(|offset| {
                self.at(index + offset)
                    .is_some_and(|c| ('0'..='7').contains(&c))
            })
            .count()
    }

    /// `redundant_single_element_character_class?`: the span to report, when the class holds one
    /// element that behaves the same way outside the brackets.
    fn redundant(&self, class: &Class) -> Option<Range<usize>> {
        if class.negative {
            return None;
        }
        let [Element::Simple(element)] = class.elements.as_slice() else {
            return None;
        };
        let text: String = self.chars[element.clone()].iter().collect();
        if codepoint_count(&text) >= 2
            || (self.extended && text.contains(REGEXP_WHITESPACE))
            // `\b` is a word boundary outside a character class and a backspace inside one.
            || text == "\\b"
            // `\1` to `\7` are backreferences outside a character class.
            || matches!(text.as_bytes(), [b'\\', digit] if (b'1'..=b'7').contains(digit))
            || (text.chars().count() == 1 && REQUIRES_ESCAPE.contains(&text.chars().next()?))
        {
            return None;
        }
        Some(self.offsets[class.open]..self.offsets[class.close + 1])
    }
}

/// `multiple_codepoints?`: how many codepoints `\u{...}` lists. Nothing else answers to
/// `codepoints` at all.
fn codepoint_count(text: &str) -> usize {
    let Some(list) = text
        .strip_prefix("\\u{")
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        return 0;
    };
    list.split_whitespace().count()
}
