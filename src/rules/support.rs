//! Tree walks shared by cops in more than one department.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

/// `RangeHelp#final_pos`: how far a range grows when it takes in the blanks beside it.
///
/// The walk is a sequence rather than a loop -- spaces and tabs first, then line breaks, then any
/// remaining whitespace -- so with `newlines` alone a run of blanks after a line break is not
/// reached, and the caller that wants it has to ask for `whitespace` too. Reaching further matters:
/// a cop that removes a whole comment line and eats the indentation of the line below it moves that
/// line's code, which is a correction it never meant to make.
pub(crate) fn final_pos(
    text: &str,
    position: usize,
    forward: bool,
    continuations: bool,
    newlines: bool,
    whitespace: bool,
) -> usize {
    let mut position = move_pos(text, position, forward, true, |byte| {
        matches!(byte, b' ' | b'\t')
    });
    position = move_pos_str(text, position, forward, continuations, "\\\n");
    position = move_pos(text, position, forward, newlines, |byte| byte == b'\n');
    move_pos(text, position, forward, whitespace, is_ruby_space)
}

/// Which side of a range grows when the blanks beside it are taken in.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Side {
    Left,
    Right,
    Both,
}

/// `RangeHelp#range_with_surrounding_space`: the range with the blanks beside it taken in.
///
/// The three switches are upstream's keywords, and each one is a stage of [`final_pos`]:
/// `continuations` reaches over a `\` that ends a line, `newlines` over line breaks, `whitespace`
/// over anything Ruby's `\s` matches. **The stages do not run again**, so `newlines` alone stops at
/// the first blank beyond a break -- a line holding only spaces above the range survives.
///
/// Upstream's defaults are `newlines: true`, `whitespace: false`, `continuations: false`, and only
/// two of its call sites ask for continuations (`Style/NestedParenthesizedCalls` and
/// `ParenthesesCorrector#parens_range`). **Passing them everywhere would eat a `\` that the cop was
/// not asked to touch.**
pub(crate) fn range_with_surrounding_space(
    range: Range<usize>,
    text: &str,
    side: Side,
    continuations: bool,
    newlines: bool,
    whitespace: bool,
) -> Range<usize> {
    let start = match side {
        Side::Left | Side::Both => final_pos(
            text,
            range.start,
            false,
            continuations,
            newlines,
            whitespace,
        ),
        Side::Right => range.start,
    };
    let end = match side {
        Side::Right | Side::Both => {
            final_pos(text, range.end, true, continuations, newlines, whitespace)
        }
        Side::Left => range.end,
    };
    start..end
}

/// `/\s/` as Ruby's regexp engine and its lexer (`ISSPACE`) define it: the six ASCII spacing
/// characters and nothing else.
///
/// **Rust's `is_ascii_whitespace` is the same set minus the vertical tab**, so a walk written with it
/// stops one character early in `%w[a\vb]` and in an indentation that begins with a `\v`. A
/// no-break space is *not* in the set either way -- it is content, not spacing.
pub(crate) const fn is_ruby_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `String#strip` (and `rstrip` / `lstrip`): Ruby's `\s` **plus NUL**.
///
/// **Rust's `str::trim` is wrong in both directions here.** It reaches over every Unicode
/// `White_Space` character -- a no-break space, an ideographic space -- which Ruby leaves in place,
/// and it stops at a NUL, which Ruby strips. A line holding only a no-break space is *not* blank to
/// `strip.empty?`, so a cop ported with `trim().is_empty()` goes quiet where upstream reports.
///
/// Reach for [`is_ruby_space_char`] instead when the original is `/\s/` rather than `strip`: the
/// regexp does not match NUL.
pub(crate) fn is_ruby_strippable(character: char) -> bool {
    character == '\0' || is_ruby_space_char(character)
}

/// The same set spelled for a `char`, which is what a walk over `chars()` needs. **The set itself is
/// defined once**, in [`is_ruby_space`]: anything outside ASCII cannot be Ruby's `\s`, so a character
/// that does not fit in a byte is not one.
pub(crate) fn is_ruby_space_char(character: char) -> bool {
    u8::try_from(character).is_ok_and(is_ruby_space)
}

/// `move_pos_str`: the same walk over a fixed run of characters rather than a class.
///
/// The one caller wants `\` and a line break, which a class cannot express -- a backslash only ends
/// the line when the break follows it, and eating it on its own would join two statements.
fn move_pos_str(
    text: &str,
    mut position: usize,
    forward: bool,
    enabled: bool,
    needle: &str,
) -> usize {
    if !enabled {
        return position;
    }
    let width = needle.len();
    loop {
        let found = match forward {
            true => text.get(position..position + width) == Some(needle),
            false => position >= width && text.get(position - width..position) == Some(needle),
        };
        if !found {
            return position;
        }
        position = if forward {
            position + width
        } else {
            position - width
        };
    }
}

fn move_pos(
    text: &str,
    mut position: usize,
    forward: bool,
    enabled: bool,
    matches: impl Fn(u8) -> bool,
) -> usize {
    if !enabled {
        return position;
    }
    let bytes = text.as_bytes();
    loop {
        let probe = if forward {
            position
        } else {
            match position.checked_sub(1) {
                Some(probe) => probe,
                None => return position,
            }
        };
        match bytes.get(probe) {
            Some(byte) if matches(*byte) => {
                position = if forward { position + 1 } else { probe };
            }
            _ => return position,
        }
    }
}

