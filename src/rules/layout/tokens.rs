//! `ProcessedSource#tokens`: the lexer's token stream, rebuilt from the syntax tree.
//!
//! Two cops read a file token by token rather than node by node -- `Layout/ExtraSpacing` measures
//! the gap between every pair of neighbours, and `Layout/BlockAlignment` counts the brackets open
//! at a position -- so the leaves of the tree have to be handed over in the order and with the
//! spans Ruby's lexer produced them in. Three details of that stream are load-bearing:
//!
//! * **A heredoc's body is lexed where its opener stands.** `foo(<<~A, bar)` yields the opener,
//!   the body and the terminator, and only then the comma that follows the opener on the first
//!   line. Emitting the body at its own position instead would make the opener and the comma
//!   neighbours and report the space between them.
//! * **A literal never has a gap inside it.** The lexer fills the space between two words of a
//!   `%w[]` with a `tSPACE` token, and covers a string's interior with content tokens, so no pair
//!   inside a literal is ever separated. The grammar leaves those gaps empty, so they are filled
//!   here.
//! * **Comments are tokens** (a `=begin` block being a single one), and the text after `__END__`
//!   is not tokenized at all.
//!
//! The `tNL` the lexer emits at the end of a statement is deliberately left out. It is always the
//! last token of its line, so a pair ending in one is skipped for being a newline and the pair
//! starting with one is skipped for spanning two lines -- exactly what dropping it achieves.

use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// What a token is, as far as the predicates `RuboCop::AST::Token` offers the cops reading this
/// stream go.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum TokenKind {
    /// `tCOMMENT`.
    Comment,
    /// `tLPAREN` and `tLPAREN2`, the two `Token#left_parens?` accepts. The `tLPAREN_ARG` that
    /// opens the sole argument of a parenthesis-less call is deliberately not one of them.
    LeftParenthesis,
    RightParenthesis,
    /// `tLBRACK`: the bracket that opens an array literal.
    LeftBracket,
    /// `tLBRACK2`: the bracket that opens an index read. `Token#left_bracket?` accepts it along
    /// with `tLBRACK`, but the two are separate types, and a cop reading for the operators that
    /// bind tighter than `+` wants only this one.
    IndexBracket,
    RightBracket,
    /// `tSTRING`: a quoted string the lexer produced whole, delimiters included.
    String,
    /// `tSTRING_BEG`: the opening delimiter of a literal the lexer had to spell out, because it
    /// interpolates or spans lines.
    StringBegin,
    /// `tSTRING_END`: its closing delimiter.
    StringEnd,
    /// `tPLUS`: the binary `+`, which is a different type from the unary `tUPLUS`.
    Plus,
    /// `tLSHFT`: `<<`, whether it appends or opens a singleton class.
    LeftShift,
    /// `tSTAR2`: the binary `*`, as opposed to the `tSTAR` that splats.
    Star,
    /// `tPERCENT`: the binary `%`. A percent literal's introducer belongs to the literal.
    Percent,
    /// `tDOT`. The `&.` of a safe navigation is `tANDDOT` and not one of these.
    Dot,
    Other,
}

pub(crate) struct Token {
    pub range: Range<usize>,
    /// 1-based line of the token's first character, `Token#line`.
    pub line: usize,
    pub kind: TokenKind,
}

impl Token {
    pub(crate) fn is_comment(&self) -> bool {
        self.kind == TokenKind::Comment
    }

    /// Whether the token opens a bracket `BlockAlignment#inside_parentheses?` counts.
    pub(crate) fn opens_bracket(&self) -> bool {
        matches!(
            self.kind,
            TokenKind::LeftParenthesis | TokenKind::LeftBracket | TokenKind::IndexBracket
        )
    }

    pub(crate) fn closes_bracket(&self) -> bool {
        matches!(
            self.kind,
            TokenKind::RightParenthesis | TokenKind::RightBracket
        )
    }
}

/// The node kinds that open a literal: everything below one of these is text rather than code,
/// apart from what an `interpolation` puts back.
const LITERALS: [&str; 9] = [
    "string",
    "bare_string",
    "bare_symbol",
    "delimited_symbol",
    "regex",
    "subshell",
    "heredoc_body",
    "string_array",
    "symbol_array",
];

