use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::access_modifier::in_macro_scope;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::verified_by_reparse;

const REQUIRE_MSG: &str = "Use parentheses for method calls with arguments.";
const OMIT_MSG: &str = "Omit parentheses for method calls with arguments.";

/// `"yield".len()`, which is the selector of a `yield`.
const YIELD_LENGTH: usize = 5;

/// Whether a call that was given arguments writes parentheses around them.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "require_parentheses".to_owned());
    match style.as_str() {
        "omit_parentheses" => omit_parentheses(context, offenses),
        _ => require_parentheses(context, offenses),
    }
}

/// `require_parentheses`.
fn require_parentheses(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed = Allowed::new(context);
    let macros = Macros::new(context);
    for node in context.nodes_of_any(&["call", "yield"]) {
        let Some(selector) = selector_end(node) else {
            continue;
        };
        let name = method_name(node, context);
        if allowed.covers(name) {
            continue;
        }
        // `eligible_for_parentheses_omission?`. A setter and an operator written as an operator are
        // not calls with arguments in the grammar at all, so only a dotted operator gets here.
        if super::nodes::is_operator_method(name) || macros.ignores(node, name, context) {
            continue;
        }
        let Some(arguments) = argument_list(node) else {
            continue;
        };
        let written = super::nodes::children(arguments);
        if written.is_empty() || is_parenthesized(node, arguments, context) {
            continue;
        }
        // `args_begin`: the character after the selector, which is the blank before the arguments.
        // A single argument that is itself parenthesized lends its own opening paren, so two are
        // replaced by one and no closing paren is added.
        let lends_parens =
            matches!(written.as_slice(), [only] if only.kind_str() == "parenthesized_statements");
        let mut edits = vec![Edit {
            start: selector,
            end: selector + if lends_parens { 2 } else { 1 },
            replacement: "(".to_owned(),
            safe: true,
        }];
        if !lends_parens {
            let end = arguments.end_byte();
            edits.push(Edit {
                start: end,
                end,
                replacement: ")".to_owned(),
                safe: true,
            });
        }
        offenses.push(
            context
                .offense(REQUIRE_MSG, node.start_byte()..arguments.end_byte())
                .corrected_by_all(edits),
        );
    }
}

/// `AllowedMethods` and `AllowedPatterns`, both empty by default.
struct Allowed {
    methods: Vec<String>,
    patterns: Vec<Regex>,
}

impl Allowed {
    fn new(context: &RuleContext<'_>) -> Self {
        Self {
            methods: context.setting("AllowedMethods").unwrap_or_default(),
            patterns: patterns(context, "AllowedPatterns"),
        }
    }

    fn covers(&self, name: &str) -> bool {
        self.methods.iter().any(|allowed| allowed == name)
            || self.patterns.iter().any(|pattern| pattern.is_match(name))
    }
}

/// `ignored_macro?`: a call standing where a class body's macros do is left alone unless it is one
/// of the ones named as an exception.
struct Macros {
    ignore: bool,
    included: Vec<String>,
    patterns: Vec<Regex>,
}

impl Macros {
    fn new(context: &RuleContext<'_>) -> Self {
        Self {
            ignore: context.setting("IgnoreMacros").unwrap_or(true),
            included: context.setting("IncludedMacros").unwrap_or_default(),
            patterns: patterns(context, "IncludedMacroPatterns"),
        }
    }

    fn ignores(&self, node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
        self.ignore
            && node.field("receiver").is_none()
            && in_macro_scope(node, context)
            && !self.included.iter().any(|macro_name| macro_name == name)
            && !self.patterns.iter().any(|pattern| pattern.is_match(name))
    }
}

fn patterns(context: &RuleContext<'_>, key: &str) -> Vec<Regex> {
    context
        .setting::<Vec<String>>(key)
        .unwrap_or_default()
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
}