use tree_sitter::{Node, Parser};

use crate::diagnostic::Edit;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{Argument, is_string, pair_key_symbol, string_text};

/// `ReparsedEquivalence#correction_parses?`: whether the exact correction a cop is about to offer
/// leaves source that still parses.
///
/// A cop that rewrites a construct into a differently shaped one cannot assert that the result
/// means the same thing, but it can insist that the result is Ruby at all. Upstream turns that into
/// the gate an offense is reported behind, which is what keeps a corrector that cannot handle an
/// unusual shape from emitting broken code rather than staying quiet.
pub(crate) fn correction_parses(context: &RuleContext<'_>, edits: &[Edit]) -> bool {
    trace_overlapping_edits(context, edits, edits.first().map_or(0, |edit| edit.start));
    // `Parser::ClobberingError`: a rewrite whose parts collide is no correction to begin with.
    let Some(corrected) = apply_edits(context.source.text(), edits) else {
        return false;
    };
    parses(&corrected)
}

/// Names a cop whose own edits overlap each other.
///
/// [`apply_edits`] refuses such a set, and every caller reads that refusal as "this correction does
/// not work" and drops the candidate -- so the cop goes quiet with nothing to say why. A cop asking
/// to remove one span twice is not a correction that fails, it is a corrector written twice over,
/// and the offense it loses never reaches the output to be noticed.
///
/// Set `SONICOP_TRACE_OVERLAP=1` to list them.
fn trace_overlapping_edits(context: &RuleContext<'_>, edits: &[Edit], at: usize) {
    /// Read once. Every offense of every cop on this path would otherwise pay for the lookup.
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("SONICOP_TRACE_OVERLAP").is_some());
    if !*ENABLED {
        return;
    }
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.start, edit.end));
    let mut cursor = 0;
    for edit in ordered {
        if edit.start < cursor {
            let (line, column) = context.source.line_column(at);
            eprintln!(
                "[overlap]\t{}\t{}:{}:{}",
                context.rule.name,
                context.source.path().display(),
                line,
                column
            );
            return;
        }
        cursor = edit.end;
    }
}

/// The source with every edit applied, or `None` when two of them overlap.
///
/// Sorting by span puts an insertion at a span's start before the span itself and one at its end
/// after it, which is the order `insert_before` and `insert_after` schedule them in.
fn apply_edits(text: &str, edits: &[Edit]) -> Option<String> {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.start, edit.end));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for edit in ordered {
        if edit.start < cursor || edit.end < edit.start || edit.end > text.len() {
            return None;
        }
        out.push_str(text.get(cursor..edit.start)?);
        out.push_str(&edit.replacement);
        cursor = edit.end;
    }
    out.push_str(text.get(cursor..)?);
    Some(out)
}

/// `ProcessedSource#valid_syntax?` for a source the run did not start from.
fn parses(text: &str) -> bool {
    parse(text).is_some()
}

fn parse(text: &str) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(text, None)?;
    if tree.root_node().has_error() || accepts_more_than_ruby(tree.root_node(), text) {
        return None;
    }
    Some(tree)
}

/// The places the grammar accepts a construct Ruby's own does not.
///
/// `not x` is an `expr` there, so it cannot stand as an argument or an element: `f(not b)` and
/// `[not a]` are both syntax errors while the grammar takes them as ordinary operands. Only the
/// `not(x)` spelling, whose parenthesis is written straight against the keyword, is a primary and
/// so allowed anywhere. A source the parser would reject has to be rejected here too, or a
/// correction that produces one reads as verified.
fn accepts_more_than_ruby(root: Node<'_>, text: &str) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if matches!(node.kind_str(), "argument_list" | "array")
            && node.named_children(&mut node.walk()).any(|child| {
                child.kind_str() == "unary"
                    && child.field("operator").is_some_and(|operator| {
                        &text[operator.byte_range()] == "not"
                            && text.as_bytes().get(operator.end_byte()) != Some(&b'(')
                    })
            })
        {
            return true;
        }
        push_named_children(node, &mut stack);
    }
    false
}

/// `MAX_VERIFICATION_FRAGMENT_SIZE`: past this, verification gives up and accepts the offense as
/// reported. Only machine-generated files come near it.
const MAX_VERIFICATION_FRAGMENT_SIZE: usize = 64 * 1024;

/// The kinds that both parse standalone and cannot capture an outer local variable, which is what
/// makes a fragment cut out of them reparse to the same tree.
const REPARSE_SCOPES: &[&str] = &[
    "method",
    "singleton_method",
    "class",
    "module",
    "singleton_class",
];

/// The two hooks `ReparsedEquivalence` lets a cop reach into the comparison with, and the answer it
/// wants for a scope too large to reparse.
///
/// The defaults are what a cop whose offense logic stands on its own asks for: the tree is compared
/// as it parses, and a scope past the size limit is accepted unverified. A cop for which the
/// verification *is* the offense logic turns both around.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Verification {
    /// `oversized: :verify`: reparse a scope past `MAX_VERIFICATION_FRAGMENT_SIZE` regardless, since
    /// accepting it unverified would report an offense nothing stands behind.
    pub(crate) verify_oversized: bool,
    /// `normalize_reparsed_ast` = `fold_string_concatenation`: joining lines merges split string
    /// literals and turns a mixed-quote pair into a `+` concatenation, which changes the tree
    /// without changing the string it builds. Folding every concatenation to a canonical form is
    /// what lets the two compare equal.
    pub(crate) fold_string_concatenation: bool,
    /// `foo()` and `foo` are one and the same node upstream: the parser builds nothing for an empty
    /// argument list, and a receiverless call carrying only its name is spelled as a bare name.
    /// The grammar here keeps the two apart -- `call` with an `argument_list` that has no children
    /// on one side, a lone `identifier` on the other -- so a correction that drops nothing but a
    /// pair of empty parentheses never compares equal without this.
    ///
    /// Upstream's parser tells `(send nil :foo)` from `(lvar :foo)` by remembering what it has seen
    /// assigned, which a tree already built cannot. The caller that turns this on has to keep the
    /// calls whose name a local variable holds out on its own.
    pub(crate) fold_empty_call_parentheses: bool,
}

