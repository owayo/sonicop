use std::collections::{HashMap, HashSet};
use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::engine::LiteralEncoding;
use crate::rules::RuleContext;

use super::literal::{self, Quoting};

const MSG: &str = "Prefer string interpolation to string concatenation.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let conservative = context
        .setting::<String>("Mode")
        .is_some_and(|mode| mode == "conservative");
    let heredocs = Heredocs::new(context);
    // How the source's declared encoding reads a literal's bytes back out.
    let encoding = crate::engine::declared_literal_encoding(context.source.text());
    // `@current_offense_locations`, which drops the second report of one range: every `+` in a
    // chain finds the same topmost node, and only the first of them runs the corrector.
    let mut reported: HashSet<usize> = HashSet::new();
    // `@corrected_nodes`, which stops an inner concatenation from being rewritten inside text an
    // outer one has already replaced.
    let mut corrected: HashSet<usize> = HashSet::new();

    for node in context.nodes_of_any(&["binary", "call"]) {
        let Some((receiver, argument)) = operands(context, node) else {
            continue;
        };
        // `string_concatenation?`: a plain literal on either side of the operator.
        if !heredocs.is_str(context, receiver) && !heredocs.is_str(context, argument) {
            continue;
        }
        // A concatenation broken over lines belongs to `Style/LineEndConcatenation`.
        if heredocs.is_str(context, receiver)
            && heredocs.is_str(context, argument)
            && node.start_position().row != node.end_position().row
            && operator_ends_a_line(context.source.node_text(node))
        {
            continue;
        }

        let topmost = topmost_plus(context, node);
        let parts = collect_parts(context, topmost);
        if conservative
            && !parts
                .first()
                .is_some_and(|first| heredocs.is_str(context, *first))
        {
            continue;
        }
        if !reported.insert(topmost.id()) {
            continue;
        }

        let offense = context.offense(MSG, topmost.byte_range());
        let correctable = parts.iter().all(|part| !uncorrectable(&heredocs, *part))
            && !corrected_ancestor(topmost, &corrected);
        offenses.push(match correctable {
            false => offense,
            true => {
                corrected.insert(topmost.id());
                offense.corrected_by(Edit {
                    start: topmost.start_byte(),
                    end: topmost.end_byte(),
                    replacement: replacement(context, &parts, encoding),
                    safe: false,
                })
            }
        })
    }
}

/// The two sides of a `+`, whichever way the call was spelled.
fn operands<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    if !is_plus(context, node) {
        return None;
    }
    match node.kind() {
        "binary" => {
            let left = node.child_by_field_name("left")?;
            match super::nodes::is_bare_jump(left) {
                true => None,
                false => Some((left, node.child_by_field_name("right")?)),
            }
        }
        _ => {
            let arguments = super::nodes::children(node.child_by_field_name("arguments")?);
            match arguments.as_slice() {
                [only] => Some((node.child_by_field_name("receiver")?, *only)),
                _ => None,
            }
        }
    }
}

/// `plus_node?`, which does not care how many arguments the call carries.
fn is_plus(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let selector = match node.kind() {
        "binary" => node.child_by_field_name("operator"),
        "call" => node.child_by_field_name("method"),
        _ => None,
    };
    selector.is_some_and(|selector| context.source.node_text(selector) == "+")
}

/// `find_topmost_plus_node`. A parenthesized subexpression is a `begin` upstream, which stops the
/// walk; an argument list is not a node there at all, so it is stepped over.
fn topmost_plus<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Node<'tree> {
    let mut current = node;
    while let Some(parent) = upstream_parent(current) {
        if !is_plus(context, parent) {
            break;
        }
        current = parent;
    }
    current
}

fn upstream_parent(node: Node<'_>) -> Option<Node<'_>> {
    let parent = node.parent()?;
    match parent.kind() {
        "argument_list" => parent.parent(),
        _ => Some(parent),
    }
}

/// `collect_parts`: the chain flattened into the operands that are not themselves `+`.
fn collect_parts<'tree>(context: &RuleContext<'_>, node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut parts = Vec::new();
    collect_into(context, node, &mut parts);
    parts
}