/// Where the selector ends, which is where the arguments' opening paren would go.
///
/// `super` reaches upstream through `on_super` rather than `on_send`, so this cop never sees one,
/// and a `foo.()` has no selector at all.
fn selector_end(node: Node<'_>) -> Option<usize> {
    if node.kind_str() == "yield" {
        return Some(node.start_byte() + YIELD_LENGTH);
    }
    let selector = node.field("method")?;
    (selector.kind_str() != "super").then(|| selector.end_byte())
}

fn method_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> &'a str {
    match node.kind_str() {
        "yield" => "yield",
        _ => node
            .field("method")
            .map_or("", |selector| context.source.node_text(selector)),
    }
}

/// The list the arguments were written in, which a `yield` holds without naming.
fn argument_list<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.field("arguments").or_else(|| {
        super::nodes::children(node)
            .into_iter()
            .find(|child| child.kind_str() == "argument_list")
    })
}

/// `parenthesized?`: the parentheses belong to the call rather than to its first argument, which is
/// what `foo(1)` has and `foo (1)` does not.
///
/// Upstream settles this while lexing -- a `(` written straight against the selector opens the call
/// -- so the answer is adjacency, not shape. Reading the shape instead is wrong in one direction:
/// the grammar here usually gives the call's own parentheses as `(` and `)` tokens of the argument
/// list, but for some receivers it wraps them in a `parenthesized_statements` node, which then reads
/// as `foo (1)` and makes a properly parenthesized call look bare.
///
/// `expect(x).to receive(:y) do ... end.at_least(:once)` is such a receiver: the `at_least(:once)`
/// there parses to the wrapped shape while the same call after a simpler receiver does not.
fn is_parenthesized(node: Node<'_>, arguments: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(selector) = selector_end(node) else {
        return false;
    };
    // The argument list begins where the arguments do, so a `(` of its own is the call's only when
    // nothing separates it from the selector.
    arguments.start_byte() == selector
        && context.source.text().as_bytes().get(selector) == Some(&b'(')
}

/// `omit_parentheses`.
///
/// Every candidate's own correction is reparsed before the offense is registered, so a pair of
/// parentheses whose removal would change how the line reads is never reported.
fn omit_parentheses(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let settings = Omission::new(context);
    let locals = LocalVariables::new(context);
    let mut pending = Vec::new();
    for node in context.nodes_of_any(&["call", "yield"]) {
        if settings.omits(node, context, &locals) {
            pending.push(node);
        }
    }
    // A heredoc's body is parked outside the node that opened it, and where the grammar parks it
    // moves with the parentheses, so the two trees never compare equal however little the
    // correction changed. Upstream's parser holds the body inside the literal and sees no
    // difference at all, which is the answer the reparse is standing in for.
    let (with_heredoc, reparsed): (Vec<Node<'_>>, Vec<Node<'_>>) =
        pending.into_iter().partition(|node| carries_heredoc(*node));
    let mut verified = verified_by_reparse(
        context,
        reparsed,
        |node| omission_edits(*node, context),
        |node| omission_range(*node),
        // 本家の `verified_by_reparse(@pending_omit_offenses)` は追加のオプションを渡さない。
        // `fold_empty_call_parentheses` は本家の設定ではなく、`foo()` と `foo` を 1 つの
        // ノードにする本家のパーサに合わせるためのもの。名前を局所変数が持っている呼び出しは
        // `omits` が先に外している。
        crate::rules::support::Verification {
            fold_empty_call_parentheses: true,
            ..Default::default()
        },
    );
    verified.extend(with_heredoc);
    verified.sort_by_key(tree_sitter::Node::start_byte);
    for node in verified {
        offenses.push(
            context
                .offense(OMIT_MSG, omission_range(node))
                .corrected_by_all(omission_edits(node, context)),
        );
    }
}

/// The four settings that let parentheses stand where the style would otherwise take them away.
struct Omission {
    multiline: bool,
    chaining: bool,
    camel_case: bool,
    interpolation: bool,
}