/// `ReparsedEquivalence#verified_by_reparse`: the items whose corrections leave a tree equal to the
/// one they started from.
///
/// This turns "is this piece of syntax redundant?" into a question the parser answers, rather than
/// a hand-kept list of the places where it is not. Items sharing a scope are verified together
/// first, since one reparse then settles the whole group; the group falls back to one reparse each
/// when the batch does not hold.
pub(crate) fn verified_by_reparse<T>(
    context: &RuleContext<'_>,
    items: Vec<T>,
    edits_of: impl Fn(&T) -> Vec<Edit>,
    range_of: impl Fn(&T) -> Range<usize>,
    verification: Verification,
) -> Vec<T> {
    let text = context.source.text();
    let root = context.root_node();
    // `scope_groups`, keyed by node identity and ordered by the first item that reached each scope.
    let mut groups: Vec<(Option<Node<'_>>, Vec<T>)> = Vec::new();
    for item in items {
        trace_overlapping_edits(context, &edits_of(&item), range_of(&item).start);
        let scope = reparse_scope(root, &range_of(&item));
        let key = scope.map(|node| node.id());
        match groups
            .iter_mut()
            .find(|(seen, _)| seen.map(|node| node.id()) == key)
        {
            Some((_, group)) => group.push(item),
            None => groups.push((scope, vec![item])),
        }
    }

    let mut verified = Vec::new();
    for (scope, group) in groups {
        let span = scope.map_or(text.len(), |node| node.byte_range().len());
        if span > MAX_VERIFICATION_FRAGMENT_SIZE && !verification.verify_oversized {
            verified.extend(group);
            continue;
        }
        let original = normalized(scope.unwrap_or(root), text, verification);
        if group.len() > 1
            && corrections_verify(text, scope, &original, &group, &edits_of, verification)
        {
            verified.extend(group);
            continue;
        }
        verified.extend(group.into_iter().filter(|item| {
            corrections_verify(
                text,
                scope,
                &original,
                std::slice::from_ref(item),
                &edits_of,
                verification,
            )
        }));
    }
    verified
}

/// `reparse_scope`: the innermost node that both contains `range` and parses standalone.
fn reparse_scope<'tree>(root: Node<'tree>, range: &Range<usize>) -> Option<Node<'tree>> {
    let mut node = root;
    let mut scope = None;
    loop {
        if REPARSE_SCOPES.contains(&node.kind_str()) {
            scope = Some(node);
        }
        // `Range#contains?`: strictly wider on at least one side, so a child that spans exactly the
        // same text does not continue the descent.
        let mut cursor = node.walk();
        let next = node.named_children(&mut cursor).find(|child| {
            let span = child.byte_range();
            (range.start > span.start && span.end >= range.end)
                || (range.start >= span.start && span.end > range.end)
        });
        match next {
            Some(child) => node = child,
            None => return scope,
        }
    }
}

/// `corrections_verify?`: whether applying every item's correction leaves the scope parsing to the
/// tree it already had.
fn corrections_verify<T>(
    text: &str,
    scope: Option<Node<'_>>,
    original: &Sexp,
    items: &[T],
    edits_of: &impl Fn(&T) -> Vec<Edit>,
    verification: Verification,
) -> bool {
    let edits: Vec<Edit> = items.iter().flat_map(edits_of).collect();
    let Some(corrected) = apply_edits(text, &edits) else {
        return false;
    };
    let fragment = match scope {
        // `corrected_scope_fragment`: every edit is inside the scope, so the corrected scope ends
        // where it did plus what the edits added.
        Some(scope) => {
            let end = (scope.end_byte() + corrected.len()).checked_sub(text.len());
            match end.and_then(|end| corrected.get(scope.start_byte()..end)) {
                Some(fragment) => fragment,
                None => return false,
            }
        }
        None => corrected.as_str(),
    };
    parse(fragment)
        .is_some_and(|tree| &normalized(tree.root_node(), fragment, verification) == original)
}

/// The label a statement list carries, which is upstream's `begin` node.
const BEGIN: &str = "(begin)";

/// A syntax tree in the shape the comparison reads it: node kinds and the text of the leaves, with
/// the differences a redundant pair of parentheses is allowed to make normalized away.
#[derive(PartialEq, Eq)]
struct Sexp {
    label: String,
    children: Vec<Sexp>,
}

/// The kinds whose children upstream's parser folds into one `begin` node, which is where a
/// parenthesized sequence written inside another sequence loses its own node.
const SEQUENCES: &[&str] = &[
    "program",
    "parenthesized_statements",
    "then",
    "else",
    "body_statement",
    "block_body",
    "begin",
    "do",
];

