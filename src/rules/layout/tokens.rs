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

/// What a token is, as far as the predicates `RuboCop::AST::Token` offers the cops reading this
/// stream go.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum TokenKind {
    /// `tCOMMENT`.
    Comment,
    /// `tLPAREN` and `tLPAREN2`, the two `Token#left_parens?` accepts. The `tLPAREN_ARG` that
    /// opens the sole argument of a parenthesis-less call is deliberately not one of them.
    LeftParenthesis,
    RightParenthesis,
    /// `tLBRACK` and `tLBRACK2`, both of which `Token#left_bracket?` accepts.
    LeftBracket,
    RightBracket,
    Other,
}

pub(super) struct Token {
    pub range: Range<usize>,
    /// 1-based line of the token's first character, `Token#line`.
    pub line: usize,
    pub kind: TokenKind,
}

impl Token {
    pub(super) fn is_comment(&self) -> bool {
        self.kind == TokenKind::Comment
    }

    /// Whether the token opens a bracket `BlockAlignment#inside_parentheses?` counts.
    pub(super) fn opens_bracket(&self) -> bool {
        matches!(
            self.kind,
            TokenKind::LeftParenthesis | TokenKind::LeftBracket
        )
    }

    pub(super) fn closes_bracket(&self) -> bool {
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

pub(super) fn tokens(context: &RuleContext<'_>) -> Vec<Token> {
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
        match node.kind() {
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
                .parent()
                .is_some_and(|parent| parent.kind() == "setter") =>
            {
                self.extend(node);
            }
            // A quoted string that fits on one line and interpolates nothing is a single
            // `tSTRING`, delimiters included. Only its length distinguishes it from the
            // delimiter-and-content spelling, but the alignment check compares a token's whole
            // text against the neighbouring line, so the length is what it turns on.
            "string" if collapses(node) => self.emit(node.byte_range(), TokenKind::Other),
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
            match child.kind() {
                "interpolation" => self.walk(child),
                kind if LITERALS.contains(&kind) => self.walk_literal_from(child, start),
                // The text of a literal is one token however the grammar subdivided it: a `#` in a
                // heredoc body is a comment node here and part of the body to the lexer.
                _ => self.emit(start..child.end_byte(), TokenKind::Other),
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
            parent.kind(),
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
        .is_none_or(|open| !matches!(open.kind(), "\"" | "'"))
    {
        return false;
    }
    let mut cursor = node.walk();
    if node
        .children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
    {
        return false;
    }
    !is_label_key(node)
}

fn is_label_key(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| matches!(parent.kind(), "pair" | "keyword_pattern"))
        && node
            .next_sibling()
            .is_some_and(|next| next.kind() == ":" && next.start_byte() == node.end_byte())
}

fn classify(node: Node<'_>) -> TokenKind {
    match node.kind() {
        "comment" => TokenKind::Comment,
        "(" if opens_a_command_argument(node) => TokenKind::Other,
        "(" => TokenKind::LeftParenthesis,
        ")" => TokenKind::RightParenthesis,
        "[" => TokenKind::LeftBracket,
        "]" => TokenKind::RightBracket,
        _ => TokenKind::Other,
    }
}

/// Whether the parenthesis is the `tLPAREN_ARG` of `foo (1 + 2)`: the lexer keeps that one apart
/// from every other opening parenthesis because it is the ambiguous case, where the parentheses
/// group the first argument of a call written without an argument list of its own.
fn opens_a_command_argument(node: Node<'_>) -> bool {
    let Some(group) = node
        .parent()
        .filter(|parent| parent.kind() == "parenthesized_statements")
    else {
        return false;
    };
    group
        .parent()
        .filter(|list| list.kind() == "argument_list")
        .and_then(|list| list.child(0))
        .is_some_and(|first| first == group)
}
