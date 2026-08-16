use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::support::is_ruby_space_char;

/// One lambda, however it was written, in the shape upstream's `BlockNode` presents.
struct Lambda<'tree> {
    /// The whole literal, which is upstream's `block` node.
    block: Node<'tree>,
    /// What `node.send_node.source` reads: the `->` token, or the call up to its arguments.
    selector: std::ops::Range<usize>,
    /// The `(x)` of a literal or the `|x|` of a method call, when either was written.
    parameters: Option<Node<'tree>>,
    /// The `{ ... }` or `do ... end` that holds the body.
    body: Node<'tree>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "line_count_dependent".to_owned());

    for node in context.nodes_of_any(&["call", "lambda"]) {
        let Some(lambda) = Lambda::of(context, node) else {
            continue;
        };
        let selector = &context.source.text()[lambda.selector.clone()];
        let multiline = lambda.block.start_position().row != lambda.block.end_position().row;
        if selector != offending_selector(&style, multiline) {
            continue;
        }

        let message = match selector {
            "->" => format!(
                "Use the `lambda` method for {} lambdas.",
                modifier(&style, multiline)
            ),
            _ => format!(
                "Use the `-> {{ ... }}` lambda literal syntax for {} lambdas.",
                modifier(&style, multiline)
            ),
        };
        let offense = context.offense(message, lambda.selector.clone());
        offenses.push(match selector {
            "->" => literal_to_method(context, &lambda, offense),
            _ => method_to_literal(context, &lambda, offense),
        });
    }
}

/// `OFFENDING_SELECTORS`: the spelling this style rejects for a lambda of this length.
fn offending_selector(style: &str, multiline: bool) -> &'static str {
    match (style, multiline) {
        ("lambda", _) => "->",
        ("literal", _) => "lambda",
        (_, true) => "->",
        (_, false) => "lambda",
    }
}

fn modifier(style: &str, multiline: bool) -> &'static str {
    match (style, multiline) {
        ("line_count_dependent", true) => "multiline",
        ("line_count_dependent", false) => "single line",
        _ => "all",
    }
}

impl<'tree> Lambda<'tree> {
    fn of(context: &RuleContext<'_>, node: Node<'tree>) -> Option<Self> {
        match node.kind_str() {
            // `->(x) { ... }`, which the grammar spells as one node.
            "lambda" => {
                let arrow = node.child(0)?;
                Some(Self {
                    block: node,
                    selector: arrow.byte_range(),
                    parameters: node.field("parameters"),
                    body: node.field("body")?,
                })
            }
            // `lambda { ... }`, where the call and its block are one `block` node upstream.
            _ => {
                let body = node.field("block")?;
                let method = node.field("method")?;
                // `lambda?`: the method the block hangs off has to be `lambda`.
                if context.source.node_text(method) != "lambda" {
                    return None;
                }
                // `send_node.source` stops where the block starts, receiver and arguments included.
                let selector = node.start_byte()..body.prev_sibling()?.end_byte();
                Some(Self {
                    block: node,
                    selector,
                    parameters: body.field("parameters"),
                    body,
                })
            }
        }
    }