/// `normalize_reparsed_ast`.
fn normalized(node: Node<'_>, text: &str, verification: Verification) -> Sexp {
    if let Some(words) = word_array(node, text) {
        return words;
    }
    if verification.fold_string_concatenation
        && let Some(literal) = string_literal(node, text, verification)
    {
        return literal;
    }
    let mut children: Vec<Sexp> = Vec::new();
    let mut dropped_parentheses = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        // Comments are invisible to the tree upstream compares.
        if child.kind_str() == "comment" {
            continue;
        }
        // So is the empty argument list of `foo()`, which upstream's parser never builds.
        if verification.fold_empty_call_parentheses && empty_argument_list(child) {
            dropped_parentheses = true;
            continue;
        }
        let normalized = normalized(child, text, verification);
        // `splice_nested_sequences`: `x; (a; b)` and `x; a; b` are the same statement list.
        match SEQUENCES.contains(&node.kind_str()) && normalized.label == BEGIN {
            true => children.extend(normalized.children),
            false => children.push(normalized),
        }
    }
    attach_heredoc_bodies(&mut children);
    // The parser has no node for the file itself: a source holding one statement parses to that
    // statement, which is what a fragment cut out of a scope has to compare against.
    if matches!(node.kind_str(), "parenthesized_statements" | "program") {
        // A `begin` holding one node is that node.
        if children.len() == 1 {
            return children.pop().expect("just measured as one");
        }
        return Sexp {
            label: BEGIN.to_owned(),
            children,
        };
    }
    if dropped_parentheses && let Some(bare) = written_without_parentheses(node, &mut children) {
        return bare;
    }
    if !verification.fold_string_concatenation {
        let label = label_of(node, text, children.is_empty());
        return rotate_same_operator(Sexp { label, children });
    }
    // The literals written side by side that upstream's parser gathers into one `dstr`.
    let label = match node.kind_str() {
        "chained_string" => DSTR.to_owned(),
        _ => label_of(node, text, children.is_empty()),
    };
    fold_string_concatenation(rotate_same_operator(Sexp { label, children }))
}

/// The two nodes tree-sitter spells a heredoc with.
const HEREDOC_OPENER: &str = "heredoc_beginning";
const HEREDOC_BODY: &str = "heredoc_body";

/// Whether a label names this kind, with or without the text a leaf carries.
fn labelled(label: &str, kind: &str) -> bool {
    label == kind
        || label
            .strip_prefix(kind)
            .is_some_and(|rest| rest.starts_with(' '))
}

/// Puts each heredoc body back under the token that opened it.
///
/// tree-sitter spells a heredoc as two nodes -- the opening token where the value is written, and
/// the body on the lines after the statement -- while upstream's parser builds a single `str` whose
/// body lives in the source map. That difference is invisible until something is written *between*
/// the two, and parentheses are exactly that: `(<<~X)` holds the opener and the body both, while
/// `<<~X` holds only the opener. The group therefore has two children before the correction and one
/// after, and a correction that is right to the byte gets rejected for changing the tree.
///
/// The bodies are written in the order their openers are, so they pair off in order. Anything left
/// over -- an opener that sits outside the subtree its body landed in -- goes back on the end
/// rather than being dropped, so its content stays in the comparison and a correction that rewrote
/// a body is still caught.
fn attach_heredoc_bodies(children: &mut Vec<Sexp>) {
    if !children
        .iter()
        .any(|child| labelled(&child.label, HEREDOC_BODY))
    {
        return;
    }
    let mut bodies = Vec::new();
    let mut index = 0;
    while index < children.len() {
        match labelled(&children[index].label, HEREDOC_BODY) {
            true => bodies.push(children.remove(index)),
            false => index += 1,
        }
    }
    let mut bodies = bodies.into_iter();
    adopt_heredoc_bodies(children, &mut bodies);
    children.extend(bodies);
}

/// Hands the bodies to the openers below `children`, in the order both are written.
fn adopt_heredoc_bodies(children: &mut [Sexp], bodies: &mut std::vec::IntoIter<Sexp>) {
    for child in children {
        if bodies.len() == 0 {
            return;
        }
        // An opener that already took one is a body attached further down, not a second chance.
        if labelled(&child.label, HEREDOC_OPENER) && child.children.is_empty() {
            child.children.extend(bodies.next());
            continue;
        }
        adopt_heredoc_bodies(&mut child.children, bodies);
    }
}

/// Whether the node is the `()` of `foo()`, which upstream's parser leaves no node behind for.
fn empty_argument_list(node: Node<'_>) -> bool {
    node.kind_str() == "argument_list" && node.named_child_count() == 0
}

/// The label a bare `yield` carries, which is a leaf holding nothing but the keyword.
const BARE_YIELD: &str = "yield yield";

/// What a call whose empty parentheses were just dropped reads as, when dropping them leaves the
/// spelling the source would have used without them.
///
/// `foo()` and `foo` reach upstream as one node, and so do `yield()` and `yield`. Here the first of
/// each pair is a parent with an `argument_list` under it and the second is written with no node in
/// between at all -- a lone `identifier`, a childless `yield` -- so the two only compare equal once
/// the first is put in the shape of the second. A receiver or a block means something is still
/// written around the name and the spellings stay apart.
fn written_without_parentheses(node: Node<'_>, children: &mut Vec<Sexp>) -> Option<Sexp> {
    match node.kind_str() {
        "yield" if children.is_empty() => Some(Sexp {
            label: BARE_YIELD.to_owned(),
            children: Vec::new(),
        }),
        "call"
            if children.len() == 1
                && node.field("receiver").is_none()
                && node.field("block").is_none() =>
        {
            children.pop()
        }
        _ => None,
    }
}

