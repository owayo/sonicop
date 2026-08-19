//! `Style/BlockDelimiters`: braces for a block written on one line, `do...end` for one that is not.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG_MULTILINE: &str = "Avoid using `{...}` for multi-line blocks.";
const MSG_SINGLE_LINE: &str = "Prefer `{...}` over `do...end` for single-line blocks.";
const BRACES_REQUIRED: &str = "Brace delimiters `{...}` required for '%<method_name>s' method.";

/// `COMPARISON_OPERATORS`, which `assignment_method?` exempts from its `=` test.
const COMPARISON_OPERATORS: &[&str] = &["==", "===", "!=", "<=", ">=", ">", "<"];

/// Operators the grammar writes as `binary` and upstream as a `send`. The logical ones become
/// `and`/`or` nodes there and are not calls at all.
const LOGICAL_OPERATORS: &[&str] = &["&&", "||", "and", "or"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let cop = Cop {
        context,
        style: context
            .setting::<String>("EnforcedStyle")
            .unwrap_or_else(|| "line_count_based".to_owned()),
        braces_required_methods: context.setting("BracesRequiredMethods").unwrap_or_default(),
        functional_methods: context.setting("FunctionalMethods").unwrap_or_else(|| {
            ["let", "let!", "subject", "watch"]
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        }),
        procedural_methods: context.setting("ProceduralMethods").unwrap_or_else(|| {
            [
                "benchmark",
                "bm",
                "bmbm",
                "create",
                "each_with_object",
                "measure",
                "new",
                "realtime",
                "tap",
                "with_object",
            ]
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
        }),
        procedural_oneliners_may_have_braces: context
            .setting("AllowBracesOnProceduralOneLiners")
            .unwrap_or(false),
        allowed_methods: context.setting("AllowedMethods").unwrap_or_else(|| {
            ["lambda", "proc", "it"]
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        }),
        allowed_patterns: context
            .setting::<Vec<String>>("AllowedPatterns")
            .unwrap_or_default()
            .iter()
            .filter_map(|pattern| regex::Regex::new(pattern).ok())
            .collect(),
    };

    // `on_send`: a block standing in an argument list without parentheses binds to the inner call,
    // so neither delimiter can be swapped for the other.
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for node in context.nodes_of_any(&["call", "method_call", "element_reference", "binary"]) {
        cop.collect_bound_blocks(node, &mut ignored);
    }

    // Upstream's `block` node reaches back over the call it hangs off, so the walk meets an
    // enclosing block before the one written inside its receiver -- which the grammar, whose block
    // starts at the brace, would visit the other way round.
    let mut blocks: Vec<Block<'_>> = context
        .nodes_of_any(&["block", "do_block"])
        .filter_map(|node| Block::new(context, node))
        .collect();
    blocks.sort_by_key(|block| (block.range().start, std::cmp::Reverse(block.range().end)));

    for block in blocks {
        let range = block.range();
        if ignored
            .iter()
            .any(|ignored| ignored.start <= range.start && ignored.end >= range.end)
        {
            continue;
        }
        if cop.proper_block_style(&block) {
            continue;
        }
        let offense = context.offense(cop.message(&block), block.begin.byte_range());
        offenses.push(match cop.correction_would_break_code(&block) {
            true => offense,
            false => offense.corrected_by_all(cop.autocorrect(&block)),
        });
        // `ignore_node`: the rewrite covers the whole block, so a block written inside it is left
        // for the next pass rather than rewritten underneath this one.
        ignored.push(range);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    style: String,
    braces_required_methods: Vec<String>,
    allowed_methods: Vec<String>,
    allowed_patterns: Vec<regex::Regex>,
    /// `FunctionalMethods`: the ones whose block always yields a value, so braces suit them.
    functional_methods: Vec<String>,
    /// `ProceduralMethods`: the ones whose block is run for its effect, so `do ... end` suits.
    procedural_methods: Vec<String>,
    /// `AllowBracesOnProceduralOneLiners`.
    procedural_oneliners_may_have_braces: bool,
}

/// One block: its delimiters, the call it hangs off, and the body between them.
struct Block<'t> {
    node: Node<'t>,
    call: Node<'t>,
    method: String,
    begin: Node<'t>,
    end: Node<'t>,
    braces: bool,
    multiline: bool,
}