pub(crate) fn tokens(context: &RuleContext<'_>) -> Vec<Token> {
    let bodies: Vec<Node<'_>> = context.nodes_of("heredoc_body").collect();
    let mut builder = Builder {
        context,
        bodies,
        openers: 0,
        tokens: Vec::new(),
    };
    builder.walk(context.root_node());
    builder.tokens
}

struct Builder<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    /// The heredoc bodies of the file in source order, which is the order their openers appear in.
    bodies: Vec<Node<'tree>>,
    /// How many heredoc openers have been reached, which indexes into `bodies`.
    openers: usize,
    tokens: Vec<Token>,
}

impl Builder<'_, '_> {
    fn walk(&mut self, node: Node<'_>) {
        match node.kind_str() {
            "heredoc_beginning" => {
                self.push(node);
                let index = self.openers;
                self.openers += 1;
                if let Some(body) = self.bodies.get(index).copied() {
                    // The grammar starts the body at the line break that ends the opener's line;
                    // the lexer leaves that break on the opener's line and starts the body's first
                    // content token below it. Keeping the break would put the body's first token
                    // on the opener's line and report the space before whatever follows the
                    // opener there.
                    let text = &self.context.source.text()[body.start_byte()..];
                    let skipped = text
                        .strip_prefix("\r\n")
                        .map_or_else(|| usize::from(text.starts_with('\n')), |_| 2);
                    self.walk_literal_from(body, body.start_byte() + skipped);
                }
            }
            // Already emitted, at the opener that introduced it.
            "heredoc_body" => {}
            // The text after `__END__`, which the lexer stops before.
            "uninterpreted" => {}
            // A label's colon belongs to the name it follows (`tLABEL`, or `tLABEL_END` when the
            // name was written as a string), and a setter's `=` to the method name.
            ":" if closes_a_label(node) => self.extend(node),
            "=" if node
                .parent_of(self.context)
                .is_some_and(|parent| parent.kind_str() == "setter") =>
            {
                self.extend(node);
            }
            // A quoted string that fits on one line and interpolates nothing is a single
            // `tSTRING`, delimiters included. Only its length distinguishes it from the
            // delimiter-and-content spelling, but the alignment check compares a token's whole
            // text against the neighbouring line, so the length is what it turns on.
            "string" if collapses(node) => {
                let kind = match self.quote_delimited(node) {
                    true => TokenKind::String,
                    false => TokenKind::Other,
                };
                self.emit(node.byte_range(), kind);
            }
            kind if LITERALS.contains(&kind) => self.walk_literal(node),
            _ => {
                if node.child_count() == 0 {
                    self.push(node);
                    return;
                }
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.walk(child);
                }
            }
        }
    }

    fn walk_literal(&mut self, node: Node<'_>) {
        self.walk_literal_from(node, node.start_byte());
    }

    /// The inside of a literal from `from` onwards: its delimiters and text are tokens of their
    /// own, the space between two of them is one as well, and an interpolation is code again.
    fn walk_literal_from(&mut self, node: Node<'_>, from: usize) {
        let mut offset = from;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.end_byte() <= from {
                continue;
            }
            let start = child.start_byte().max(from);
            self.fill(offset..start);
            match child.kind_str() {
                "interpolation" => self.walk(child),
                kind if LITERALS.contains(&kind) => self.walk_literal_from(child, start),
                // The text of a literal is one token however the grammar subdivided it: a `#` in a
                // heredoc body is a comment node here and part of the body to the lexer.
                _ => self.emit(start..child.end_byte(), delimiter_kind(node, child)),
            }
            offset = child.end_byte();
        }
        self.fill(offset..node.end_byte());
    }

    /// The lexer's `tSPACE`: the run of characters between two parts of a literal, which keeps the
    /// two from ever being neighbours.
    fn fill(&mut self, range: Range<usize>) {
        if range.start < range.end {
            self.emit(range, TokenKind::Other);
        }
    }

    fn push(&mut self, node: Node<'_>) {
        self.emit(node.byte_range(), classify(node));
    }

    /// Folds the node into the token before it, which is how the lexer spells a name and the
    /// punctuation that terminates it as one token.
    fn extend(&mut self, node: Node<'_>) {
        match self.tokens.last_mut() {
            Some(last) if last.range.end == node.start_byte() => last.range.end = node.end_byte(),
            _ => self.push(node),
        }
    }

    /// Whether the literal was written with an ordinary quote. The grammar aliases every opening
    /// delimiter to `"`, so only the source says whether a `string` is `'a'` or `%q(a)` -- and the
    /// lexer spells the second one out rather than producing a single `tSTRING`.
    fn quote_delimited(&self, node: Node<'_>) -> bool {
        node.child(0)
            .is_some_and(|open| matches!(self.context.source.node_text(open), "'" | "\""))
    }

    fn emit(&mut self, range: Range<usize>, kind: TokenKind) {
        let line = self.context.source.line_column(range.start).0;
        self.tokens.push(Token { range, line, kind });
    }
}