/// The kinds whose contents upstream's parser reads as whitespace-separated words, one node each.
const WORD_ARRAYS: &[&str] = &["string_array", "symbol_array"];

/// The words a `%w` or `%i` literal holds.
///
/// The grammar does not always lex them one node per word: a `%w[` written at the end of a line makes
/// the break part of the first word, so the same literal joined onto one line parses to a different
/// tree even though it holds the same words. Reading the words out of the source is what upstream's
/// parser does, and it is what lets the two compare equal. A backslash escapes whatever follows it,
/// so an escaped space or line break joins two words rather than separating them.
fn word_array(node: Node<'_>, text: &str) -> Option<Sexp> {
    if !WORD_ARRAYS.contains(&node.kind_str()) {
        return None;
    }
    // `%w[`, `%i(`, ...: two characters of prefix, an opening delimiter, and the closing one.
    let source = &text[node.byte_range()];
    let body = source.get(3..source.len().checked_sub(1)?)?;
    let mut words: Vec<Sexp> = Vec::new();
    let mut word = String::new();
    let mut characters = body.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            word.push(character);
            if let Some(escaped) = characters.next() {
                word.push(escaped);
            }
            continue;
        }
        if u8::try_from(character).is_ok_and(separates_words) {
            if !word.is_empty() {
                words.push(word_leaf(std::mem::take(&mut word)));
            }
            continue;
        }
        word.push(character);
    }
    if !word.is_empty() {
        words.push(word_leaf(word));
    }
    Some(Sexp {
        label: node.kind_str().to_owned(),
        children: words,
    })
}

/// Whether the byte ends a word of a `%w` or `%i` literal.
///
/// Ruby's lexer asks `ISSPACE`, which is the six ASCII spacing characters and nothing else -- a
/// no-break space is part of the word. `is_ascii_whitespace` is the same set minus the vertical
/// tab, so spelling the six out is what matches (measured: `%w[a\vb]` is two words).
///
/// The same six are what Ruby's `/\s/` matches, so a cop written against `\S` asks for the
/// negation of this. Every place that decides where one word of a percent literal ends comes
/// through here, because the same mistake was written into two of them independently and a fix
/// reached only one.
pub(crate) const fn separates_words(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn word_leaf(text: String) -> Sexp {
    Sexp {
        label: format!("word {text}"),
        children: Vec::new(),
    }
}

/// The label a merged string literal carries, which is upstream's `str` node.
const STR: &str = "str ";

/// The label a string built from several parts carries, which is upstream's `dstr` node.
const DSTR: &str = "dstr";

/// The label a `+` carries, which is the one concatenation upstream reads as a string.
const PLUS: &str = "binary +";

/// The `str` or `dstr` upstream's parser builds for a string literal: the value it holds, or the
/// parts an interpolation splits it into. The quotes are no part of either, which is what makes
/// `'a'` and `"a"` the same literal and lets a merged concatenation reach the same shape.
fn string_literal(node: Node<'_>, text: &str, verification: Verification) -> Option<Sexp> {
    if node.kind_str() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    let parts: Vec<Node<'_>> = node
        .named_children(&mut cursor)
        .filter(|child| child.kind_str() != "comment")
        .collect();
    if !parts.iter().any(|part| part.kind_str() == "interpolation") {
        return Some(str_leaf(
            parts
                .iter()
                .map(|part| &text[part.byte_range()])
                .collect::<String>(),
        ));
    }
    let mut children: Vec<Sexp> = Vec::new();
    let mut run = String::new();
    for part in parts {
        if part.kind_str() != "interpolation" {
            run.push_str(&text[part.byte_range()]);
            continue;
        }
        if !run.is_empty() {
            children.push(str_leaf(std::mem::take(&mut run)));
        }
        children.push(normalized(part, text, verification));
    }
    if !run.is_empty() {
        children.push(str_leaf(run));
    }
    Some(Sexp {
        label: DSTR.to_owned(),
        children,
    })
}

fn str_leaf(text: String) -> Sexp {
    Sexp {
        label: format!("{STR}{text}"),
        children: Vec::new(),
    }
}

/// `plain_string?`: a literal holding its value and nothing else, which is what two of them being
/// adjacent lets merge.
fn is_str(node: &Sexp) -> bool {
    node.label.starts_with(STR)
}

/// `stringish?`.
fn is_stringish(node: &Sexp) -> bool {
    is_str(node) || node.label == DSTR
}

/// `fold_string_concatenation`: every string concatenation in one canonical shape, so that a literal
/// split across lines and the same literal written whole compare equal.
fn fold_string_concatenation(node: Sexp) -> Sexp {
    let parts = match concatenation_parts(node) {
        Ok(parts) => parts,
        Err(node) => return node,
    };
    let mut merged = merge_string_parts(parts);
    match merged.as_slice() {
        [single] if is_str(single) => merged.pop().expect("just matched one"),
        _ => Sexp {
            label: DSTR.to_owned(),
            children: merged,
        },
    }
}

/// `string_concatenation_parts`, handing the node back when it is no concatenation of strings.
fn concatenation_parts(node: Sexp) -> Result<Vec<Sexp>, Sexp> {
    if node.label == DSTR || (node.label == PLUS && node.children.iter().all(is_stringish)) {
        return Ok(node.children);
    }
    Err(node)
}

/// `merge_string_parts`: a nested concatenation contributes its own parts, and a run of plain
/// literals becomes the one literal their values spell.
fn merge_string_parts(parts: Vec<Sexp>) -> Vec<Sexp> {
    let mut merged: Vec<Sexp> = Vec::new();
    for part in parts {
        let flattened = match part.label == DSTR {
            true => part.children,
            false => vec![part],
        };
        for part in flattened {
            match merged.last_mut() {
                Some(last) if is_str(last) && is_str(&part) => {
                    last.label.push_str(&part.label[STR.len()..]);
                }
                _ => merged.push(part),
            }
        }
    }
    merged
}

/// The name two trees are compared by: the node's kind, the operator it was written with, and for a
/// leaf the text it spans. `&&` and `and` are one type upstream, as are `||` and `or`.
fn label_of(node: Node<'_>, text: &str, leaf: bool) -> String {
    let mut label = node.kind_str().to_owned();
    if let Some(operator) = node.field("operator") {
        let spelling = match &text[operator.byte_range()] {
            "&&" | "and" => "and",
            "||" | "or" => "or",
            other => other,
        };
        label.push(' ');
        label.push_str(spelling);
    }
    if leaf {
        label.push(' ');
        label.push_str(&text[node.byte_range()]);
    }
    label
}

/// `rotate_same_operator`: `x && (y && z)` and `x && y && z` differ as trees, but neither operator
/// can be redefined, so the two say the same thing.
fn rotate_same_operator(node: Sexp) -> Sexp {
    if !node.label.ends_with(" and") && !node.label.ends_with(" or") {
        return node;
    }
    let Sexp {
        label,
        mut children,
    } = node;
    let [_, right] = children.as_mut_slice() else {
        return Sexp { label, children };
    };
    if right.label != label {
        return Sexp { label, children };
    }
    let right = children.pop().expect("just matched two children");
    let left = children.pop().expect("just matched two children");
    let mut inner = right.children.into_iter();
    let (Some(right_left), Some(right_right)) = (inner.next(), inner.next()) else {
        return Sexp {
            label: label.clone(),
            children: vec![left],
        };
    };
    let rotated = rotate_same_operator(Sexp {
        label: label.clone(),
        children: vec![left, right_left],
    });
    rotate_same_operator(Sexp {
        label,
        children: vec![rotated, right_right],
    })
}

/// `VERSION_SPECIFICATION_REGEX`, shared by the two cops that ask whether a dependency was pinned.
/// Ruby anchors `^` at the start of a *line*, which this engine only does under `(?m)`.
static VERSION_SPECIFICATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*[~<>=]*\s*[0-9.]+").expect("the version requirement pattern compiles")
});