impl Omission {
    fn new(context: &RuleContext<'_>) -> Self {
        Self {
            multiline: context
                .setting("AllowParenthesesInMultilineCall")
                .unwrap_or(false),
            chaining: context
                .setting("AllowParenthesesInChaining")
                .unwrap_or(false),
            camel_case: context
                .setting("AllowParenthesesInCamelCaseMethod")
                .unwrap_or(false),
            interpolation: context
                .setting("AllowParenthesesInStringInterpolation")
                .unwrap_or(false),
        }
    }

    fn omits(
        &self,
        node: Node<'_>,
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) -> bool {
        let Some(arguments) = argument_list(node) else {
            return false;
        };
        if !is_parenthesized(node, arguments, context) {
            return false;
        }
        let written = super::nodes::children(arguments);
        // Upstream's reparse settles this one: `foo()` is a call, `foo` is the local variable of
        // that name, and the two trees do not match. The comparison here cannot tell them apart,
        // so the candidate never reaches it.
        !locals.shadows_a_local(node)
            && !inside_endless_method_def(node, &written, context)
            && !hash_value_omission_needs_parentheses(node, &written, context)
            && !syntax_like_method_call(node, context)
            && !before_constant_resolution(node, context)
            && !self.legitimate(node, arguments, &written, context, locals)
            && !self.allowed_camel_case(node, &written, context)
            && !self.allowed_string_interpolation(node, context)
    }