/// Whether the colon terminates a label -- `a: 1`, `def m(a: 1)`, `in {a: 1}` -- rather than
/// separating the branches of a ternary.
fn closes_a_label(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(
            parent.kind_str(),
            "pair" | "keyword_parameter" | "keyword_pattern"
        )
    })
}

/// Whether the string is the single `tSTRING` the lexer produces rather than a delimiter, its
/// content and a closing delimiter.
///
/// A percent literal always keeps the three, and so does a string spread over two lines. A string
/// written as a hash key keeps them too: its closing quote is folded into the label's colon
/// instead, which is what `tLABEL_END` is.
fn collapses(node: Node<'_>) -> bool {
    if node.start_position().row != node.end_position().row {
        return false;
    }
    if node
        .child(0)
        .is_none_or(|open| !matches!(open.kind_str(), "\"" | "'"))
    {
        return false;
    }
    let mut cursor = node.walk();
    if node
        .children(&mut cursor)
        .any(|child| child.kind_str() == "interpolation")
    {
        return false;
    }
    !is_label_key(node)
}

fn is_label_key(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| matches!(parent.kind_str(), "pair" | "keyword_pattern"))
        && node
            .next_sibling()
            .is_some_and(|next| next.kind_str() == ":" && next.start_byte() == node.end_byte())
}

/// Which of a literal's tokens the part is: the delimiter that opens it, the one that closes it,
/// or the text between them.
fn delimiter_kind(literal: Node<'_>, part: Node<'_>) -> TokenKind {
    if literal.kind_str() != "string" {
        return TokenKind::Other;
    }
    let last = u32::try_from(literal.child_count())
        .unwrap_or(0)
        .saturating_sub(1);
    if literal.child(0) == Some(part) {
        return TokenKind::StringBegin;
    }
    match literal.child(last) == Some(part) {
        true => TokenKind::StringEnd,
        false => TokenKind::Other,
    }
}

fn classify(node: Node<'_>) -> TokenKind {
    match node.kind_str() {
        "comment" => TokenKind::Comment,
        "(" if opens_a_command_argument(node) => TokenKind::Other,
        "(" => TokenKind::LeftParenthesis,
        ")" => TokenKind::RightParenthesis,
        "[" if node
            .parent()
            .is_some_and(|parent| parent.kind_str() == "element_reference") =>
        {
            TokenKind::IndexBracket
        }
        "[" => TokenKind::LeftBracket,
        "]" => TokenKind::RightBracket,
        "." => TokenKind::Dot,
        "<<" => TokenKind::LeftShift,
        "+" if is_binary_operator(node) => TokenKind::Plus,
        "*" if is_binary_operator(node) => TokenKind::Star,
        "%" if is_binary_operator(node) => TokenKind::Percent,
        _ => TokenKind::Other,
    }
}

/// Whether the operator stands between two operands. A `*` that splats and a `+` that signs a
/// number are types of their own to the lexer.
fn is_binary_operator(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind_str() == "binary")
}

/// Whether the parenthesis is the `tLPAREN_ARG` of `foo (1 + 2)`: the lexer keeps that one apart
/// from every other opening parenthesis because it is the ambiguous case, where the parentheses
/// group the first argument of a call written without an argument list of its own.
fn opens_a_command_argument(node: Node<'_>) -> bool {
    let Some(group) = node
        .parent()
        .filter(|parent| parent.kind_str() == "parenthesized_statements")
    else {
        return false;
    };
    group
        .parent()
        .filter(|list| list.kind_str() == "argument_list")
        .and_then(|list| list.child(0))
        .is_some_and(|first| first == group)
}