/// The keys that pin a dependency to a commit rather than to a version.
const COMMIT_KEYS: &[&str] = &["branch", "ref", "tag"];

/// `<(str #version_specification?) ...>`: whether the argument is a string that opens with a version
/// requirement.
pub(crate) fn is_version_specification(argument: &Argument<'_>, context: &RuleContext<'_>) -> bool {
    let node = argument.first();
    argument.parts().len() == 1
        && is_string(node, context)
        && VERSION_SPECIFICATION.is_match(string_text(node, context))
}

/// `<(hash <(pair (sym {:branch :ref :tag}) (str _)) ...>) ...>`: whether the argument is a hash that
/// pins the dependency to a commit.
pub(crate) fn is_commit_reference(argument: &Argument<'_>, context: &RuleContext<'_>) -> bool {
    // A trailing run of `key: value` pairs is one `hash` argument upstream even though it was
    // written without braces, so both spellings have to be looked into.
    let pairs: Vec<Node<'_>> = match argument.first().kind_str() {
        "hash" if argument.parts().len() == 1 => {
            let mut cursor = argument.first().walk();
            argument.first().named_children(&mut cursor).collect()
        }
        _ => argument.parts().to_vec(),
    };
    pairs.iter().any(|pair| {
        pair.kind_str() == "pair"
            && pair_key_symbol(*pair, context).is_some_and(|key| COMMIT_KEYS.contains(&key))
            && pair
                .field("value")
                .is_some_and(|value| is_string(value, context))
    })
}

/// `File.expand_path`: RuboCop resolves every target against the working directory before it
/// inspects it, so a cop that reads the path of the file it is inspecting always sees an absolute
/// one.
pub(crate) fn expand_path(path: &std::path::Path) -> std::path::PathBuf {
    let absolute = match path.is_absolute() {
        true => path.to_path_buf(),
        false => std::env::current_dir().unwrap_or_default().join(path),
    };
    let mut expanded = std::path::PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                expanded.pop();
            }
            component => expanded.push(component),
        }
    }
    expanded
}

/// `range_by_whole_lines(range, include_final_newline: true)`: the lines `range` sits on, taken
/// whole, with the line break that closes the last of them.
pub(crate) fn whole_lines(range: Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    by_whole_lines(range, context, true)
}

/// `range_by_whole_lines(range)`: the same lines, stopping before the line break that closes them.
///
/// This is upstream's default. A caller that deletes the span wants [`whole_lines`] instead, or the
/// line it emptied stays behind as a blank one; a caller that only measures or replaces the text of
/// those lines wants this one, or it takes a character that is not on them.
pub(crate) fn whole_lines_without_terminator(
    range: Range<usize>,
    context: &RuleContext<'_>,
) -> Range<usize> {
    by_whole_lines(range, context, false)
}