    /// `legitimate_call_with_parentheses?`.
    fn legitimate(
        &self,
        node: Node<'_>,
        arguments: Node<'_>,
        written: &[Node<'_>],
        context: &RuleContext<'_>,
        locals: &LocalVariables<'_, '_>,
    ) -> bool {
        call_in_literals(node, context)
            || parent_is_when(node, context)
            || call_with_ambiguous_arguments(node, arguments, written, context, locals)
            || call_in_logical_operators(node, context)
            || call_in_optional_arguments(node, context)
            || call_in_single_line_inheritance(node, context)
            || (self.multiline && is_multiline(node, arguments, context))
            || (self.chaining && chained_with_parentheses(node, context))
            || assignment_in_condition(node, context)
            || forwards_anonymous_rest_arguments(written)
    }

    /// `allowed_camel_case_method_call?`.
    fn allowed_camel_case(
        &self,
        node: Node<'_>,
        written: &[Node<'_>],
        context: &RuleContext<'_>,
    ) -> bool {
        method_name(node, context).starts_with(|character: char| character.is_ascii_uppercase())
            && (written.is_empty() || self.camel_case)
    }

    /// `allowed_string_interpolation_method_call?`.
    fn allowed_string_interpolation(&self, node: Node<'_>, context: &RuleContext<'_>) -> bool {
        self.interpolation && inside_string_interpolation(node, context)
    }
}

/// `offense_range`: from the opening parenthesis through the closing one.
fn omission_range(node: Node<'_>) -> std::ops::Range<usize> {
    argument_list(node).map_or_else(|| node.byte_range(), |arguments| arguments.byte_range())
}

/// `autocorrect`: the opening parenthesis becomes the blank that separates the arguments, and the
/// closing one goes away.
fn omission_edits(node: Node<'_>, context: &RuleContext<'_>) -> Vec<Edit> {
    let Some(arguments) = argument_list(node) else {
        return Vec::new();
    };
    let open = arguments.start_byte();
    let close = arguments.end_byte();
    // `parentheses_at_the_end_of_multiline_call?`: a `(` closing its line has to leave a line
    // continuation behind, since a blank after one is a syntax error.
    let mut replacement = " ".to_owned();
    let mut end = open + 1;
    if is_multiline(node, arguments, context) && opens_the_line_end(arguments, context) {
        replacement = " \\".to_owned();
        let text = context.source.text().as_bytes();
        while text
            .get(end)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            end += 1;
        }
    }
    vec![
        Edit {
            start: open,
            end,
            replacement,
            safe: true,
        },
        Edit {
            start: close - 1,
            end: close,
            replacement: String::new(),
            safe: true,
        },
    ]
}

fn opens_the_line_end(arguments: Node<'_>, context: &RuleContext<'_>) -> bool {
    let line = context.source.line_column(arguments.start_byte()).0;
    context.source.line(line).trim_end().ends_with('(')
}

/// `inside_endless_method_def?`: an endless definition needs the parentheses to tell its body from
/// the arguments.
fn inside_endless_method_def(
    node: Node<'_>,
    written: &[Node<'_>],
    context: &RuleContext<'_>,
) -> bool {
    if written.is_empty() {
        return false;
    }
    let mut current = node.parent_of(context);
    while let Some(candidate) = current {
        if matches!(candidate.kind_str(), "method" | "singleton_method")
            && candidate
                .field("body")
                .is_some_and(|body| body.kind_str() != "body_statement")
        {
            return true;
        }
        current = candidate.parent_of(context);
    }
    false
}

/// `require_parentheses_for_hash_value_omission?`: `foo(x:)` keeps its parentheses where the value
/// it leaves out could otherwise be read as belonging to what follows.
fn hash_value_omission_needs_parentheses(
    node: Node<'_>,
    written: &[Node<'_>],
    context: &RuleContext<'_>,
) -> bool {
    let Some(last) = written.last() else {
        return false;
    };
    let omits_value = match last.kind_str() {
        "pair" => last.field("value").is_none(),
        "hash" => super::nodes::children(*last)
            .last()
            .is_some_and(|pair| pair.kind_str() == "pair" && pair.field("value").is_none()),
        _ => false,
    };
    if !omits_value {
        return false;
    }
    let parent = raw_parent(node, context);
    parent.is_some_and(|parent| is_conditional(parent) || is_single_line(parent, context))
        || !is_last_expression(node, context)
}

/// `last_expression?`: nothing is written after the call, or after the assignment it feeds.
fn is_last_expression(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    // A call that carries a block is the block's first child upstream, and the parameter list and
    // the body come after it, so something is always written after it there.
    if node.field("block").is_some() {
        return false;
    }
    let (subject, parent) = match parser_parent(node, context) {
        Some(parent) if is_assignment(parent) => (parent, parser_parent(parent, context)),
        parent => (node, parent),
    };
    let Some(parent) = parent else {
        return true;
    };
    statements(parent)
        .into_iter()
        .skip_while(|child| child.id() != subject.id())
        .nth(1)
        .is_none()
}

/// The statements a list holds, leaving out the clauses upstream's parser lifts out of the `begin`
/// they guard: a `rescue` is a node *around* the statements there rather than one among them.
fn statements<'tree>(parent: Node<'tree>) -> Vec<Node<'tree>> {
    super::nodes::children(parent)
        .into_iter()
        .filter(|child| !matches!(child.kind_str(), "rescue" | "ensure" | "else"))
        .collect()
}

/// `syntax_like_method_call?`: `foo.()` and an operator written as one need what they were written
/// with.
fn syntax_like_method_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "yield" {
        return false;
    }
    match node.field("method") {
        // `implicit_call?`: `foo.(1)` names no method at all.
        None => true,
        Some(selector) => {
            selector.kind_str() == "super"
                || super::nodes::is_operator_method(context.source.node_text(selector))
        }
    }
}

/// `method_call_before_constant_resolution?`: `foo(1)::Bar` reads the constant off the result.
fn before_constant_resolution(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    raw_parent(node, context).is_some_and(|parent| parent.kind_str() == "scope_resolution")
}

/// `call_in_literals?`.
fn call_in_literals(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    parent_beyond_block(node, context).is_some_and(|parent| {
        matches!(parent.kind_str(), "pair" | "array" | "range")
            || is_splat(parent)
            || parent.kind_str() == "conditional"
    })
}