impl<'t> Block<'t> {
    fn new(context: &RuleContext<'_>, node: Node<'t>) -> Option<Self> {
        let braces = node.kind_str() == "block";
        let call = node.parent()?;
        // `-> { }` is a block whose call is `lambda` upstream, however the arrow is written.
        let method = match call.kind_str() {
            "lambda" => "lambda".to_owned(),
            _ => call
                .field("method")
                .map(|method| context.source.node_text(method).to_owned())
                .unwrap_or_default(),
        };
        let mut cursor = node.walk();
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        let begin = *children
            .iter()
            .find(|child| !child.is_named() && matches!(child.kind_str(), "{" | "do"))?;
        let end = *children
            .iter()
            .rfind(|child| !child.is_named() && matches!(child.kind_str(), "}" | "end"))?;
        Some(Self {
            node,
            call,
            method,
            begin,
            end,
            braces,
            multiline: node.start_position().row != node.end_position().row,
        })
    }

    fn body(&self) -> Option<Node<'t>> {
        self.node.field("body")
    }

    /// The span upstream's `block` node covers, which reaches back to the start of its call.
    fn range(&self) -> Range<usize> {
        self.call.start_byte()..self.node.end_byte()
    }
}

impl Cop<'_, '_> {
    fn source(&self, node: Node<'_>) -> &str {
        self.context.source.node_text(node)
    }

    /// `get_blocks` reached from `on_send`: the blocks an unparenthesized argument list binds.
    fn collect_bound_blocks(&self, node: Node<'_>, ignored: &mut Vec<Range<usize>>) {
        let arguments = match node.kind_str() {
            "binary" => {
                let Some(operator) = node.field("operator") else {
                    return;
                };
                if LOGICAL_OPERATORS.contains(&self.source(operator)) {
                    return;
                }
                let Some(right) = node.field("right") else {
                    return;
                };
                // `single_argument_operator_method?`: an operator taking exactly one block still
                // binds it the same way whichever delimiter is written.
                if is_block_bearing_call(right) {
                    return;
                }
                vec![right]
            }
            "element_reference" => {
                let mut arguments = super::nodes::children(node);
                if node.field("object").is_some() && !arguments.is_empty() {
                    arguments.remove(0);
                }
                // `single_argument_operator_method?`: `[]` is an operator method, so a lone block
                // argument binds the same way whichever delimiter is written.
                if arguments.len() == 1 && is_block_bearing_call(arguments[0]) {
                    return;
                }
                arguments
            }
            _ => {
                let Some(list) = node.field("arguments") else {
                    return;
                };
                // `node.parenthesized?`: the parenthesis has to be the call's own, not one that
                // happens to open its first argument (`foo (a).b`).
                if list
                    .child(0)
                    .is_some_and(|first| !first.is_named() && first.kind_str() == "(")
                {
                    return;
                }
                // `node.assignment_method?`.
                if node.field("method").is_some_and(|method| {
                    let name = self.source(method);
                    name.ends_with('=') && !COMPARISON_OPERATORS.contains(&name)
                }) {
                    return;
                }
                super::nodes::children(list)
            }
        };
        for argument in arguments {
            get_blocks(argument, ignored);
        }
    }

    fn message(&self, block: &Block<'_>) -> String {
        if self.braces_required_method(&block.method) {
            return BRACES_REQUIRED.replace("%<method_name>s", &block.method);
        }
        match self.style.as_str() {
            "always_braces" => "Prefer `{...}` over `do...end` for blocks.".to_owned(),
            "semantic" => match block.braces {
                true => "Prefer `do...end` over `{...}` for procedural blocks.".to_owned(),
                false => "Prefer `{...}` over `do...end` for functional blocks.".to_owned(),
            },
            "braces_for_chaining" => match block.multiline {
                true => match chained(block.call) {
                    true => {
                        "Prefer `{...}` over `do...end` for multi-line chained blocks.".to_owned()
                    }
                    false => "Prefer `do...end` for multi-line blocks without chaining.".to_owned(),
                },
                false => MSG_SINGLE_LINE.to_owned(),
            },
            _ => match block.multiline {
                true => MSG_MULTILINE.to_owned(),
                false => MSG_SINGLE_LINE.to_owned(),
            },
        }
    }

