use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::ruby_version::RubyVersion;
use crate::rules::RuleContext;
use crate::rules::send_node::arguments;
use crate::rules::node_ext::NodeExt;

/// `minimum_target_ruby_version 2.6`: endless ranges are what the cop asks for.
const MINIMUM_VERSION: RubyVersion = RubyVersion::new(2, 6);

/// Beginless ranges arrived a release later, so `ary[nil..n]` is only rewritten from 2.7 on.
const BEGINLESS_VERSION: RubyVersion = RubyVersion::new(2, 7);

/// What a slice would become, and the part of it that has to go.
struct Rewrite {
    message: String,
    removal: Range<usize>,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    if context.target_ruby_version() < MINIMUM_VERSION {
        return;
    }
    for node in context.nodes_of_any(&["element_reference", "call"]) {
        let Some(slice) = Slice::read(node, context) else {
            continue;
        };
        let offense_range = slice.offense_range();
        let Some(rewrite) = slice.rewrite(context, &offense_range) else {
            continue;
        };
        // Making the range beginless or endless where the call has no parentheses would change
        // what the arguments bind to, so upstream leaves it alone.
        if rewrite.removal != offense_range && slice.unparenthesized_call() {
            continue;
        }
        offenses.push(
            context
                .offense(rewrite.message, offense_range)
                .corrected_by(Edit {
                    start: rewrite.removal.start,
                    end: rewrite.removal.end,
                    replacement: String::new(),
                    safe: true,
                }),
        );
    }
}

/// A call to `[]` with a single range argument, as either of the two ways it can be written.
struct Slice<'tree> {
    node: Node<'tree>,
    /// The `.` or `&.` of `ary.[](0..-1)`, absent for the `ary[0..-1]` form.
    dot: Option<Node<'tree>>,
    /// The `[...]` of the index form, which is what upstream's `loc.selector` covers there.
    selector: Range<usize>,
    range: Node<'tree>,
    parenthesized: bool,
}

impl<'tree> Slice<'tree> {
    fn read(node: Node<'tree>, context: &'tree RuleContext<'_>) -> Option<Self> {
        match node.kind_str() {
            "element_reference" => {
                // `ary[0..-1] = x` is a call to `:[]=`, which the cop does not restrict itself to.
                // `ary[0..-1] += x` still calls `:[]` and is reported.
                if is_assignment_target(node) {
                    return None;
                }
                let children = super::nodes::children_in(node, context);
                let [_, argument] = children.as_slice() else {
                    return None;
                };
                if argument.kind_str() != "range" {
                    return None;
                }
                let opening = super::conditional::token(node, &["["])?;
                Some(Self {
                    node,
                    dot: None,
                    selector: opening.start_byte()..node.end_byte(),
                    range: *argument,
                    parenthesized: true,
                })
            }
            "call" => {
                let method = node.field("method")?;
                if context.source.node_text(method) != "[]" {
                    return None;
                }
                let list = arguments(node);
                let [only] = list.as_slice() else {
                    return None;
                };
                let [argument] = only.parts() else {
                    return None;
                };
                if argument.kind_str() != "range" {
                    return None;
                }
                let dot = node.field("operator")?;
                Some(Self {
                    node,
                    dot: Some(dot),
                    selector: method.byte_range(),
                    range: *argument,
                    parenthesized: node
                        .field("arguments")
                        .is_some_and(|list| context.source.node_text(list).starts_with('(')),
                })
            }
            _ => None,
        }
    }

    /// `find_offense_range`: everything from the dot on, or the `[...]` when there is no dot.
    fn offense_range(&self) -> Range<usize> {
        match self.dot {
            Some(dot) => dot.start_byte()..self.node.end_byte(),
            None => self.selector.clone(),
        }
    }

    fn unparenthesized_call(&self) -> bool {
        self.dot.is_some() && !self.parenthesized
    }