/// `node.parent&.when_type?`: the grammar wraps a `when`'s conditions in a list of their own.
fn parent_is_when(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    raw_parent(node, context).is_some_and(|parent| parent.kind_str() == "when")
}

/// `call_with_ambiguous_arguments?`.
fn call_with_ambiguous_arguments(
    node: Node<'_>,
    arguments: Node<'_>,
    written: &[Node<'_>],
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    // `call_with_braced_block?`.
    if node
        .field("block")
        .is_some_and(|block| block.kind_str() == "block")
    {
        return true;
    }
    // `call_in_argument_with_block?`: a call standing where a block does, inside another call.
    if block_parent(node, context)
        .is_some_and(|parent| is_send_like(parent, context) || is_setter_call(parent))
    {
        return true;
    }
    // `call_as_argument_or_chain?`.
    if raw_parent(node, context).is_some_and(|parent| is_send_like(parent, context)) {
        return true;
    }
    // `call_in_match_pattern?`.
    if raw_parent(node, context)
        .is_some_and(|parent| matches!(parent.kind_str(), "match_pattern" | "test_pattern"))
    {
        return true;
    }
    let _ = arguments;
    if hash_literal_in_arguments(node, written, context, locals)
        || ambiguous_range_argument(written)
    {
        return true;
    }
    descendants(node, context)
        .into_iter()
        .any(|child| is_ambiguous_descendant(child, context))
}

fn is_ambiguous_descendant(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    matches!(
        node.kind_str(),
        "forward_argument" | "block" | "do_block" | "lambda"
    ) || is_ambiguous_literal(node, context)
        || is_logical_operator(node, context)
}

/// `hash_literal_in_arguments?`.
fn hash_literal_in_arguments<'tree>(
    node: Node<'tree>,
    written: &[Node<'tree>],
    context: &'tree RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> bool {
    written.iter().any(|argument| {
        argument.kind_str() == "hash"
            // A bare name is a receiverless call there unless it names a local variable, which is
            // an `lvar` and no call at all.
            || ((matches!(argument.kind_str(), "call" | "element_reference")
                || (argument.kind_str() == "identifier" && !locals.is_lvar(*argument)))
                && descendants(node, context)
                    .into_iter()
                    .any(|child| child.kind_str() == "hash"))
    })
}

/// `ambiguous_range_argument?`: a range missing an end reads on into whatever follows.
fn ambiguous_range_argument(written: &[Node<'_>]) -> bool {
    let beginless = written
        .first()
        .is_some_and(|first| first.kind_str() == "range" && first.field("begin").is_none());
    let endless = written
        .last()
        .is_some_and(|last| last.kind_str() == "range" && last.field("end").is_none());
    beginless || endless
}

/// `call_in_logical_operators?`.
fn call_in_logical_operators(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = parent_beyond_block(node, context) else {
        return false;
    };
    if is_logical_operator(parent, context) {
        return true;
    }
    matches!(parent.kind_str(), "call" | "element_reference")
        && argument_list(parent).is_some_and(|arguments| {
            super::nodes::children(arguments)
                .into_iter()
                .any(|argument| is_logical_operator(argument, context))
        })
}

/// `call_in_optional_arguments?`.
fn call_in_optional_arguments(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        matches!(
            parent.kind_str(),
            "optional_parameter" | "keyword_parameter"
        )
    })
}

/// `call_in_single_line_inheritance?`.
fn call_in_single_line_inheritance(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    raw_parent(node, context)
        .is_some_and(|parent| parent.kind_str() == "class" && is_single_line(parent, context))
}

/// `allowed_chained_call_with_parentheses?`: the call it hangs off already writes its own.
fn chained_with_parentheses(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    loop {
        let Some(previous) = current
            .field("receiver")
            .filter(|node| node.kind_str() == "call")
        else {
            return false;
        };
        if argument_list(previous)
            .is_some_and(|arguments| is_parenthesized(previous, arguments, context))
        {
            return true;
        }
        current = previous;
    }
}