fn collect_into<'tree>(context: &RuleContext<'_>, node: Node<'tree>, parts: &mut Vec<Node<'tree>>) {
    match operands(context, node) {
        Some((receiver, argument)) => {
            collect_into(context, receiver, parts);
            collect_into(context, argument, parts);
        }
        None => parts.push(node),
    }
}

/// `/\+\s*\n/` over the node's own source.
fn operator_ends_a_line(source: &str) -> bool {
    let bytes = source.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'+'
            && bytes[index + 1..]
                .iter()
                .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\x0b' | b'\x0c'))
                == Some(&b'\n')
    })
}

/// `uncorrectable?`: text the interpolation could not carry over unchanged.
fn uncorrectable(heredocs: &Heredocs, part: Node<'_>) -> bool {
    part.start_position().row != part.end_position().row
        || heredocs.is_heredoc(part)
        || has_block_descendant(part)
}

/// `part.each_descendant(:any_block).any?`. Upstream's block node covers the call it is attached
/// to, so the braces hanging directly off `part` are `part` itself rather than a descendant.
fn has_block_descendant(part: Node<'_>) -> bool {
    let mut stack = vec![part];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "block" | "do_block")
            && node.parent().is_some_and(|parent| parent.id() != part.id())
        {
            return true;
        }
        crate::rules::push_named_children(node, &mut stack);
    }
    false
}

fn corrected_ancestor(node: Node<'_>, corrected: &HashSet<usize>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if corrected.contains(&parent.id()) {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// `replacement`: one double-quoted string holding every part.
fn replacement(context: &RuleContext<'_>, parts: &[Node<'_>], encoding: LiteralEncoding) -> String {
    let body: String = parts
        .iter()
        .map(|part| adjust_str(context, *part, encoding))
        // `handle_quotes`: a part that came out as a bare quote would close the literal.
        .map(|part| match part == "\"" {
            true => "\\\"".to_owned(),
            false => part,
        })
        .collect();
    format!("\"{body}\"")
}

/// `adjust_str`: what one part contributes to the interpolated string.
fn adjust_str(context: &RuleContext<'_>, node: Node<'_>, encoding: LiteralEncoding) -> String {
    match node.kind() {
        "string" | "character" => match interpolated(context, node) {
            // A literal carrying an interpolation is a `dstr`, whose children are spelled out one
            // by one rather than through the value of the whole.
            Some(literal) => literal
                .pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Code(node) => adjust_str(context, *node, encoding),
                    Piece::Text(range) => {
                        let source = &context.source.text()[range.clone()];
                        let bytes =
                            literal::decode_raw(source, literal.quoting, &literal.delimiters);
                        escaped(source, &bytes, encoding)
                    }
                })
                .collect(),
            None => {
                let bytes = plain_body(context, node)
                    .map(|(body, quoting, delimiters)| {
                        literal::decode_raw(body, quoting, &delimiters)
                    })
                    .unwrap_or_default();
                escaped(context.source.node_text(node), &bytes, encoding)
            }
        },
        // `'a' 'b'`, `(...)` and `#{...}` are all a list of children upstream.
        "chained_string" | "parenthesized_statements" | "interpolation" => {
            super::nodes::children(node)
                .into_iter()
                .map(|child| adjust_str(context, child, encoding))
                .collect()
        }
        _ => format!("#{{{}}}", context.source.node_text(node)),
    }
}

