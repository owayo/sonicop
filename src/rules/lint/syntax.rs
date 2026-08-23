use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// The first Ruby version whose grammar accepts each construct.
///
/// RuboCop runs the `parser` gem pinned to `TargetRubyVersion`, so a construct newer than the
/// target is a syntax error there. The tree-sitter grammar only knows the latest Ruby and accepts
/// all of them, which is why every one of these has to be gated by hand.
const BEGINLESS_RANGE_SINCE: RubyVersion = RubyVersion::new(2, 7);
const ARGUMENT_FORWARDING_SINCE: RubyVersion = RubyVersion::new(2, 7);
const PATTERN_MATCHING_SINCE: RubyVersion = RubyVersion::new(2, 7);
const ONE_LINE_PATTERN_MATCH_SINCE: RubyVersion = RubyVersion::new(2, 7);
const KEYWORD_ARGUMENT_REJECTION_SINCE: RubyVersion = RubyVersion::new(2, 7);
const ENDLESS_METHOD_SINCE: RubyVersion = RubyVersion::new(3, 0);
const RIGHTWARD_ASSIGNMENT_SINCE: RubyVersion = RubyVersion::new(3, 0);
const FIND_PATTERN_SINCE: RubyVersion = RubyVersion::new(3, 0);
const HASH_VALUE_OMISSION_SINCE: RubyVersion = RubyVersion::new(3, 1);
const EXPRESSION_PIN_SINCE: RubyVersion = RubyVersion::new(3, 1);
const ANONYMOUS_BLOCK_FORWARDING_SINCE: RubyVersion = RubyVersion::new(3, 1);
const ANONYMOUS_REST_FORWARDING_SINCE: RubyVersion = RubyVersion::new(3, 2);
const COMMAND_ARGUMENT_STATEMENTS_SINCE: RubyVersion = RubyVersion::new(3, 3);

/// The nodes an error inside a hash or an argument list is recovered out of. After the parser
/// reports one omitted value it discards the rest of the surrounding literal, so climbing to the
/// outermost of these gives the region whose later omissions never reach a diagnostic.
const OMISSION_RECOVERY_KINDS: &[&str] = &["argument_list", "hash", "array", "pair"];

/// Tokens that open a region the parser has to see closed. An `ERROR` node holding an unmatched
/// one of these means the parser ran out of input rather than tripping over a token.
const OPENING_TOKENS: &[&str] = &[
    "(", "[", "{", "class", "module", "def", "do", "begin", "if", "unless", "while", "until",
    "case", "for",
];
const CLOSING_TOKENS: &[&str] = &[")", "]", "}", "end"];

/// The nodes that hold a sequence of statements. The statement an error was found in is the
/// ancestor whose own parent is one of these.
const BODY_KINDS: &[&str] = &[
    "program",
    "body_statement",
    "then",
    "else",
    "ensure",
    "block_body",
    "begin_block",
    "end_block",
    "parenthesized_statements",
];

/// One `Lint/Syntax` finding: the parser's own wording and the range it blames.
struct Diagnostic {
    reason: String,
    range: Range<usize>,
}

/// A source holding a NUL byte reaches every cop already rewritten the way Ruby's lexer reads one
/// (see `crate::nul_bytes`), so there is nothing left to account for here.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let target = context.target_ruby_version();
    let mut diagnostics = Vec::new();
    if context.root_node().has_error() {
        parse_errors(context, &mut diagnostics);
    }
    version_gated_syntax(context, target, &mut diagnostics);
    diagnostics.sort_by(|left, right| {
        (left.range.start, left.range.end).cmp(&(right.range.start, right.range.end))
    });
    diagnostics.dedup_by(|left, right| left.range == right.range);
    for diagnostic in diagnostics {
        offenses
            .push(context.offense(syntax_message(&diagnostic.reason, target), diagnostic.range));
    }
}

/// The errors the tree-sitter grammar itself rejected the file for.
fn parse_errors(context: &RuleContext<'_>, out: &mut Vec<Diagnostic>) {
    // A heredoc opened inside an interpolation throws the grammar off for the rest of the file --
    // the unterminated body it then reports has nothing to do with what the source says. Nothing
    // the scan finds here can be trusted, so none of it is reported.
    if opens_heredoc_in_interpolation(context) {
        return;
    }
    let mut ran_out_of_input = false;
    for node in context.nodes() {
        let nested_error = node
            .parent_of(context)
            .is_some_and(|parent| parent.is_error());
        if (!node.is_error() && !node.is_missing()) || nested_error {
            continue;
        }
        // A missing token is a construct the file never closed, which the parser only notices once
        // it reaches the end of the input.
        if node.is_missing() || opens_without_closing(node) {
            ran_out_of_input = true;
            continue;
        }
        if grammar_gap(node, context) {
            continue;
        }
        let (reason, range) = match offending_token(node) {
            Some(token) => (
                format!("unexpected token {}", token_name(token)),
                token.byte_range(),
            ),
            None => (
                "unexpected token".to_owned(),
                node.start_byte()
                    ..node
                        .end_byte()
                        .max(node.start_byte() + usize::from(!context.source.is_empty())),
            ),
        };
        out.push(Diagnostic { reason, range });
    }
    if ran_out_of_input {
        let (reason, range) = end_of_input(context);
        out.push(Diagnostic { reason, range });
    }
}