/// `assignment_in_condition?`.
fn assignment_in_condition(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = raw_parent(node, context) else {
        return false;
    };
    if !is_assignment(parent) {
        return false;
    }
    parser_parent(parent, context)
        .is_some_and(|grandparent| is_conditional(grandparent) || grandparent.kind_str() == "when")
}

/// `forwards_anonymous_rest_arguments?`: `foo(*)` and `foo(**)` name nothing to forward.
fn forwards_anonymous_rest_arguments(written: &[Node<'_>]) -> bool {
    let Some(last) = written.last() else {
        return false;
    };
    match last.kind_str() {
        "splat_argument" => last.named_child_count() == 0,
        "hash" => super::nodes::children(*last).into_iter().any(|child| {
            child.kind_str() == "hash_splat_argument" && child.named_child_count() == 0
        }),
        "hash_splat_argument" => last.named_child_count() == 0,
        _ => false,
    }
}

/// `ambiguous_literal?`.
fn is_ambiguous_literal(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if is_splat(node) || node.kind_str() == "conditional" {
        return true;
    }
    // `regexp_slash_literal?`.
    if node.kind_str() == "regex" {
        return context.source.node_text(node).starts_with('/');
    }
    // `unary_literal?`: a signed number, and anything a unary operator was written in front of.
    node.kind_str() == "unary"
        && node.field("operator").is_some_and(|operator| {
            matches!(
                context.source.node_text(operator),
                "!" | "~" | "-" | "+" | "not"
            )
        })
}

/// `logical_operator?`: `&&` and `||`, but not the `and` and `or` that read as words.
fn is_logical_operator(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.kind_str() == "binary"
        && node
            .field("operator")
            .is_some_and(|operator| matches!(context.source.node_text(operator), "&&" | "||"))
}

/// `splat?`.
///
/// An anonymous `*`, `**` or `&` carries no expression and is a `forwarded_restarg` and its
/// siblings there rather than a splat, so it is none of the three types this asks about.
fn is_splat(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "splat_argument" | "hash_splat_argument" | "block_argument"
    ) && node.named_child_count() > 0
}

fn is_assignment(node: Node<'_>) -> bool {
    matches!(node.kind_str(), "assignment" | "operator_assignment")
}

/// `conditional?`.
fn is_conditional(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "if" | "unless"
            | "elsif"
            | "if_modifier"
            | "unless_modifier"
            | "conditional"
            | "while"
            | "until"
            | "while_modifier"
            | "until_modifier"
            | "case"
            | "case_match"
    )
}

fn is_single_line(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    context.source.line_column(node.start_byte()).0 == context.source.line_column(node.end_byte()).0
}

/// `multiline?`, read off the call as upstream's `send` spans it: the block it carries is a node
/// of its own there.
fn is_multiline(node: Node<'_>, arguments: Node<'_>, context: &RuleContext<'_>) -> bool {
    context.source.line_column(node.start_byte()).0
        != context.source.line_column(arguments.end_byte()).0
}

/// `inside_string_interpolation?`.
fn inside_string_interpolation(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = node.parent_of(context);
    while let Some(candidate) = current {
        if candidate.kind_str() == "interpolation" {
            return candidate
                .parent_of(context)
                .is_some_and(|parent| matches!(parent.kind_str(), "string" | "heredoc_body"));
        }
        current = candidate.parent_of(context);
    }
    false
}

/// The node's descendants, leaving out the block a call carries: upstream's parser wraps the block
/// around the call rather than hanging it off it, so a block is never a call's descendant there.
fn descendants<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Vec<Node<'tree>> {
    let mut out = Vec::new();
    let block = node.field("block").map(|block| block.id());
    for child in super::nodes::children(node) {
        if Some(child.id()) == block {
            continue;
        }
        out.push(child);
        collect(child, context, &mut out);
    }
    out
}