    fn proper_block_style(&self, block: &Block<'_>) -> bool {
        if self.require_do_end(block) {
            return true;
        }
        if self.allowed_methods.contains(&block.method)
            || self
                .allowed_patterns
                .iter()
                .any(|pattern| pattern.is_match(&block.method))
        {
            return true;
        }
        if self.braces_required_method(&block.method) {
            return block.braces;
        }
        match self.style.as_str() {
            "always_braces" => block.braces,
            "braces_for_chaining" => match block.multiline {
                true => match chained(block.call) {
                    true => block.braces,
                    false => !block.braces,
                },
                false => block.braces,
            },
            "semantic" => self.semantic_block_style(block),
            // `line_count_based_block_style?`.
            _ => block.multiline != block.braces,
        }
    }

    /// `semantic_block_style?`: braces mark a block whose value is used, `do ... end` one that is
    /// run for its effect. Which one a block is comes from the method it hangs off and from where
    /// the block itself sits -- a block whose result nothing reads is procedural whatever it does.
    fn semantic_block_style(&self, block: &Block<'_>) -> bool {
        if block.braces {
            self.functional_methods.contains(&block.method)
                || self.functional_block(block)
                || (self.procedural_oneliners_may_have_braces && !block.multiline)
        } else {
            self.procedural_methods.contains(&block.method) || !self.return_value_used(block.call)
        }
    }

    /// `functional_block?`.
    fn functional_block(&self, block: &Block<'_>) -> bool {
        self.return_value_used(block.call) || self.return_value_of_scope(block.call)
    }

    /// `return_value_used?`: the block feeds an assignment or another call.
    ///
    /// The node walked from is upstream's `block`, which the grammar spells as the call the braces
    /// hang off, so the parent chain reads the same from there.
    fn return_value_used(&self, node: Node<'_>) -> bool {
        let Some(parent) = node.parent_of(self.context) else {
            return false;
        };
        match parent.kind_str() {
            // `node.parent.begin_type?`: parentheses around the block, which pass the question on.
            "parenthesized_statements" => self.return_value_used(parent),
            "assignment" | "operator_assignment" => true,
            // `call_type?`: Parser folds arguments, indexing and non-keyword operators into
            // `send` nodes. tree-sitter keeps the argument-list wrapper and operator forms.
            "call" | "method_call" | "argument_list" | "element_reference" | "unary" => true,
            // Ordinary operators are `send` nodes to Parser, while `&&` / `||` / `and` / `or`
            // remain keyword operators. The latter make braces functional through
            // `return_value_of_scope?`, but do not make a `do...end` block's value directly used.
            "binary" => parent.field("operator").is_some_and(|operator| {
                !LOGICAL_OPERATORS.contains(&self.context.source.node_text(operator))
            }),
            _ => false,
        }
    }

    /// `return_value_of_scope?`: the block sits where a value is read off -- a condition, an
    /// operand, an element, or the last expression of whatever encloses it.
    fn return_value_of_scope(&self, node: Node<'_>) -> bool {
        let Some(parent) = node.parent_of(self.context) else {
            return false;
        };
        if matches!(
            parent.kind_str(),
            "if" | "unless"
                | "elsif"
                | "if_modifier"
                | "unless_modifier"
                | "while"
                | "until"
                | "while_modifier"
                | "until_modifier"
                | "case"
                | "case_match"
                | "conditional"
                | "array"
                | "range"
        ) {
            return true;
        }
        // `operator_keyword?`: `and`, `or`, `not`.
        if parent.kind_str() == "binary"
            && parent.field("operator").is_some_and(|operator| {
                LOGICAL_OPERATORS.contains(&&self.context.source.text()[operator.byte_range()])
            })
        {
            return true;
        }
        if parent.kind_str() == "unary" {
            return true;
        }
        // `parent.children.last == node`: the block is the last thing the parent holds, so its
        // value is the parent's.
        let children = super::nodes::children(parent);
        // Parser represents a method-level rescue as a `rescue` whose protected body is a
        // separate `begin`. tree-sitter appends the rescue clauses to one `body_statement`; the
        // last expression before the first clause is the value of that virtual begin.
        let value_children = match parent.kind_str() {
            "body_statement" => {
                &children[..children
                    .iter()
                    .position(|child| matches!(child.kind_str(), "rescue" | "else" | "ensure"))
                    .unwrap_or(children.len())]
            }
            _ => children.as_slice(),
        };
        value_children
            .last()
            .is_some_and(|last| last.id() == node.id())
    }