/// `RangeHelp#range_by_whole_lines`, with upstream's keyword spelled out as an argument.
///
/// The last line's break is looked for rather than added, so a file that ends without one is not
/// reported as reaching past its own text.
fn by_whole_lines(
    range: Range<usize>,
    context: &RuleContext<'_>,
    include_final_newline: bool,
) -> Range<usize> {
    let text = context.source.text();
    let start = text[..range.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = text[range.end..].find('\n').map_or(text.len(), |offset| {
        range.end + offset + usize::from(include_final_newline)
    });
    start..end
}

/// Node kinds that hold a comma-separated list of expressions. Ruby's own parser closes such a
/// list at every comma, so `foo(a, b = 1)` passes two arguments and only `b` is assigned.
const COMMA_SEPARATED_LISTS: &[&str] = &[
    "argument_list",
    "array",
    "splat_argument",
    "optional_parameter",
    "keyword_parameter",
    "right_assignment_list",
];

/// Whether a `left_assignment_list` is one the grammar invented. tree-sitter parses `foo(a, b = 1)`
/// and `def m(x = A, y = 2)` as a multiple assignment that swallowed the items written before the
/// one being assigned to, which is not how Ruby reads them, so such a list is not a `masgn` and
/// binds only its last name.
///
/// Every walk that binds names -- the one the Lint cops run, the one the Metrics cops run and the
/// one the Naming cops run -- meets the same invented lists, so the reading lives here rather than
/// once per walk.
pub(crate) fn spurious_assignment_list(list: Node<'_>) -> bool {
    // A swallowed list runs on into the value, so `foo(a = 1, b = 2, c = 3)` nests one invented
    // assignment inside the next and only the outermost one stands in the list itself.
    let mut current = list.parent();
    while let Some(node) = current {
        let Some(parent) = node.parent() else {
            return false;
        };
        if COMMA_SEPARATED_LISTS.contains(&parent.kind_str()) {
            return true;
        }
        let continues = parent.kind_str() == "assignment"
            && parent
                .field("right")
                .is_some_and(|right| right.id() == node.id());
        current = continues.then_some(parent);
    }
    false
}

/// The scope a node opens, and the fields that still belong to the scope around it. RuboCop calls
/// these "twisted" nodes: `class Foo < bar` evaluates `bar` outside the class body it precedes, and
/// so do the receiver of `def obj.name` and the value of `class << expr`. The `bool` says whether
/// the scope is a block, which is the one kind that still sees the variables around it.
pub(crate) fn scope_kind(kind: &str) -> Option<(bool, &'static [&'static str])> {
    match kind {
        "method" => Some((false, &[])),
        "singleton_method" => Some((false, &["object"])),
        "class" => Some((false, &["name", "superclass"])),
        "module" => Some((false, &["name"])),
        "singleton_class" => Some((false, &["value"])),
        "block" | "do_block" | "lambda" => Some((true, &[])),
        _ => None,
    }
}

/// The constant path as written, joined with `::`.
///
/// A `scope` that cannot be read is dropped rather than failing the whole name, so `::Foo` answers
/// `::Foo`. The cops that must not accept a partial reading keep their own stricter version.
pub(crate) fn const_name(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let name = match node.kind_str() {
        "constant" => return Some(context.source.node_text(node).to_owned()),
        "scope_resolution" => context.source.node_text(node.field("name")?),
        _ => return None,
    };
    match node.field("scope") {
        Some(scope) => Some(format!(
            "{}::{name}",
            const_name(scope, context).unwrap_or_default()
        )),
        None => Some(name.to_owned()),
    }
}

/// Pushes `node`'s named children so that popping the stack yields them in
/// source order, making a `pop`-driven loop reproduce depth-first pre-order.
pub(crate) fn push_named_children<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let start = stack.len();
    let mut cursor = node.walk();
    stack.extend(node.named_children(&mut cursor));
    stack[start..].reverse();
}

pub(crate) fn walk_named(node: Node<'_>, callback: &mut impl FnMut(Node<'_>)) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        callback(current);
        push_named_children(current, &mut stack);
    }
}

/// Node kinds whose span is literal text. The code inside a `#{...}` is not, even though the
/// string around it is, so these are what re-cover an offset once an interpolation has uncovered
/// it.
const LITERAL_KINDS: &[&str] = &[
    "comment",
    "string",
    "symbol",
    "simple_symbol",
    "delimited_symbol",
    "heredoc_body",
    "regex",
    "subshell",
    "bare_string",
    "character",
];

/// The `#{...}` spans of the file, which are code even though the string around them is not.
pub(crate) struct Interpolations {
    spans: Vec<Range<usize>>,
    literals: Vec<Range<usize>>,
}

impl Interpolations {
    pub(crate) fn new(context: &RuleContext<'_>) -> Self {
        Self {
            spans: context
                .nodes_of("interpolation")
                .map(|node| node.byte_range())
                .collect(),
            literals: context
                .nodes_of_any(LITERAL_KINDS)
                .map(|node| node.byte_range())
                .collect(),
        }
    }

