//! A method call, and the literals it is written with, as RuboCop's `SendNode` presents them.
//!
//! tree-sitter records a call's arguments exactly as they were written, so `foo(a: 1, b: 2)` has
//! two argument nodes where RuboCop's parser has one `hash`, and `foo(1) { }` reaches the call
//! itself with the block already inside its span. A cop ported from a node pattern counts
//! arguments, reaches for "the last argument" and reports "the call" in upstream's terms, so it
//! has to see the call the way upstream does or it answers a different question than the pattern
//! it came from.

use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// One argument of a call.
///
/// Almost every argument is a single node. The exception is the brace-less hash that a trailing
/// run of `key: value` pairs and `**splat`s builds: upstream's parser folds those into one `hash`
/// argument, while tree-sitter leaves them as siblings of the arguments before them.
pub(crate) struct Argument<'tree> {
    parts: Vec<Node<'tree>>,
}

impl<'tree> Argument<'tree> {
    /// The node the argument begins with. For a brace-less hash that is its first pair rather than
    /// the `hash` upstream would hand out, which nothing can point at because it was never written.
    pub(crate) fn first(&self) -> Node<'tree> {
        self.parts[0]
    }

    /// Every node the argument was written as, which is more than one only for a brace-less hash.
    pub(crate) fn parts(&self) -> &[Node<'tree>] {
        &self.parts
    }

    pub(crate) fn range(&self) -> Range<usize> {
        self.parts[0].start_byte()..self.parts[self.parts.len() - 1].end_byte()
    }
}

/// A call's arguments, grouped the way `SendNode#arguments` returns them.
pub(crate) fn arguments<'tree>(call: Node<'tree>) -> Vec<Argument<'tree>> {
    let Some(list) = call.field("arguments") else {
        return Vec::new();
    };
    let mut cursor = list.walk();
    let mut arguments: Vec<Argument<'tree>> = Vec::new();
    let mut hash: Vec<Node<'tree>> = Vec::new();
    for node in list.named_children(&mut cursor) {
        // A comment written between two arguments is a child of the argument list here and no part
        // of the call at all upstream, where comments never reach the syntax tree.
        if node.kind_str() == "comment" {
            continue;
        }
        if matches!(node.kind_str(), "pair" | "hash_splat_argument") {
            hash.push(node);
            continue;
        }
        if !hash.is_empty() {
            arguments.push(Argument {
                parts: std::mem::take(&mut hash),
            });
        }
        arguments.push(Argument { parts: vec![node] });
    }
    if !hash.is_empty() {
        arguments.push(Argument { parts: hash });
    }
    arguments
}

/// The span of the call itself, without a block written after it. Upstream's `send` node ends where
/// its arguments do -- the block belongs to the `block` node wrapped around it -- so a cop that
/// reports "the call" has to stop there too.
pub(crate) fn send_range(call: Node<'_>, context: &RuleContext<'_>) -> Range<usize> {
    let Some(block) = call.field("block") else {
        return call.byte_range();
    };
    let text = context.source.text().as_bytes();
    let mut end = block.start_byte();
    while end > call.start_byte() && text[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    call.start_byte()..end
}

/// Whether the call is a `send` rather than a `csend`. `foo&.bar` is a node type of its own
/// upstream, so a pattern written for `send` never matches one.
pub(crate) fn is_plain_send(call: Node<'_>, context: &RuleContext<'_>) -> bool {
    call.field("operator")
        .is_none_or(|operator| context.source.node_text(operator) != "&.")
}

/// Whether `node` is the constant `name` reached from the top level, which is how a node pattern
/// spells `(const {nil? cbase} :Name)`. A constant reached through any other scope -- `Foo::Marshal`
/// -- is a different constant and never matches.
pub(crate) fn top_level_constant(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "constant" => context.source.node_text(node) == name,
        // `::Name`, but not a `Foo::Name` that merely ends in the name.
        "scope_resolution" => {
            node.field("scope").is_none()
                && node
                    .field("name")
                    .is_some_and(|inner| context.source.node_text(inner) == name)
        }
        _ => false,
    }
}

/// Whether `node` is what upstream's parser calls a `str`: a string literal with nothing
/// interpolated into it. A literal that interpolates is a `dstr` there, and so are the adjacent
/// literals of `"a" "b"`, which tree-sitter keeps as a `chained_string`.
pub(crate) fn is_string(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        // `?a` is a one-character string literal upstream, not a type of its own.
        "character" => true,
        "string" => !has_interpolation(node),
        // `emit_file_line_as_literals`: upstream's parser resolves `__FILE__` while it parses, so
        // a cop never sees the keyword -- only the string holding the path it stood for.
        "identifier" => context.source.node_text(node) == FILE_KEYWORD,
        _ => false,
    }
}