/// A literal's value written for a double-quoted string: `inspect` for text that came out of one,
/// and the narrower escape upstream applies to text that came out of a single-quoted literal.
///
/// The value is carried as bytes because `"\xFF"` holds one that is not a character at all, and
/// `inspect` writes that byte back as the escape it was written with.
fn escaped(source: &str, bytes: &[u8], encoding: LiteralEncoding) -> String {
    // A seven-bit source keeps that encoding only for a literal written in ASCII throughout; one
    // carrying a character of its own is text, whatever the file said.
    //
    // A `\u` escape retags the literal as text upstream, so its code point would be written back
    // as the character. It is spelled out byte by byte here instead: the character could not be
    // written back into a file this one declares to hold only ASCII, and refusing to write costs
    // every correction in it rather than this one.
    let by_byte = match encoding {
        LiteralEncoding::Binary => true,
        LiteralEncoding::SevenBit => source.is_ascii(),
        LiteralEncoding::Text => false,
    };
    match source.starts_with('\'') {
        true => escape_single_quoted(&String::from_utf8_lossy(bytes)),
        false => literal::inspect_bytes(bytes, by_byte),
    }
}

/// The body of a literal with no interpolation in it, and how its delimiters read.
fn plain_body<'a>(
    context: &'a RuleContext<'_>,
    node: Node<'_>,
) -> Option<(&'a str, Quoting, Vec<char>)> {
    // `?a` is a one-character string with the full set of escapes and no delimiter to escape.
    if node.kind() == "character" {
        return Some((
            &context.source.node_text(node)[1..],
            Quoting::Double,
            Vec::new(),
        ));
    }
    let first = node.child(0)?;
    let last = node.child(node.child_count().saturating_sub(1) as u32)?;
    if first.id() == last.id() {
        return None;
    }
    let opener = context.source.node_text(first);
    let quoting = match opener.starts_with('\'') || opener.starts_with("%q") {
        true => Quoting::Single,
        false => Quoting::Double,
    };
    let delimiters = vec![
        opener.chars().next_back()?,
        context.source.node_text(last).chars().next()?,
    ];
    Some((
        &context.source.text()[first.end_byte()..last.start_byte()],
        quoting,
        delimiters,
    ))
}

/// One child of an interpolated literal: a run of plain text, or an embedded expression.
enum Piece<'tree> {
    Text(Range<usize>),
    Code(Node<'tree>),
}

struct Interpolated<'tree> {
    quoting: Quoting,
    delimiters: Vec<char>,
    pieces: Vec<Piece<'tree>>,
}

/// The children upstream's `dstr` holds, or `None` when the literal has no interpolation and is a
/// plain `str`.
fn interpolated<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<Interpolated<'tree>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'tree>> = node.children(&mut cursor).collect();
    if !children.iter().any(|child| child.kind() == "interpolation") {
        return None;
    }
    let (first, last) = (children.first()?, children.last()?);
    let opener = context.source.node_text(*first);
    let quoting = match opener.starts_with('\'') || opener.starts_with("%q") {
        true => Quoting::Single,
        false => Quoting::Double,
    };
    let delimiters = vec![
        opener.chars().next_back()?,
        context.source.node_text(*last).chars().next()?,
    ];

    let mut pieces: Vec<Piece<'tree>> = Vec::new();
    let mut run: Option<Range<usize>> = None;
    for child in children {
        match child.kind() {
            "interpolation" => {
                if let Some(range) = run.take() {
                    pieces.push(Piece::Text(range));
                }
                pieces.push(Piece::Code(child));
            }
            "string_content" | "escape_sequence" => match &mut run {
                Some(range) => range.end = child.end_byte(),
                None => run = Some(child.byte_range()),
            },
            _ => {}
        }
    }
    if let Some(range) = run {
        pieces.push(Piece::Text(range));
    }
    Some(Interpolated {
        quoting,
        delimiters,
        pieces,
    })
}

/// `part.value.gsub(/(\\|"|#\{|#@|#\$)/, '\\\\\&')`.
fn escape_single_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '#' if matches!(characters.peek(), Some('{' | '@' | '$')) => {
                out.push('\\');
                out.push('#');
                out.extend(characters.next());
            }
            character => out.push(character),
        }
    }
    out
}