/// The token an `ERROR` node is blamed on.
///
/// The parser stops at the first token that cannot continue the parse. A closing delimiter can
/// never start one, so it is that token as soon as one appears; otherwise everything up to the
/// last token of the region was still a valid prefix and the last one is what broke it.
fn offending_token(error: Node<'_>) -> Option<Node<'_>> {
    let children = direct_children(error);
    children
        .iter()
        .find(|child| CLOSING_TOKENS.contains(&child.kind_str()) && !child.is_named())
        .or_else(|| children.last())
        .copied()
}

/// Whether an `ERROR` node marks Ruby the grammar cannot read rather than Ruby the parser rejects.
///
/// `Lint/Syntax` reports what `parser` reports, and a file the real thing reads without complaint
/// must draw no offense here -- reporting one would be a false positive on valid code, and would
/// also stop every other cop from running on the file. Both shapes below were found by comparing
/// against upstream; each is a construct `parser` accepts and tree-sitter's Ruby grammar does not.
fn grammar_gap(error: Node<'_>, context: &RuleContext<'_>) -> bool {
    multiline_array_pattern(error, context)
}

/// `in bar,\n   baz then …`: an array pattern continued on the next line. The grammar closes the
/// pattern at the comma -- reading it as a nameless splat -- and everything after the line break
/// lands in an `ERROR` inside the clause's body.
fn multiline_array_pattern(error: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = Some(error);
    while let Some(node) = current {
        if node.kind_str() == "in_clause" {
            return node.field("pattern").is_some_and(|pattern| {
                pattern.kind_str() == "array_pattern"
                    && context.source.node_text(pattern).trim_end().ends_with(',')
            });
        }
        current = node.parent();
    }
    false
}

/// `"…#{ <<~INNER … }"`: a heredoc opened inside an interpolation. Ruby puts the body after the
/// line the interpolation is written on, which the grammar has no way to express -- it looks for
/// the body where the interpolation still is, and never finds the terminator.
fn opens_heredoc_in_interpolation(context: &RuleContext<'_>) -> bool {
    context.nodes_of("interpolation").any(|interpolation| {
        let mut cursor = interpolation.walk();
        interpolation
            .named_children(&mut cursor)
            .any(|child| child.kind_str() == "heredoc_beginning")
    })
}

/// Whether an `ERROR` node leaves a region open, which the parser reports at the end of the input
/// rather than at a token.
fn opens_without_closing(error: Node<'_>) -> bool {
    let mut depth = 0i32;
    for child in direct_children(error) {
        if child.is_named() && child.child_count() > 0 {
            continue;
        }
        if OPENING_TOKENS.contains(&child.kind_str()) {
            depth += 1;
        } else if CLOSING_TOKENS.contains(&child.kind_str()) {
            depth -= 1;
        }
    }
    depth > 0
}

fn version_gated_syntax(context: &RuleContext<'_>, target: RubyVersion, out: &mut Vec<Diagnostic>) {
    // The parser blames tokens in the order it reads them, and one error takes the rest of the
    // construct it was found in with it, so the gates have to be weighed in the order of the
    // tokens they blame rather than the order their nodes are walked in: a nested pattern reaches
    // its offending token before the pattern holding it reaches its own.
    let mut gates: Vec<(Node<'_>, Gate)> = context
        .nodes()
        .filter_map(|node| feature_use(node, context).map(|gate| (node, gate)))
        .filter(|(_, gate)| target < gate.since)
        .collect();
    gates.sort_by_key(|(_, gate)| gate.range.start);

    // One omitted hash value takes the rest of its literal down with it: the parser discards
    // tokens until it can resume, which is past the end of the construct the error was found in.
    // This is the end of the region whose later omissions upstream therefore never reports.
    let mut recovered_through = 0;
    let mut resumed_at = None;
    let mut lost_its_definition = false;
    let mut abandoned = false;
    for (node, gate) in gates {
        if abandoned {
            continue;
        }
        if let Some(region) = gate.recovery {
            if node.start_byte() < recovered_through {
                continue;
            }
            recovered_through = region;
        }
        if gate.abandons_file {
            // The parser reads nothing more, so it never reaches the end of the input either.
            abandoned = true;
        } else if gate.in_method_body {
            // The parser never gets the definition it was in back, so only the first such error
            // decides how the rest of the file is read.
            if !lost_its_definition {
                lost_its_definition = true;
                method_body_recovery(node, context, out);
            }
        } else if gate.first_only {
            // The rest of what upstream reports here is a known divergence, so nothing about how
            // the file resumes is claimed either.
        } else if let Some(statement) = statement_closing_its_body(node) {
            resumed_at = Some(statement.end_byte().max(resumed_at.unwrap_or(0)));
        }
        let endless_in_block = gate.endless_in_block;
        out.push(Diagnostic {
            reason: gate.reason,
            range: gate.range,
        });
        if gate.legacy_forwarding {
            legacy_forwarding_recovery(node, context, out);
        }
        if endless_in_block {
            endless_in_block_recovery(node, context, out);
        }
    }

    // Recovering out of the last statement of a body takes the `end` that closed it as well, which
    // leaves the file a keyword short and the parser looking for more input at the end. It only
    // gets that far when real code follows: `end` keywords alone are consumed by the same recovery
    // and never bring the parser back to reporting.
    if let Some(resumed_at) = resumed_at
        && has_code_after(context, resumed_at)
    {
        let (reason, range) = end_of_input(context);
        out.push(Diagnostic { reason, range });
    }
}

/// The statement holding `node`: the ancestor whose own parent holds a sequence of statements.
fn enclosing_statement(node: Node<'_>) -> Option<Node<'_>> {
    let mut statement = node;
    while let Some(parent) = statement.parent() {
        if BODY_KINDS.contains(&parent.kind_str()) {
            return Some(statement);
        }
        statement = parent;
    }
    None
}

/// The statement holding `node` when it is the last one of a body that a keyword closes, which is
/// what makes the recovery consume that keyword.
fn statement_closing_its_body(node: Node<'_>) -> Option<Node<'_>> {
    let statement = enclosing_statement(node)?;
    let body = statement.parent()?;
    let closed = body.parent().is_some_and(|owner| {
        direct_children(owner)
            .iter()
            .any(|child| child.kind_str() == "end")
    });
    let last = significant_named_children(body)
        .last()
        .is_some_and(|last| last.id() == statement.id());
    (closed && last).then_some(statement)
}

/// Whether anything but block terminators and comments is left after `offset`.
fn has_code_after(context: &RuleContext<'_>, offset: usize) -> bool {
    context.nodes().any(|node| {
        node.child_count() == 0
            && node.start_byte() >= offset
            && !matches!(node.kind_str(), "comment" | "end")
    })
}

/// A use of syntax that only some Ruby versions accept.
struct Gate {
    since: RubyVersion,
    reason: String,
    range: Range<usize>,
    /// The end of the region the parser discards after reporting this use, set when reporting it
    /// makes the parser give up on the construct around it. Uses starting inside that region are
    /// never reached.
    recovery: Option<usize>,
    /// Set when the parser loses the method definition it was in, which changes how it reads the
    /// rest of the file.
    in_method_body: bool,
    /// Set when the parser has nothing left to read the rest of the file against, so that neither
    /// a later use nor the end of the input is ever reached.
    abandons_file: bool,
    legacy_forwarding: bool,
    /// Set for an endless definition written inside a block. The parser keeps `def name(args)` and
    /// discards the `= body`, which leaves the block's `}` with nothing to close and the file a
    /// token short at the end.
    endless_in_block: bool,
    /// Set when upstream reports **more** after this one and the shape of those further
    /// diagnostics is not modelled here. Only this one is emitted; see `first_only()`.
    first_only: bool,
}

impl Gate {
    fn new(since: RubyVersion, reason: String, range: Range<usize>) -> Self {
        Self {
            since,
            reason,
            range,
            recovery: None,
            in_method_body: false,
            abandons_file: false,
            legacy_forwarding: false,
            endless_in_block: false,
            first_only: false,
        }
    }

    fn recovers_through(mut self, end: usize) -> Self {
        self.recovery = Some(end);
        self
    }

    /// See [`Gate::endless_in_block`].
    fn endless_in_block(mut self) -> Self {
        self.endless_in_block = true;
        self
    }

    fn abandons_file(mut self) -> Self {
        self.abandons_file = true;
        self.recovers_through(usize::MAX)
    }

    fn in_method_body(mut self) -> Self {
        self.in_method_body = true;
        self
    }

    fn legacy_forwarding(mut self) -> Self {
        self.legacy_forwarding = true;
        self
    }

    /// **Report the first diagnostic and stop.**
    ///
    /// Upstream reports one or two more after this one, at a position that depends on what the
    /// `case` was written inside of: its own `end` at the top level, the enclosing `def`'s or
    /// `begin`'s `end` when nested (the `case`'s own `end` and any statement between are eaten),
    /// the **outermost** `end` when nested twice -- and inside a method an `else` becomes
    /// `else without rescue is useless`, which is not an unexpected-token diagnostic at all.
    /// Reproducing that is reproducing the parser's state machine.
    ///
    /// **The line drawn here is the one drawn for Homebrew**: in a file both sides call a syntax
    /// error, the diagnostics after the first are not chased (`#57`). The first one is measured and
    /// exact, and it is what makes the file unparsable so that no other cop runs on it -- which is
    /// the whole of what was missing.
    /// The caller pairs this with `recovers_through` over the construct the error was found in:
    /// a second `case` further down the file **is** reported (measured), so the region skipped has
    /// to stop at the first one's `end`.
    fn first_only(mut self) -> Self {
        self.first_only = true;
        self
    }
}

fn feature_use(node: Node<'_>, context: &RuleContext<'_>) -> Option<Gate> {
    match node.kind_str() {
        "range" if node.field("begin").is_none() => {
            let text = context.source.node_text(node);
            let (width, token) = if text.starts_with("...") {
                (3, "tDOT3")
            } else if text.starts_with("..") {
                (2, "tDOT2")
            } else {
                return None;
            };
            Some(Gate::new(
                BEGINLESS_RANGE_SINCE,
                format!("unexpected token {token}"),
                node.start_byte()..node.start_byte() + width,
            ))
        }
        "forward_parameter" => Some(
            Gate::new(
                ARGUMENT_FORWARDING_SINCE,
                "unexpected token tDOT3".to_owned(),
                node.byte_range(),
            )
            .legacy_forwarding(),
        ),
        "forward_argument" => Some(Gate::new(
            ARGUMENT_FORWARDING_SINCE,
            "unexpected token tDOT3".to_owned(),
            node.byte_range(),
        )),
        // `def name = body` has no `end`, so the assignment sits directly under the definition.
        // A setter's `=` belongs to its `setter` name node and a default argument's to the
        // parameter, so neither is mistaken for one.
        "method" | "singleton_method" => {
            let equals = direct_children(node)
                .into_iter()
                .find(|child| !child.is_named() && child.kind_str() == "=")?;
            let gate = Gate::new(
                ENDLESS_METHOD_SINCE,
                "unexpected token tEQL".to_owned(),
                equals.byte_range(),
            );
            Some(match enclosing_brace_block(node).is_some() {
                true => gate.endless_in_block(),
                false => gate,
            })
        }
        // `case a / in Integer / ... / end`. Upstream reports every clause keyword and then a
        // surplus `end` whose position depends on the nesting, so only the first is claimed --
        // see `Gate::first_only`. A second `case` further down the file is reported on its own.
        "case_match" => {
            let clause = direct_children(node)
                .into_iter()
                .find(|child| child.kind_str() == "in_clause")?;
            let keyword = direct_children(clause)
                .into_iter()
                .find(|child| !child.is_named() && child.kind_str() == "in")?;
            Some(
                Gate::new(
                    PATTERN_MATCHING_SINCE,
                    "unexpected token kIN".to_owned(),
                    keyword.byte_range(),
                )
                .first_only()
                .recovers_through(node.end_byte()),
            )
        }
        // `in ^(1 + 1)`. Pinning a **variable** has been allowed since pattern matching itself;
        // 3.1 is when the pin was allowed to hold an expression, and the parser blames the `(`
        // rather than the `^`.
        "expression_reference_pattern" => {
            let paren = direct_children(node)
                .into_iter()
                .find(|child| !child.is_named() && child.kind_str() == "(")?;
            let owner = ancestor_matching(node, |ancestor| ancestor.kind_str() == "case_match");
            let gate = Gate::new(
                EXPRESSION_PIN_SINCE,
                "unexpected token tLPAREN".to_owned(),
                paren.byte_range(),
            );
            Some(match owner {
                // A one-line `a in ^(1 + 1)` is the whole of what upstream reports, so nothing is
                // withheld there and the statement is where the parser resumes.
                None => gate.recovers_through(enclosing_statement(node).unwrap_or(node).end_byte()),
                Some(case) => gate.first_only().recovers_through(case.end_byte()),
            })
        }
        // `42 in Integer`. The parser stops at the keyword and picks the next statement up after
        // the one it was in, so this reports once and nothing else changes -- measured with a
        // following statement both at the top level and inside a method body.
        "test_pattern" => {
            let keyword = direct_children(node)
                .into_iter()
                .find(|child| !child.is_named() && child.kind_str() == "in")?;
            Some(Gate::new(
                ONE_LINE_PATTERN_MATCH_SINCE,
                "unexpected token kIN".to_owned(),
                keyword.byte_range(),
            ))
        }
        // `def a(**nil)`. The parser blames the `nil`, not the `**` in front of it, and the
        // definition survives: two of them in one file report twice.
        "hash_splat_nil" => {
            let keyword = direct_children(node)
                .into_iter()
                .find(|child| child.kind_str() == "nil")?;
            Some(Gate::new(
                KEYWORD_ARGUMENT_REJECTION_SINCE,
                "unexpected token kNIL".to_owned(),
                keyword.byte_range(),
            ))
        }
        // The parser stops at the arrow, so the pattern written after it is never read.
        "match_pattern" => {
            let arrow = direct_children(node)
                .into_iter()
                .find(|child| !child.is_named() && child.kind_str() == "=>")?;
            Some(
                Gate::new(
                    RIGHTWARD_ASSIGNMENT_SINCE,
                    "unexpected token tASSOC".to_owned(),
                    arrow.byte_range(),
                )
                .recovers_through(node.end_byte()),
            )
        }
        // `in [*, x, *]` -- an array pattern before 3.0 has room for one splat, so the second one
        // is where the parser stops.
        "find_pattern" => {
            let splat = direct_children(node)
                .into_iter()
                .filter(|child| child.kind_str() == "splat_parameter")
                .nth(1)?;
            let start = splat.start_byte();
            let gate = Gate::new(
                FIND_PATTERN_SINCE,
                "unexpected token tSTAR".to_owned(),
                start..start + 1,
            );
            // Giving up inside a `case`/`in` consumes the `end` that closes it, and the keyword
            // the parser then trips over leaves it with nothing to read the rest of the file
            // against. A one-line `in` or `=>` ends at its own statement, which the parser picks
            // the next one up after.
            let in_case_expression = ancestor_matching(node, |ancestor| {
                matches!(
                    ancestor.kind_str(),
                    "case_match" | "test_pattern" | "match_pattern"
                )
            })
            .is_some_and(|owner| owner.kind_str() == "case_match");
            Some(match in_case_expression {
                true => gate.abandons_file(),
                false => {
                    gate.recovers_through(enclosing_statement(node).unwrap_or(node).end_byte())
                }
            })
        }
        "parenthesized_statements"
            if !node.has_error() && command_argument_parentheses(node, context) =>
        {
            let (reason, range) = command_argument_error(node, context)?;
            Some(Gate::new(COMMAND_ARGUMENT_STATEMENTS_SINCE, reason, range).in_method_body())
        }
        // The omitted value leaves the parser looking at whatever follows the label.
        "pair" if node.field("value").is_none() => {
            let (reason, range) = unexpected_token_after(node, context);
            let discarded = recovery_region(node).end_byte();
            Some(Gate::new(HASH_VALUE_OMISSION_SINCE, reason, range).recovers_through(discarded))
        }
        "block_parameter" | "block_argument" if node.named_child_count() == 0 => {
            let (reason, range) = unexpected_token_after(node, context);
            Some(Gate::new(ANONYMOUS_BLOCK_FORWARDING_SINCE, reason, range))
        }
        // Declaring `def foo(*)` or `def foo(**)` has always been allowed; passing the collected
        // arguments on without naming them is what Ruby 3.2 added.
        "splat_argument" | "hash_splat_argument" if node.named_child_count() == 0 => {
            let (reason, range) = unexpected_token_after(node, context);
            Some(Gate::new(ANONYMOUS_REST_FORWARDING_SINCE, reason, range))
        }
        _ => None,
    }
}

/// The construct the parser discards after reporting an omitted value inside `node`.
fn recovery_region(node: Node<'_>) -> Node<'_> {
    let mut region = node;
    while let Some(parent) = region.parent() {
        if !OMISSION_RECOVERY_KINDS.contains(&parent.kind_str()) {
            break;
        }
        region = parent;
    }
    region
}

/// Whether `paren` is where the lexer reads the `(` as `tLPAREN_ARG`: the start of a command's
/// first argument, or the operand of `defined?` and `not`, all of which leave it expecting an
/// argument with a space in front of the parenthesis. A `(` that opens an argument list (`p(x)`)
/// or one the lexer meets with a fresh expression expected (`a = (x)`, `foo bar, (x)`) is an
/// ordinary `tLPAREN` and takes a whole `compstmt`.
fn command_argument_parentheses(paren: Node<'_>, context: &RuleContext<'_>) -> bool {
    if !preceded_by_space(paren, context) {
        return false;
    }
    let mut current = paren;
    loop {
        let Some(parent) = current.parent_of(context) else {
            return false;
        };
        if opens_command_argument(parent, current) {
            return true;
        }
        // Climbing past a node the parenthesis does not begin would ask about a `(` the lexer was
        // no longer looking at when it chose between the two tokens.
        if parent.start_byte() != paren.start_byte() {
            return false;
        }
        current = parent;
    }
}

fn opens_command_argument(parent: Node<'_>, child: Node<'_>) -> bool {
    match parent.kind_str() {
        // An argument list written without parentheses of its own belongs to a command call, and
        // only its first argument stands where the method name has just been read. `return`,
        // `break` and `next` leave the lexer expecting a fresh expression instead.
        "argument_list" => {
            parent.child(0).is_some_and(|first| first.kind_str() != "(")
                && parent
                    .named_child(0)
                    .is_some_and(|first| first.id() == child.id())
                && parent.parent().is_some_and(|owner| {
                    matches!(owner.kind_str(), "call" | "yield") && command_stands_here(owner)
                })
        }
        "unary" => parent
            .field("operator")
            .is_some_and(|operator| matches!(operator.kind_str(), "defined?" | "not")),
        _ => false,
    }
}

/// Whether a command call is allowed where `call` is written.
///
/// `call_args` accepts a command only in place of a whole argument list, which it then swallows
/// the rest of, so a command written after another argument -- or inside an array or a hash -- is
/// what the parser rejects, at the parenthesis and whatever Ruby version it runs as. Nothing
/// inside those parentheses is ever reached.
fn command_stands_here(call: Node<'_>) -> bool {
    let Some(parent) = call.parent() else {
        return true;
    };
    match parent.kind_str() {
        "argument_list" => parent
            .named_child(0)
            .is_some_and(|first| first.id() == call.id()),
        "array" | "hash" | "pair" => false,
        _ => true,
    }
}

/// Whether whitespace sits in front of `node`, which is what makes the lexer read a parenthesis as
/// the start of an argument rather than as the argument list of the call it follows.
fn preceded_by_space(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let start = node.start_byte();
    start > 0
        && context
            .source
            .text()
            .as_bytes()
            .get(start - 1)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
}

/// One thing a parser older than 3.3 reads between the parentheses of a command argument.
#[derive(Clone, Copy)]
enum Interior<'tree> {
    Statement(Node<'tree>),
    Semicolon(usize),
    Newline(usize),
}

/// The token a parser older than 3.3 trips over inside a `(...)` written as a command argument.
///
/// `tLPAREN_ARG` takes one statement and one optional newline before the `)`, where the `tLPAREN`
/// of an ordinary parenthesised expression takes a whole `compstmt`. Newlines in front of the
/// statement are not tokens at all -- the lexer is still expecting the beginning of an expression
/// there -- but the action that runs once the statement is reduced puts it back in a state where
/// they are, which is why the second of two trailing newlines is what gets blamed.
fn command_argument_error(
    paren: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<(String, Range<usize>)> {
    let items = paren_interior(paren, context);
    let first = items
        .iter()
        .position(|item| !matches!(item, Interior::Newline(_)))?;
    let offending = match items[first] {
        Interior::Statement(_) => {
            let after = first + 1;
            let after = after + usize::from(matches!(items.get(after), Some(Interior::Newline(_))));
            *items.get(after)?
        }
        item => item,
    };
    Some(match offending {
        Interior::Semicolon(at) => ("unexpected token tSEMI".to_owned(), at..at + 1),
        Interior::Newline(at) => ("unexpected token tNL".to_owned(), at..at + 1),
        Interior::Statement(statement) => {
            let token = leftmost_token(statement);
            (
                format!("unexpected token {}", token_name(token)),
                token.byte_range(),
            )
        }
    })
}

/// What the parser reads between the parentheses of `paren`, in order.
///
/// Comments and the body of a heredoc are invisible to it: a comment is dropped by the lexer, and
/// a heredoc body belongs to the token that opened it however far down the file it is written.
fn paren_interior<'tree>(paren: Node<'tree>, context: &RuleContext<'_>) -> Vec<Interior<'tree>> {
    let mut items = Vec::new();
    let mut offset = paren.start_byte() + 1;
    let mut closing = paren.end_byte();
    for child in direct_children(paren) {
        if !child.is_named() {
            if child.kind_str() == ")" {
                closing = child.start_byte();
            }
            continue;
        }
        push_separators(&mut items, context, offset..child.start_byte());
        match child.kind_str() {
            "comment" | "heredoc_body" => {}
            "empty_statement" => items.push(Interior::Semicolon(child.start_byte())),
            _ => items.push(Interior::Statement(child)),
        }
        offset = child.end_byte();
    }
    push_separators(&mut items, context, offset..closing.max(offset));
    items
}

/// The statement separators written in a stretch of source that holds nothing else.
fn push_separators(items: &mut Vec<Interior<'_>>, context: &RuleContext<'_>, range: Range<usize>) {
    let start = range.start;
    let mut continued = false;
    for (index, character) in context.source.slice(range).char_indices() {
        match character {
            // A backslash takes the newline after it out of the token stream.
            '\n' if continued => {}
            '\n' => items.push(Interior::Newline(start + index)),
            ';' => items.push(Interior::Semicolon(start + index)),
            _ => {}
        }
        continued = character == '\\';
    }
}

/// What the parser reports for the rest of a file after an error inside a method body.
///
/// Recovering from it never finishes the definition the error was found in, so the flag `parse.y`
/// keeps for being inside one stays set for good: every later `class` or `module` definition that
/// is not itself written inside one is reported as being written in a method body. `class << obj`
/// carries no such check and is left alone. The file then ends a keyword short as well, unless the
/// last thing the parser read was one of those definitions.
/// The `{ … }` block a definition was written inside, which is the one whose closing brace loses
/// its opener. A `do … end` block does not: its `end` is a keyword the parser can still pair up.
fn enclosing_brace_block<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if ancestor.kind_str() == "block" {
            return Some(ancestor);
        }
        if matches!(
            ancestor.kind_str(),
            "method" | "singleton_method" | "do_block"
        ) {
            return None;
        }
        current = ancestor.parent();
    }
    None
}

/// What the parser reports after an endless definition inside a block.
///
/// `def name(args)` still parses, so the `= body` is what it stops on -- and once the definition is
/// closed there, the `}` that was meant to end the block has no opener left. The file then runs out
/// of input with the block still unclosed.
fn endless_in_block_recovery(node: Node<'_>, context: &RuleContext<'_>, out: &mut Vec<Diagnostic>) {
    let Some(block) = enclosing_brace_block(node) else {
        return;
    };
    let Some(brace) = block
        .child(block.child_count().saturating_sub(1) as u32)
        .filter(|last| last.kind_str() == "}")
    else {
        return;
    };
    out.push(Diagnostic {
        reason: "unexpected token tRCURLY".to_owned(),
        range: brace.byte_range(),
    });
    let (reason, range) = end_of_input(context);
    out.push(Diagnostic { reason, range });
}

fn method_body_recovery(node: Node<'_>, context: &RuleContext<'_>, out: &mut Vec<Diagnostic>) {
    if !inside_method_body(node) {
        return;
    }
    let Some(statement) = enclosing_statement(node) else {
        return;
    };
    let resumed_at = statement.end_byte();
    for definition in context.nodes().filter(|candidate| {
        candidate.is_named()
            && matches!(candidate.kind_str(), "class" | "module")
            && candidate.start_byte() >= resumed_at
            && ancestor_matching(*candidate, |ancestor| {
                matches!(ancestor.kind_str(), "class" | "module" | "singleton_class")
            })
            .is_none_or(|ancestor| ancestor.start_byte() < resumed_at)
    }) {
        let (keyword, reason) = match definition.kind_str() {
            "module" => ("module", "module definition in method body"),
            _ => ("class", "class definition in method body"),
        };
        out.push(Diagnostic {
            reason: reason.to_owned(),
            range: definition.start_byte()..definition.start_byte() + keyword.len(),
        });
    }
    if last_statement_after(statement)
        .is_some_and(|last| !matches!(last.kind_str(), "class" | "module"))
    {
        let (reason, range) = end_of_input(context);
        out.push(Diagnostic { reason, range });
    }
}

/// Whether the parser is inside a method definition where `node` is written. Entering a class or
/// module body clears the flag `parse.y` keeps for that, and a block leaves it alone.
fn inside_method_body(node: Node<'_>) -> bool {
    ancestor_matching(node, |ancestor| {
        matches!(
            ancestor.kind_str(),
            "method" | "singleton_method" | "class" | "module" | "singleton_class"
        )
    })
    .is_some_and(|owner| matches!(owner.kind_str(), "method" | "singleton_method"))
}

/// The last statement the parser reads after the one holding `statement`, at the outermost level
/// that has anything left after it. Statements written inside that one are read as part of it.
fn last_statement_after(statement: Node<'_>) -> Option<Node<'_>> {
    let mut last = None;
    let mut current = statement;
    loop {
        if let Some(body) = current.parent() {
            last = significant_named_children(body)
                .filter(|sibling| sibling.start_byte() >= current.end_byte())
                .last()
                .or(last);
        }
        let Some(owner) = current.parent().and_then(|body| body.parent()) else {
            return last;
        };
        let Some(next) = enclosing_statement(owner) else {
            return last;
        };
        current = next;
    }
}

/// How the parser names the token that follows `node`, which is where it notices the omission.
fn unexpected_token_after(node: Node<'_>, context: &RuleContext<'_>) -> (String, Range<usize>) {
    match following_token(node) {
        Some(token) => (
            format!("unexpected token {}", token_name(token)),
            token.byte_range(),
        ),
        None => end_of_input(context),
    }
}

/// The parser reports running out of input at a zero-width position past the last byte. RuboCop
/// widens that onto the final character so the range can be displayed (`lint/syntax.rb`'s
/// `diagnostic_location`).
fn end_of_input(context: &RuleContext<'_>) -> (String, Range<usize>) {
    let text = context.source.text();
    let start = text
        .char_indices()
        .next_back()
        .map_or(0, |(offset, _)| offset);
    ("unexpected token $end".to_owned(), start..text.len())
}

/// The next token after `node`, skipping comments the way the lexer does.
fn following_token(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node;
    loop {
        match current.next_sibling() {
            Some(sibling) => {
                let token = leftmost_token(sibling);
                if token.kind_str() == "comment" {
                    current = token;
                    continue;
                }
                return Some(token);
            }
            None => current = current.parent()?,
        }
    }
}

fn leftmost_token(node: Node<'_>) -> Node<'_> {
    let mut current = node;
    while let Some(child) = current.child(0) {
        current = child;
    }
    current
}

/// The `parser` gem's name for a token, which is what its diagnostics quote.
///
/// The lexers do not agree node for node, so this covers the tokens a diagnostic actually lands on
/// -- delimiters, operators and keywords -- and falls back to `tIDENTIFIER` for the rest, which is
/// what an unrecognized word lexes as.
fn token_name(token: Node<'_>) -> &'static str {
    if token.kind_str() == "hash_key_symbol" {
        return "tLABEL";
    }
    match token.kind_str() {
        "," => "tCOMMA",
        "(" => "tLPAREN",
        ")" => "tRPAREN",
        "{" => "tLCURLY",
        "}" => "tRCURLY",
        "[" => "tLBRACK",
        "]" => "tRBRACK",
        "=>" => "tASSOC",
        "=" => "tEQL",
        "|" => "tPIPE",
        ";" => "tSEMI",
        "&" => "tAMPER",
        "&." => "tANDDOT",
        "*" => "tSTAR",
        "**" => "tDSTAR",
        "::" => "tCOLON2",
        "." => "tDOT",
        ":" => "tCOLON",
        "?" => "tEH",
        "end" => "kEND",
        "do" => "kDO",
        "then" => "kTHEN",
        "else" => "kELSE",
        "elsif" => "kELSIF",
        "when" => "kWHEN",
        "in" => "kIN",
        "rescue" => "kRESCUE",
        "ensure" => "kENSURE",
        "nil" => "kNIL",
        "if" => "kIF",
        "unless" => "kUNLESS",
        "while" => "kWHILE",
        "until" => "kUNTIL",
        "class" => "kCLASS",
        "module" => "kMODULE",
        "def" => "kDEF",
        "constant" => "tCONSTANT",
        "instance_variable" => "tIVAR",
        "class_variable" => "tCVAR",
        "global_variable" => "tGVAR",
        "integer" => "tINTEGER",
        "float" => "tFLOAT",
        "\"" | "'" => "tSTRING_BEG",
        _ => "tIDENTIFIER",
    }
}

fn direct_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

fn legacy_forwarding_recovery(
    parameter: Node<'_>,
    context: &RuleContext<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let Some(method) = ancestor_matching(parameter, |node| {
        matches!(node.kind_str(), "method" | "singleton_method")
    }) else {
        return;
    };
    let Some(container) =
        ancestor_matching(method, |node| matches!(node.kind_str(), "class" | "module"))
    else {
        return;
    };
    let Some(body) = container.field("body") else {
        return;
    };
    let later_nodes = significant_named_children(body)
        .filter(|node| node.start_byte() >= method.end_byte())
        .collect::<Vec<_>>();
    if later_nodes.is_empty() {
        return;
    }

    let (keyword, reason) = if container.kind_str() == "class" {
        ("class", "class definition in method body")
    } else {
        ("module", "module definition in method body")
    };
    out.push(Diagnostic {
        reason: reason.to_owned(),
        range: container.start_byte()..container.start_byte() + keyword.len(),
    });

    let has_preceding_top_level_statement =
        std::iter::successors(container.prev_named_sibling(), |node| {
            node.prev_named_sibling()
        })
        .any(|node| node.kind_str() != "comment");
    let later_nonempty_method = later_nodes.iter().any(|node| {
        matches!(node.kind_str(), "method" | "singleton_method")
            && node
                .field("body")
                .is_some_and(|body| significant_named_children(body).next().is_some())
    });
    if container.kind_str() != "module"
        || !has_preceding_top_level_statement
        || !later_nonempty_method
    {
        return;
    }

    let end = container.end_byte();
    let start = end.saturating_sub(3);
    if context.source.slice(start..end) == "end" {
        out.push(Diagnostic {
            reason: "unexpected token kEND".to_owned(),
            range: start..end,
        });
    }
}

fn significant_named_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind_str() != "comment")
        .collect::<Vec<_>>()
        .into_iter()
}

fn ancestor_matching(mut node: Node<'_>, predicate: impl Fn(Node<'_>) -> bool) -> Option<Node<'_>> {
    while let Some(parent) = node.parent() {
        if predicate(parent) {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn syntax_message(reason: &str, target: RubyVersion) -> String {
    format!(
        "{reason}\n(Using Ruby {target} parser; configure using `TargetRubyVersion` parameter, under `AllCops`)"
    )
}