/// The keyword upstream's parser replaces with the path of the file being inspected.
pub(crate) const FILE_KEYWORD: &str = "__FILE__";

/// The text a string literal holds, without its delimiters. Escape sequences are left as written:
/// nothing a cop asks of a gem name or a URL can tell `'a\tb'` from what it decodes to.
pub(crate) fn string_text<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    if node.kind_str() == "character" {
        return &context.source.node_text(node)[1..];
    }
    if node.kind_str() == "identifier" {
        return context.source.path().to_str().unwrap_or_default();
    }
    let text = context.source.node_text(node);
    let (Some(open), Some(close)) = (
        node.child(0),
        node.child(node.child_count().saturating_sub(1) as u32),
    ) else {
        return text;
    };
    if open.id() == close.id() || close.end_byte() < open.end_byte() {
        return text;
    }
    context
        .source
        .slice(open.end_byte()..close.start_byte().max(open.end_byte()))
}

/// The name a symbol literal spells, or `None` when the node is not one.
pub(crate) fn symbol_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind_str() {
        "simple_symbol" => Some(&context.source.node_text(node)[1..]),
        "hash_key_symbol" => Some(context.source.node_text(node)),
        "delimited_symbol" if !has_interpolation(node) => Some(string_text(node, context)),
        _ => None,
    }
}

/// The symbol a hash pair is keyed by, or `None` when its key is anything else.
///
/// `"name": value` keys the pair by a symbol while `"name" => value` keys it by a string, and the
/// two are told apart only by the separator: tree-sitter writes both keys as a `string` node where
/// upstream's parser has already resolved one into a `sym`.
pub(crate) fn pair_key_symbol<'a>(pair: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    let key = pair.field("key")?;
    if let Some(name) = symbol_name(key, context) {
        return Some(name);
    }
    let colon_separated = pair
        .child(1)
        .is_some_and(|separator| context.source.node_text(separator) == ":");
    (key.kind_str() == "string" && !has_interpolation(key) && colon_separated)
        .then(|| string_text(key, context))
}

/// Whether a literal has an interpolation in it, which is what makes upstream's parser build a
/// `dstr`/`dsym` rather than a `str`/`sym`.
pub(crate) fn has_interpolation(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind_str() == "interpolation")
}

/// The `heredoc_body` opened by `beginning`. Bodies appear after the statement in the same order as
/// their openers, so the nth opener owns the nth body.
pub(crate) fn heredoc_body<'a>(
    beginning: Node<'_>,
    context: &'a RuleContext<'_>,
) -> Option<Node<'a>> {
    let position = context
        .nodes_of("heredoc_beginning")
        .position(|node| node.id() == beginning.id())?;
    context.nodes_of("heredoc_body").nth(position)
}

/// What makes two literals the same literal. Upstream compares the nodes themselves, and two
/// literal nodes are equal when they hold equal values, so `'k'` and `"k"` are one and the same
/// hash key.
pub(crate) fn literal_key(node: Node<'_>, context: &RuleContext<'_>) -> String {
    if let Some(name) = symbol_name(node, context) {
        return format!("sym:{name}");
    }
    match node.kind_str() {
        "string" | "bare_string" | "character" if !has_interpolation(node) => {
            format!("str:{}", string_text(node, context))
        }
        "identifier" if context.source.node_text(node) == FILE_KEYWORD => {
            format!("str:{}", string_text(node, context))
        }
        kind => format!("{kind}:{}", context.source.node_text(node)),
    }
}

/// The range that `source_range(buffer, node.first_line, node.loc.column...node.loc.last_column)`
/// produces: from where the node starts through its *last* column, taken on its **first** line.
///
/// Upstream builds it that way in three cops that report a repeated declaration, and it is only
/// the node itself while the node stays on one line. Spread over several, the range ends wherever
/// the closing line happened to end -- which is shorter than the node, and empty when the closing
/// line ends to the left of where the node began.
pub(crate) fn first_line_range(range: Range<usize>, context: &RuleContext<'_>) -> Range<usize> {
    let (_, column) = context.source.line_column(range.start);
    let (_, last_column) = context.source.line_column(range.end);
    let length = last_column.saturating_sub(column);
    let end = context.source.text()[range.start..]
        .char_indices()
        .nth(length)
        .map_or(context.source.len(), |(offset, _)| range.start + offset);
    range.start..end
}

/// The named children of `node`, collected so the cursor does not outlive the borrow.
pub(crate) fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// Whether any node in `node`'s subtree, including `node` itself, satisfies `predicate`. This is
/// the search a node pattern writes with a leading backtick.
pub(crate) fn any_descendant(node: Node<'_>, predicate: &mut impl FnMut(Node<'_>) -> bool) -> bool {
    if predicate(node) {
        return true;
    }
    named_children(node)
        .into_iter()
        .any(|child| any_descendant(child, predicate))
}
