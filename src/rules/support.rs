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
    newlines: bool,
    whitespace: bool,
) -> usize {
    let mut position = move_pos(text, position, forward, true, |byte| {
        matches!(byte, b' ' | b'\t')
    });
    position = move_pos(text, position, forward, newlines, |byte| byte == b'\n');
    move_pos(text, position, forward, whitespace, |byte| {
        byte.is_ascii_whitespace()
    })
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
    // `Parser::ClobberingError`: a rewrite whose parts collide is no correction to begin with.
    let Some(corrected) = apply_edits(context.source.text(), edits) else {
        return false;
    };
    parses(&corrected)
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
) -> Vec<T> {
    let text = context.source.text();
    let root = context.root_node();
    // `scope_groups`, keyed by node identity and ordered by the first item that reached each scope.
    let mut groups: Vec<(Option<Node<'_>>, Vec<T>)> = Vec::new();
    for item in items {
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
        if span > MAX_VERIFICATION_FRAGMENT_SIZE {
            verified.extend(group);
            continue;
        }
        let original = normalized(scope.unwrap_or(root), text);
        if group.len() > 1 && corrections_verify(text, scope, &original, &group, &edits_of) {
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
    parse(fragment).is_some_and(|tree| &normalized(tree.root_node(), fragment) == original)
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
fn normalized(node: Node<'_>, text: &str) -> Sexp {
    let mut children: Vec<Sexp> = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        // Comments are invisible to the tree upstream compares.
        if child.kind_str() == "comment" {
            continue;
        }
        let normalized = normalized(child, text);
        // `splice_nested_sequences`: `x; (a; b)` and `x; a; b` are the same statement list.
        match SEQUENCES.contains(&node.kind_str()) && normalized.label == BEGIN {
            true => children.extend(normalized.children),
            false => children.push(normalized),
        }
    }
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
    let label = label_of(node, text, children.is_empty());
    rotate_same_operator(Sexp { label, children })
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
    let text = context.source.text();
    let start = text[..range.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |offset| range.end + offset + 1);
    start..end
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