    /// Whether `offset` sits in interpolated code rather than in the text around it.
    ///
    /// A literal opened inside the interpolation covers it again, which is what keeps the `;` of
    /// `"#{x.sub(/;/, '')}"` out of the token stream.
    pub(crate) fn holds_code(&self, offset: usize) -> bool {
        let Some(innermost) = self
            .spans
            .iter()
            .filter(|span| span.contains(&offset))
            .map(|span| span.start)
            .max()
        else {
            return false;
        };
        !self
            .literals
            .iter()
            .any(|literal| literal.start > innermost && literal.contains(&offset))
    }
}

/// Node kinds upstream's parser writes as a container whose own value decides its children's,
/// which is the first arm of `Node#value_used?`.
const VALUE_CONTAINERS: &[&str] = &[
    "array",
    "string_array",
    "symbol_array",
    "hash",
    "pair",
    "string",
    "chained_string",
    "delimited_symbol",
    "subshell",
    "regex",
    "range",
    "when",
];

/// Node kinds that hold a list of statements, which upstream folds into one `begin` when there is
/// more than one. Only the last statement of such a list carries the list's value.
const STATEMENT_LISTS: &[&str] = &[
    "program",
    "then",
    "else",
    "body_statement",
    "block_body",
    "do",
    "begin",
    "parenthesized_statements",
    "ensure",
];

/// `RuboCop::AST::Node#value_used?`: whether anything reads what this expression evaluates to.
///
/// A cop asks this to tell `File.open(path)` written for its side effect from one whose result is
/// handed on. It is answered by walking up rather than down, because the same expression is used
/// or discarded depending only on where it was written.
pub(crate) fn value_used(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = context.parent(node) else {
        return false;
    };
    let kind = parent.kind_str();
    if VALUE_CONTAINERS.contains(&kind) || (kind == "unary" && is_logical_not(parent, context)) {
        return value_used(context, parent);
    }
    if STATEMENT_LISTS.contains(&kind) {
        // `begin_value_used?`.
        return last_statement(parent).is_some_and(|last| last.id() == node.id())
            && value_used(context, parent);
    }
    match kind {
        // `for_value_used?`: the variable and the collection are both read; the body is not,
        // unless the loop itself is.
        "for" => {
            parent
                .field("body")
                .is_none_or(|body| body.id() != node.id())
                || value_used(context, parent)
        }
        // `case_if_value_used?`: the condition is always read.
        "case" | "case_match" | "if" | "unless" | "elsif" | "conditional" => {
            is_field(parent, "condition", node)
                || is_field(parent, "value", node)
                || value_used(context, parent)
        }
        // `while_until_value_used?`: a loop always evaluates to `nil`, so only its condition is
        // read.
        "while" | "until" | "while_modifier" | "until_modifier" => {
            is_field(parent, "condition", node)
        }
        _ => true,
    }
}

fn is_field(parent: Node<'_>, name: &str, node: Node<'_>) -> bool {
    parent
        .field(name)
        .is_some_and(|child| child.id() == node.id())
}

/// `!x` and `not x`, which upstream's parser writes as a `not` node rather than a call.
fn is_logical_not(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.child(0)
        .is_some_and(|operator| matches!(context.source.node_text(operator), "!" | "not"))
}

/// The span upstream's parser gives the node.
///
/// A heredoc's body is spelled after the statement that opened it and a comment is no node at all
/// there, so neither is part of the expression: a branch whose last statement is followed by one
/// runs only as far as that statement. A node closed by a keyword of its own -- an `if`, a `def`
/// -- ends at the keyword either way, so this only ever differs for the ones that are not.
pub(crate) fn expression_range(node: Node<'_>) -> Range<usize> {
    node.start_byte()..expression_end(node)
}

fn expression_end(node: Node<'_>) -> usize {
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
    // A `;` between statements is a token here and nothing at all upstream, so an expression that
    // happens to be followed by one would otherwise reach a byte further than upstream's node
    // does. `if a; 1; elsif b; 2; end` reported the `elsif` branch one character too long.
    let Some(last) = children.into_iter().rfind(|child| {
        !matches!(
            child.kind_str(),
            "comment" | "heredoc_body" | ";" | "empty_statement"
        )
    }) else {
        return node.end_byte();
    };
    match last.is_named() {
        true => expression_end(last),
        false => last.end_byte(),
    }
}

/// The last statement of a statement list, skipping what the grammar parks there that upstream's
/// `begin` has no child for.
fn last_statement<'tree>(list: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = list.walk();
    let children: Vec<Node<'tree>> = list.named_children(&mut cursor).collect();
    children.into_iter().rfind(|child| {
        !matches!(
            child.kind_str(),
            // A `rescue`, `else` or `ensure` clause stands beside the statements it guards here,
            // where upstream wraps the statements in a node of its own. Reading the clause as the
            // last statement makes the value of the last real statement look unused.
            "comment" | "heredoc_body" | "rescue" | "else" | "ensure"
        )
    })
}

/// `ProcessedSource#contains_comment?`: whether any comment sits on one of the lines the range
/// touches.
///
/// The question is asked of *lines*, not of the span itself, so a trailing comment on the line the
/// range ends on counts even though it lies outside the range. `class Foo # note` and the `end` of
/// an otherwise empty body both answer yes for that reason.
pub(crate) fn contains_comment(context: &RuleContext<'_>, range: Range<usize>) -> bool {
    let first = context.source.line_column(range.start).0;
    let last = context
        .source
        .line_column(range.end.min(context.source.text().len()))
        .0;
    context.comment_ranges().iter().any(|comment| {
        let line = context.source.line_column(comment.start).0;
        line >= first && line <= last
    })
}