/// Which heredoc bodies make their opener a plain `str`.
///
/// Upstream reads a heredoc as one `str` or `dstr` node; the grammar splits it into the marker
/// where the expression sits and a body parked after the line. The two lists run in step, so the
/// n-th body is the n-th marker's.
///
/// The lexer hands the parser one part per physical line of the body, so only a body of exactly
/// one line -- and no interpolation -- arrives as a `str`. Two lines, or none at all, is a `dstr`
/// however plain its text is.
struct Heredocs {
    plain: HashMap<usize, bool>,
}

impl Heredocs {
    fn new(context: &RuleContext<'_>) -> Self {
        let mut plain = HashMap::new();
        let bodies: Vec<Node<'_>> = context.nodes_of("heredoc_body").collect();
        for (index, marker) in context.nodes_of("heredoc_beginning").enumerate() {
            let is_str = bodies
                .get(index)
                .is_some_and(|body| body_is_str(context, *body));
            plain.insert(marker.id(), is_str);
        }
        Self { plain }
    }

    fn is_heredoc(&self, node: Node<'_>) -> bool {
        node.kind() == "heredoc_beginning"
    }

    /// `str_type?`: one `str` node, which means nothing interpolated and nothing spread over more
    /// than one line.
    fn is_str(&self, context: &RuleContext<'_>, node: Node<'_>) -> bool {
        match node.kind() {
            "string" => interpolated(context, node).is_none() && written_on_one_line(context, node),
            "character" => true,
            "heredoc_beginning" => self.plain.get(&node.id()).copied().unwrap_or(false),
            _ => false,
        }
    }
}

/// Whether a literal's body holds no line break of its own.
///
/// The lexer hands the parser one part per physical line, so `"one\ntwo"` written over two lines
/// arrives as a `dstr` of two `str` children however plain its text is. A backslash at the end of
/// a line joins the two, and in a single-quoted literal it does not.
fn written_on_one_line(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(first) = node.child(0) else {
        return true;
    };
    let last = node.child(node.child_count().saturating_sub(1) as u32);
    let Some(last) = last.filter(|last| last.id() != first.id()) else {
        return true;
    };
    let body = &context.source.text()[first.end_byte()..last.start_byte()];
    let opener = context.source.node_text(first);
    let single = opener.starts_with('\'') || opener.starts_with("%q");
    line_breaks(body, single) == 0
}

/// The line breaks a literal's body carries, counting only those a backslash does not swallow.
fn line_breaks(body: &str, single_quoted: bool) -> usize {
    let bytes = body.as_bytes();
    let mut breaks = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            // A single-quoted literal only escapes the backslash and its own delimiters, so a
            // backslash before a line break is text and the break still splits the literal.
            b'\\' if !single_quoted || bytes.get(index + 1) == Some(&b'\\') => index += 2,
            b'\n' => {
                breaks += 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    breaks
}

fn body_is_str(context: &RuleContext<'_>, body: Node<'_>) -> bool {
    if has_interpolation_descendant(body) {
        return false;
    }
    let mut cursor = body.walk();
    let end = body
        .children(&mut cursor)
        .find(|child| child.kind() == "heredoc_end")
        .map_or(body.end_byte(), |terminator| terminator.start_byte());
    let text = &context.source.text()[body.start_byte()..end];
    // The body opens with the newline that closed the line the marker was written on.
    let content = text
        .strip_prefix("\r\n")
        .or_else(|| text.strip_prefix('\n'))
        .unwrap_or(text);
    physical_lines(content) == 1
}

/// The lines the lexer splits a heredoc body into, which is one per line break it keeps.
fn physical_lines(content: &str) -> usize {
    line_breaks(content, false)
}

fn has_interpolation_descendant(node: Node<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == "interpolation" {
            return true;
        }
        crate::rules::push_named_children(current, &mut stack);
    }
    false
}