    fn block_begin(&self) -> Option<Node<'tree>> {
        self.body.child(0)
    }

    fn block_end(&self) -> Option<Node<'tree>> {
        self.body
            .child(self.body.child_count().saturating_sub(1) as u32)
    }

    fn braces(&self) -> bool {
        self.body.kind_str() == "block"
    }

    /// `arguments.children`, split the way `lambda_arg_string` needs them: a block-local goes
    /// after a `;` rather than becoming another parameter.
    fn argument_list(&self, context: &RuleContext<'_>) -> String {
        let Some(parameters) = self.parameters else {
            return String::new();
        };
        let mut cursor = parameters.walk();
        let mut regular = Vec::new();
        let mut shadow = Vec::new();
        if cursor.goto_first_child() {
            loop {
                let child = cursor.node();
                if child.is_named() && super::nodes::is_child(child) {
                    let text = context.source.node_text(child);
                    match cursor.field_name() {
                        Some("locals") => shadow.push(text),
                        _ => regular.push(text),
                    }
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
        let mut joined = regular.join(", ");
        if !shadow.is_empty() {
            joined.push_str(&format!("; {}", shadow.join(", ")));
        }
        joined
    }

    /// `node.arguments?`: parameters were written and at least one of them names something.
    fn has_arguments(&self) -> bool {
        self.parameters
            .is_some_and(|parameters| !super::nodes::children(parameters).is_empty())
    }
}

/// `autocorrect_method_to_literal`.
fn method_to_literal(context: &RuleContext<'_>, lambda: &Lambda<'_>, offense: Offense) -> Offense {
    let mut edits = vec![Edit {
        start: lambda.selector.start,
        end: lambda.selector.end,
        replacement: "->".to_owned(),
        safe: true,
    }];
    if lambda.has_arguments() {
        let (Some(begin), Some(parameters)) = (lambda.block_begin(), lambda.parameters) else {
            return offense.corrected_by_all(edits);
        };
        edits.push(Edit {
            start: lambda.selector.end,
            end: lambda.selector.end,
            replacement: format!("({})", lambda.argument_list(context)),
            safe: true,
        });
        // `arguments_with_whitespace`: from just inside the block to the closing `|`.
        edits.push(Edit {
            start: begin.end_byte(),
            end: parameters.end_byte(),
            replacement: String::new(),
            safe: true,
        });
    }
    offense.corrected_by_all(edits)
}

/// `LambdaLiteralToMethodCorrector#call`.
fn literal_to_method(context: &RuleContext<'_>, lambda: &Lambda<'_>, offense: Offense) -> Offense {
    let (Some(begin), Some(end)) = (lambda.block_begin(), lambda.block_end()) else {
        return offense;
    };
    let text = context.source.text();
    let mut edits = Vec::new();
    let parenthesized = lambda
        .parameters
        .is_some_and(|parameters| context.source.node_text(parameters).starts_with('('));

    // `remove_unparenthesized_whitespace`: `-> x do` loses the space either side of `x`.
    if let Some(parameters) = lambda.parameters
        && lambda.has_arguments()
        && !parenthesized
    {
        edits.push(Edit {
            start: lambda.selector.end,
            end: parameters.start_byte(),
            replacement: String::new(),
            safe: true,
        });
        // One space is kept between the parameters and the block.
        if begin.start_byte() > parameters.end_byte() + 1 {
            edits.push(Edit {
                start: parameters.end_byte() + 1,
                end: begin.start_byte(),
                replacement: String::new(),
                safe: true,
            });
        }
    }

    // `insert_separating_space`: `->do` must not become `lambdado`.
    if needs_separating_space(lambda, begin, parenthesized) {
        edits.push(Edit {
            start: begin.start_byte(),
            end: begin.start_byte(),
            replacement: " ".to_owned(),
            safe: true,
        });
    }
    // `remove_arguments`: the parameter list moves inside the block.
    if let Some(parameters) = lambda.parameters {
        edits.push(Edit {
            start: parameters.start_byte(),
            end: parameters.end_byte(),
            replacement: String::new(),
            safe: true,
        });
    }
    edits.push(Edit {
        start: lambda.selector.start,
        end: lambda.selector.end,
        replacement: "lambda".to_owned(),
        safe: true,
    });

    // `replace_delimiters`: `do ... end` handed to a call without parentheses has to become braces
    // so that it still binds to that call.
    if !lambda.braces() && argument_of_unparenthesized_call(context, lambda.block) {
        let after = text[begin.end_byte()..].chars().next();
        if !after.is_some_and(is_ruby_space_char) {
            edits.push(Edit {
                start: begin.end_byte(),
                end: begin.end_byte(),
                replacement: " ".to_owned(),
                safe: true,
            });
        }
        edits.push(Edit {
            start: begin.start_byte(),
            end: begin.end_byte(),
            replacement: "{".to_owned(),
            safe: true,
        });
        edits.push(Edit {
            start: end.start_byte(),
            end: end.end_byte(),
            replacement: "}".to_owned(),
            safe: true,
        });
    }

    // `insert_arguments`.
    if lambda.has_arguments() {
        edits.push(Edit {
            start: begin.end_byte(),
            end: begin.end_byte(),
            replacement: format!(" |{}|", lambda.argument_list(context)),
            safe: true,
        });
    }
    // Every insertion here hangs off the block's opening delimiter rather than off the reported
    // range, which is the selector.
    offense
        .corrected_by_all(edits)
        .corrections_anchored_at(begin.byte_range())
}

/// `needs_separating_space?`.
fn needs_separating_space(lambda: &Lambda<'_>, begin: Node<'_>, parenthesized: bool) -> bool {
    if begin.start_byte() == lambda.selector.end {
        return true;
    }
    parenthesized
        && lambda.parameters.is_some_and(|parameters| {
            begin.start_byte() == parameters.end_byte()
                && lambda.selector.end == parameters.start_byte()
        })
}

/// `arg_to_unparenthesized_call?`.
fn argument_of_unparenthesized_call(context: &RuleContext<'_>, block: Node<'_>) -> bool {
    let Some(mut parent) = block.parent_of(context) else {
        return false;
    };
    // A lambda written as a hash value stands where the hash stands. The grammar leaves the pairs
    // of a trailing hash argument as siblings of the other arguments, so the hash is only there to
    // step over when the braces were written.
    if parent.kind_str() == "pair" {
        let Some(above) = parent.parent_of(context) else {
            return false;
        };
        parent = above;
        if parent.kind_str() == "hash" {
            let Some(above) = parent.parent_of(context) else {
                return false;
            };
            parent = above;
        }
    }
    // Only an argument counts: a lambda the call hangs off is its receiver, and a call written
    // with parentheses keeps its argument whatever delimiters the block uses.
    match parent.kind_str() {
        "argument_list" => !parent.child(0).is_some_and(|open| open.kind_str() == "("),
        // An operator call is a send with no parentheses at all, so its right-hand operand is an
        // argument of an unparenthesized call.
        "binary" => {
            parent
                .field("operator")
                .is_some_and(|operator| {
                    super::nodes::is_operator_method(context.source.node_text(operator))
                })
                && parent
                    .field("right")
                    .is_some_and(|right| right.id() == block.id())
        }
        // `a[-> do end]` is a call to `:[]`, whose `loc.begin` is a bracket rather than a
        // parenthesis.
        "element_reference" => parent
            .field("object")
            .is_some_and(|object| object.id() != block.id()),
        // `a.b = -> do end` and `a[b] = -> do end` are calls to `:b=` and `:[]=` upstream, so the
        // value is one of their arguments. A plain variable assignment is not a call at all.
        "assignment" => {
            parent
                .field("right")
                .is_some_and(|right| right.id() == block.id())
                && parent
                    .field("left")
                    .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference"))
        }
        _ => false,
    }
}