    fn braces_required_method(&self, method: &str) -> bool {
        self.braces_required_methods
            .iter()
            .any(|name| name == method)
    }

    /// `require_do_end?`: `ensure` and a block-level `rescue` cannot be written inside braces.
    fn require_do_end(&self, block: &Block<'_>) -> bool {
        if block.braces || block.multiline {
            return false;
        }
        let Some(body) = block.body() else {
            return false;
        };
        let clauses = super::nodes::children(body);
        if clauses.iter().any(|child| child.kind_str() == "ensure") {
            return true;
        }
        let Some(rescue) = clauses.iter().find(|child| child.kind_str() == "rescue") else {
            return false;
        };
        // `modifier_rescue?`: only a bare `expr rescue expr` may keep its braces.
        //
        // `return false if rescue_node.body.nil?` is the part that is easy to lose: a `rescue`
        // with nothing in front of it (`foo do rescue; bar end`) protects no expression, so it is
        // not the modifier form and the block has to stay `do ... end`. The protected part is not
        // a child of the `rescue` node here -- it is whatever statements precede it -- so the
        // question becomes "is the `rescue` the first thing in the body".
        let protects_something = clauses
            .first()
            .is_some_and(|first| first.id() != rescue.id());
        let bare = protects_something
            && clauses
                .iter()
                .filter(|child| child.kind_str() == "rescue")
                .count()
                == 1
            && !clauses.iter().any(|child| child.kind_str() == "else")
            && rescue.field("exceptions").is_none()
            && rescue.field("variable").is_none();
        !bare
    }

    /// `correction_would_break_code?`: swapping `do...end` for braces would rebind the block to
    /// the last argument of an unparenthesized call.
    fn correction_would_break_code(&self, block: &Block<'_>) -> bool {
        if block.braces {
            return false;
        }
        block.call.field("arguments").is_some_and(|list| {
            !list
                .child(0)
                .is_some_and(|first| !first.is_named() && first.kind_str() == "(")
        })
    }

    fn autocorrect(&self, block: &Block<'_>) -> Vec<Edit> {
        match block.braces {
            true => self.replace_braces_with_do_end(block),
            false => self.replace_do_end_with_braces(block),
        }
    }

    fn replace_braces_with_do_end(&self, block: &Block<'_>) -> Vec<Edit> {
        let text = self.context.source.text();
        let mut edits = Vec::new();
        if !whitespace_at(text, block.begin.start_byte().wrapping_sub(1)) {
            edits.push(insert(block.begin.start_byte(), " "));
        }
        if !whitespace_at(text, block.end.start_byte().wrapping_sub(1)) {
            edits.push(insert(block.end.start_byte(), " "));
        }
        if !whitespace_at(text, block.begin.start_byte() + 1) {
            edits.push(insert(block.begin.end_byte(), " "));
        }
        edits.push(replace(block.begin.byte_range(), "do"));
        if let Some(comment) = self.comment_on_line(block.end.end_position().row + 1) {
            edits.extend(self.move_comment_before_block(block, comment));
        }
        edits.push(replace(block.end.byte_range(), "end"));
        edits
    }

    fn replace_do_end_with_braces(&self, block: &Block<'_>) -> Vec<Edit> {
        let text = self.context.source.text();
        let mut edits = Vec::new();
        if !whitespace_at(text, block.begin.start_byte() + 2) {
            edits.push(insert(block.begin.end_byte(), " "));
        }
        edits.push(replace(block.begin.byte_range(), "{"));
        edits.push(replace(block.end.byte_range(), "}"));
        // `begin_required?`: a protected body needs its own `begin` once the keywords are gone.
        if block.multiline
            && let Some(body) = block.body()
            && super::nodes::children(body)
                .iter()
                .any(|child| matches!(child.kind_str(), "rescue" | "ensure"))
        {
            edits.push(insert(body.start_byte(), "begin\n"));
            edits.push(insert(body.end_byte(), "\nend"));
        }
        edits
    }

