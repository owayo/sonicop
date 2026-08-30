//! What the four regexp-reading cops are handed instead of a `RegexpNode`.
//!
//! `RegexpNode#parsed_tree` blanks every interpolation to one space per character before handing
//! the pattern to the gem, so that the offsets it reports still line up with the source. That
//! blanking, the `x` flag, and the map from those offsets back into the file are the same three
//! things every one of the cops needs, so they are worked out once here.

use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

use super::regexp_tree::{self, Tree};
use crate::rules::send_node::named_children_of;

/// A regexp literal, parsed.
pub(super) struct Pattern {
    pub tree: Tree,
    /// Byte offset the pattern's first character sits at in the file.
    pub origin: usize,
    /// Byte offset of each character of the pattern, relative to `origin`, plus its end.
    ///
    /// Blanking replaces an interpolation with one space per *character*, so the blanked pattern
    /// and the source agree on where the character after it falls but not on where its byte does.
    /// The map is therefore built from the source, which is what an offense has to point into.
    offsets: Vec<usize>,
    /// The interpolations, as byte ranges, which two of the cops have to keep clear of.
    pub interpolations: Vec<Range<usize>>,
}

impl Pattern {
    /// The byte range in the file of a character range of the pattern.
    pub fn range(&self, characters: Range<usize>) -> Range<usize> {
        let last = self.offsets.len() - 1;
        let start = self.offsets[characters.start.min(last)];
        let end = self.offsets[characters.end.min(last)];
        self.origin + start..self.origin + end
    }
}

/// `node.parsed_tree` for a `regexp` node, or `None` where upstream would also have nothing.
pub(super) fn parse(node: Node<'_>, context: &RuleContext<'_>) -> Option<Pattern> {
    let text = context.source.node_text(node);
    // The body runs from after the opening delimiter to before the closing one and its flags.
    let (body, origin) = body_range(node, context)?;
    let mut blanked = String::with_capacity(body.end - body.start);
    let mut interpolations = Vec::new();
    let mut cursor = body.start;
    for child in named_children_of(node, context) {
        if child.kind_str() != "interpolation" {
            continue;
        }
        let child_range = child.byte_range();
        blanked.push_str(context.source.slice(cursor..child_range.start));
        // Spaces, not nothing: the offsets after an interpolation have to stay where they are.
        blanked.extend(
            context
                .source
                .slice(child_range.clone())
                .chars()
                .map(|_| ' '),
        );
        interpolations.push(child_range.clone());
        cursor = child_range.end;
    }
    blanked.push_str(context.source.slice(cursor..body.end));
    let extended = text[body.end - node.start_byte()..].contains('x');
    let tree = regexp_tree::parse(&blanked, extended)?;
    let source = context.source.slice(body.clone());
    let mut offsets: Vec<usize> = source.char_indices().map(|(index, _)| index).collect();
    offsets.push(body.end - body.start);
    Some(Pattern {
        tree,
        origin,
        offsets,
        interpolations,
    })
}

/// The span of the pattern itself, without its delimiters or flags, and where it starts.
fn body_range(node: Node<'_>, context: &RuleContext<'_>) -> Option<(Range<usize>, usize)> {
    let text = context.source.node_text(node);
    let start = node.start_byte();
    // `/…/`, `%r{…}`, `%r[…]`, `%r(…)`, `%r<…>` and `%r|…|` are all one opening delimiter wide
    // once the `%r` is behind us. Ruby takes any punctuation as that delimiter, multibyte ones
    // included, so its width is measured rather than assumed.
    let opener = match text.strip_prefix("%r") {
        Some(rest) => 2 + rest.chars().next()?.len_utf8(),
        None => text.chars().next()?.len_utf8(),
    };
    let closer = text.rfind(closing_delimiter(text)?)?;
    if closer < opener {
        return None;
    }
    Some((start + opener..start + closer, start + opener))
}

fn closing_delimiter(text: &str) -> Option<char> {
    let opener = if text.starts_with("%r") {
        text.chars().nth(2)?
    } else {
        text.chars().next()?
    };
    Some(match opener {
        '{' => '}',
        '[' => ']',
        '(' => ')',
        '<' => '>',
        other => other,
    })
}