    fn rewrite(&self, context: &RuleContext<'_>, offense_range: &Range<usize>) -> Option<Rewrite> {
        let begin = self.range.field("begin");
        let end = self.range.field("end");
        let exclusive = context
            .source
            .node_text(self.range.field("operator")?)
            == "...";
        // `{(int -1) nil}` for `..`, `nil` alone for `...`: `x...-1` stops one element earlier
        // than `x...` would, so only the `nil` literal is redundant there.
        let ends_the_slice = end.is_some_and(|node| {
            node.kind_str() == "nil" || (!exclusive && is_minus_one(node, context))
        });
        // `range_from_zero_till_minus_one?`: `0..-1`, `0..nil` and `0...nil`.
        let from_zero = begin.is_some_and(|node| is_integer(node, 0, context));
        if from_zero && ends_the_slice {
            return Some(Rewrite {
                message: format!(
                    "Remove the useless `{}`.",
                    context.source.slice(offense_range.clone())
                ),
                removal: offense_range.clone(),
            });
        }
        // `range_till_minus_one?`: `x..-1`, `x..nil` and `x...nil` with any beginning.
        if begin.is_some() && ends_the_slice {
            let prefer = format!(
                "{}{}",
                context.source.node_text(begin?),
                context
                    .source
                    .node_text(self.range.field("operator")?)
            );
            return Some(Rewrite {
                message: self.partial_message(context, &prefer, offense_range),
                removal: end?.byte_range(),
            });
        }
        // `range_from_zero?`: `nil..x`, which a beginless range says better.
        if context.target_ruby_version() >= BEGINLESS_VERSION
            && !exclusive
            && begin.is_some_and(|node| node.kind_str() == "nil")
            && end.is_some()
        {
            let prefer = format!(
                "{}{}",
                context
                    .source
                    .node_text(self.range.field("operator")?),
                context.source.node_text(end?)
            );
            return Some(Rewrite {
                message: self.partial_message(context, &prefer, offense_range),
                removal: begin?.byte_range(),
            });
        }
        None
    }

    /// `offense_message_for_partial_range`: the index form quotes the brackets on both sides, the
    /// dot form quotes the arguments alone.
    fn partial_message(
        &self,
        context: &RuleContext<'_>,
        prefer: &str,
        offense_range: &Range<usize>,
    ) -> String {
        match self.dot {
            Some(_) => format!(
                "Prefer `{prefer}` over `{}`.",
                context.source.node_text(self.range)
            ),
            None => format!(
                "Prefer `[{prefer}]` over `{}`.",
                context.source.slice(offense_range.clone())
            ),
        }
    }
}

/// Whether the node stands where a value is written rather than read, which upstream's parser
/// spells as a call to `:[]=`.
fn is_assignment_target(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind_str() {
        "assignment" => parent
            .field("left")
            .is_some_and(|left| left.id() == node.id()),
        "left_assignment_list" | "rest_assignment" | "destructured_left_assignment" => true,
        "for" => parent
            .field("pattern")
            .is_some_and(|pattern| pattern.id() == node.id()),
        _ => false,
    }
}

/// Whether the node is the literal `-1`, which upstream's parser folds into one `int` node.
fn is_minus_one(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "unary" {
        return false;
    }
    let (Some(operator), Some(operand)) = (
        node.field("operator"),
        node.field("operand"),
    ) else {
        return false;
    };
    context.source.node_text(operator) == "-"
        && operator.end_byte() == operand.start_byte()
        && is_integer(operand, 1, context)
}

fn is_integer(node: Node<'_>, value: i64, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "integer" {
        return false;
    }
    let text: String = context
        .source
        .node_text(node)
        .chars()
        .filter(|character| *character != '_')
        .collect();
    let (radix, digits) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (16, &text[2..]),
        Some("0b") => (2, &text[2..]),
        Some("0o") => (8, &text[2..]),
        Some("0d") => (10, &text[2..]),
        _ if text.len() > 1 && text.starts_with('0') => (8, &text[1..]),
        _ => (10, &text[..]),
    };
    i64::from_str_radix(digits, radix).is_ok_and(|parsed| parsed == value)
}