    /// `move_comment_before_block`: a comment closing the block's last line has to move above the
    /// block, or the `end` that replaces the brace would land behind it.
    fn move_comment_before_block(&self, block: &Block<'_>, comment: Range<usize>) -> Vec<Edit> {
        let text = self.context.source.text();
        let anchor = match chained(block.call) {
            true => end_of_chain(block.call).end_byte(),
            false => block.end.end_byte(),
        };
        // `source_range_before_comment`: the code between the block and the comment stays, the
        // blanks in front of the comment do not.
        let between = &text[anchor.min(comment.start)..comment.start];
        let pre_comment = comment.start - (between.len() - between.trim_end().len());
        // `range_with_surrounding_space(side: :right)`: the blanks after the comment, then the
        // line breaks, go with it.
        let mut after = comment.end;
        while after < text.len() && matches!(text.as_bytes()[after], b' ' | b'\t') {
            after += 1;
        }
        while after < text.len() && text.as_bytes()[after] == b'\n' {
            after += 1;
        }
        vec![
            remove(comment.start..after),
            remove(pre_comment..comment.start),
            insert(pre_comment, "\n"),
            insert(
                block.range().start,
                &format!("{}\n", &text[comment.clone()]),
            ),
        ]
    }

    fn comment_on_line(&self, line: usize) -> Option<Range<usize>> {
        self.context
            .comment_ranges()
            .iter()
            .find(|range| self.context.source.line_column(range.start).0 == line)
            .cloned()
    }
}

/// `get_blocks`: the blocks that would change meaning, reached through the shapes an argument can
/// take.
fn get_blocks(node: Node<'_>, out: &mut Vec<Range<usize>>) {
    match node.kind_str() {
        // A lambda literal is a `block` upstream, whose call is the implicit `lambda`.
        "lambda" => out.push(node.byte_range()),
        "call" | "method_call" => {
            if node.field("block").is_some() {
                // The call and its block are one `block` node upstream, which is what gets ignored.
                out.push(node.byte_range());
                return;
            }
            if let Some(receiver) = node.field("receiver") {
                get_blocks(receiver, out);
            }
            if let Some(list) = node.field("arguments") {
                for argument in super::nodes::children(list) {
                    get_blocks(argument, out);
                }
            }
        }
        // A braced hash cannot hide a block that would rebind; a bare pair list can.
        "pair" => {
            for child in super::nodes::children(node) {
                get_blocks(child, out);
            }
        }
        // Upstream's `when :send, :csend` covers every operator spelling, because its parser
        // writes them all as `send`. `puts [0] + [1,2,3].map { ... }, 1` reaches the block through
        // the `+`, and walking only `call` leaves it visible to the cop -- which then offers to
        // turn `{ }` into `do ... end` and change what the block binds to.
        "binary" | "element_reference" | "unary" | "assignment" | "operator_assignment" => {
            for child in super::nodes::children(node) {
                get_blocks(child, out);
            }
        }
        _ => {}
    }
}

/// Whether the call carrying the block is itself the receiver of another one.
fn chained(call: Node<'_>) -> bool {
    call.parent().is_some_and(|parent| {
        matches!(parent.kind_str(), "call" | "method_call")
            && parent
                .field("receiver")
                .is_some_and(|receiver| receiver.id() == call.id())
    })
}

fn end_of_chain(call: Node<'_>) -> Node<'_> {
    match chained(call) {
        true => match call.parent() {
            Some(parent) => end_of_chain(parent),
            None => call,
        },
        false => call,
    }
}

/// Whether the node is a call carrying a block, which is one `block` node upstream.
fn is_block_bearing_call(node: Node<'_>) -> bool {
    node.kind_str() == "lambda"
        || (matches!(node.kind_str(), "call" | "method_call") && node.field("block").is_some())
}

fn whitespace_at(text: &str, offset: usize) -> bool {
    text.as_bytes()
        .get(offset)
        .is_some_and(|byte| byte.is_ascii_whitespace())
}

fn insert(offset: usize, replacement: &str) -> Edit {
    Edit {
        start: offset,
        end: offset,
        replacement: replacement.to_owned(),
        safe: true,
    }
}

fn replace(range: Range<usize>, replacement: &str) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: replacement.to_owned(),
        safe: true,
    }
}

fn remove(range: Range<usize>) -> Edit {
    Edit {
        start: range.start,
        end: range.end,
        replacement: String::new(),
        safe: true,
    }
}