fn collect<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>, out: &mut Vec<Node<'tree>>) {
    // A heredoc's body is written after the statement that opened it, and upstream's parser holds
    // it inside the literal, so what it interpolates is part of the call that carries it.
    if node.kind_str() == "heredoc_beginning"
        && let Some(body) = crate::rules::send_node::heredoc_body(node, context)
    {
        out.push(body);
        collect(body, context, out);
    }
    for child in super::nodes::children(node) {
        out.push(child);
        collect(child, context, out);
    }
}

/// `node.parent` as upstream's parser holds it.
///
/// A call that carries a block is the block's first child there, so its parent is the block --
/// which is none of the kinds the predicates that ask for the parent directly are looking for.
fn raw_parent<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    match node.field("block") {
        Some(_) => None,
        None => parser_parent(node, context),
    }
}

/// The parent upstream's parser gives the node.
///
/// The grammar has three wrappers it has no node for -- the list an argument was written in, the
/// conditions of a `when`, and the superclass of a `class` -- and a statement list of its own where
/// upstream has one only when there are several statements to hold.
fn parser_parent<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    let mut current = node;
    loop {
        let parent = current.parent_of(context)?;
        match parent.kind_str() {
            "argument_list" | "pattern" | "superclass" => current = parent,
            "program" | "body_statement" | "block_body" | "then" | "else" | "ensure" | "do" => {
                if super::nodes::children(parent).len() > 1 {
                    return Some(parent);
                }
                current = parent;
            }
            _ => return Some(parent),
        }
    }
}

/// `a[b] = c` and `a.b = c`, which upstream's parser writes as sends of `:[]=` and `:b=`.
///
/// `call_as_argument_or_chain?` turns these down through `assigned_before?` -- what stands to the
/// right of the `=` is the value being assigned rather than a call the send was handed to -- so
/// only the predicate that has no such guard counts them.
fn is_setter_call(node: Node<'_>) -> bool {
    node.kind_str() == "assignment"
        && node
            .field("left")
            .is_some_and(|left| matches!(left.kind_str(), "element_reference" | "call"))
}

/// The kinds that stand where upstream's parser writes a `send`, a `yield` or a `super`.
///
/// An index, an operator written as an operator and a unary `!` are all calls there, so a call
/// handed to one of them is a call handed to a method.
fn is_send_like(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    match node.kind_str() {
        "call" | "yield" | "element_reference" | "super" => true,
        "unary" => node.field("operator").is_some_and(|operator| {
            matches!(
                context.source.node_text(operator),
                "!" | "~" | "-" | "+" | "not"
            )
        }),
        // `a && b` is an `and` there rather than a call, and so is the word it can be written as.
        "binary" => node.field("operator").is_some_and(|operator| {
            !matches!(
                context.source.node_text(operator),
                "&&" | "||" | "and" | "or"
            )
        }),
        _ => false,
    }
}

/// `node.parent.parent` for the block a call stands in, which upstream wraps around the call
/// rather than hanging off it.
///
/// The call that carries a block *is* the block's first child there, and a call that is the whole
/// of a block's body is its last, so both answer with what the block itself sits in.
fn block_parent<'tree>(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
    if node.field("block").is_some() {
        return parser_parent(node, context);
    }
    let parent = parser_parent(node, context)?;
    if !matches!(parent.kind_str(), "block" | "do_block") {
        return None;
    }
    parser_parent(parent.parent_of(context)?, context)
}

/// `node.parent&.any_block_type? ? node.parent.parent : node.parent`.
fn parent_beyond_block<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Option<Node<'tree>> {
    block_parent(node, context).or_else(|| parser_parent(node, context))
}

/// Whether the call opens a heredoc among its arguments.
fn carries_heredoc(node: Node<'_>) -> bool {
    fn opens(node: Node<'_>) -> bool {
        node.kind_str() == "heredoc_beginning"
            || super::nodes::children(node).into_iter().any(opens)
    }
    argument_list(node).is_some_and(opens)
}
