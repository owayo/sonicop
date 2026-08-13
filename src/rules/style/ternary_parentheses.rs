use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const MSG: &str = "%s parentheses for ternary conditions.";
const MSG_COMPLEX: &str = "%s parentheses for ternary expressions with complex conditions.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "require_no_parentheses".to_owned());
    let require_parentheses = style == "require_parentheses";
    let when_complex = style == "require_parentheses_when_complex";
    let allow_safe_assignment: bool = context.setting("AllowSafeAssignment").unwrap_or(true);

    for node in context.nodes_of("conditional") {
        let Some(condition) = node.field("condition") else {
            continue;
        };
        if only_closing_parenthesis_is_last_line(context, condition)
            || condition_as_parenthesized_one_line_pattern_matching(condition)
        {
            continue;
        }
        let parenthesized = condition.kind_str() == "parenthesized_statements";
        let safe_assignment = parenthesized && is_safe_assignment(context, condition);
        // `offense?`.
        let offending = if parenthesized_modifier_condition(condition) {
            false
        } else if safe_assignment {
            !allow_safe_assignment
        } else if when_complex {
            complex_condition(context, condition) != parenthesized
        } else {
            require_parentheses != parenthesized
        };
        if !offending {
            continue;
        }
        let command = match (when_complex, parenthesized, require_parentheses) {
            (true, true, _) => "Only use",
            (true, false, _) => "Use",
            (false, _, true) => "Use",
            (false, _, false) => "Omit",
        };
        let template = if when_complex { MSG_COMPLEX } else { MSG };
        let message = template.replacen("%s", command, 1);
        let offense = context.offense(message, node.byte_range());
        // `autocorrect` returns before touching the corrector when the parentheses carry meaning,
        // which leaves the offense with the `:unsupported` status rather than a correction.
        if parenthesized && (safe_assignment || unsafe_autocorrect(context, condition)) {
            offenses.push(offense);
            continue;
        }
        if parenthesized {
            offenses.push(offense.corrected_by_all(correct_parenthesized(context, condition)));
        } else {
            offenses.push(
                offense
                    .corrected_by_all(wrap_in_parentheses(condition))
                    // `corrector.wrap(condition, '(', ')')` is one action over the condition, so
                    // both insertions have to hang off that range rather than off the ternary.
                    .corrections_anchored_at(condition.byte_range()),
            );
        }
    }
}

/// `only_closing_parenthesis_is_last_line?`: a condition whose last line is nothing but `)` is
/// spread over lines on purpose, and pulling the parentheses off it would run the lines together.
fn only_closing_parenthesis_is_last_line(context: &RuleContext<'_>, condition: Node<'_>) -> bool {
    let source = context.source.node_text(condition);
    let mut lines: Vec<&str> = source.split('\n').collect();
    // `String#split` drops the empty strings a trailing separator leaves behind.
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines.last() == Some(&")")
}

/// `condition_as_parenthesized_one_line_pattern_matching?`: `(foo in Integer) ? a : b` needs its
/// parentheses, since `in` binds looser than the ternary.
fn condition_as_parenthesized_one_line_pattern_matching(condition: Node<'_>) -> bool {
    condition.kind_str() == "parenthesized_statements"
        && super::nodes::children(condition)
            .first()
            // `match_pattern_p_type?`: the `in` form. The `=>` form is `match_pattern`, which the
            // cop only looks for below Ruby 3.0, where the grammar had `in` build that node.
            .is_some_and(|first| first.kind_str() == "test_pattern")
}

/// `parenthesized_modifier_condition?`: `(a if b) ? x : y` needs its parentheses, since a modifier
/// `if` cannot be the condition of a ternary on its own.
fn parenthesized_modifier_condition(condition: Node<'_>) -> bool {
    condition.kind_str() == "parenthesized_statements"
        && super::nodes::children(condition)
            .first()
            // `inner&.if_type? && inner.modifier_form?`: a `while` written the same way is not
            // spared, since the cop only asks about an `if`.
            .is_some_and(|first| matches!(first.kind_str(), "if_modifier" | "unless_modifier"))
}

/// `safe_assignment?`: `(begin {equals_asgn? #setter_method?})`, the parenthesized assignment that
/// says the assignment was meant.
fn is_safe_assignment(context: &RuleContext<'_>, condition: Node<'_>) -> bool {
    let children = super::nodes::children(condition);
    let [only] = children.as_slice() else {
        return false;
    };
    only.kind_str() == "assignment" && !super::nodes::is_match_assignment(*only, context.source.text())
}

/// `complex_condition?`: a parenthesized condition is complex when anything written inside it is.
fn complex_condition(context: &RuleContext<'_>, condition: Node<'_>) -> bool {
    if condition.kind_str() == "parenthesized_statements" {
        return super::nodes::children(condition)
            .into_iter()
            .any(|child| complex_condition(context, child));
    }
    !non_complex_expression(context, condition)
}

/// `non_complex_expression?`: a variable, a constant, `defined?`, `yield`, or a call by a name that
/// is not an operator -- `[]` excepted, which reads as an index rather than as an operator.
fn non_complex_expression(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        // `VARIABLE_TYPES`. A bare name is an `lvar` where one is in scope and a receiverless
        // `send` where none is, and neither counts as complex.
        "instance_variable" | "global_variable" | "class_variable" | "identifier" => true,
        "constant" | "scope_resolution" | "yield" => true,
        // `a[0]` is `(send a :[] 0)`, the one operator method the cop lets through.
        "element_reference" => true,
        "unary" => node
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "defined?"),
        // A call carrying a block is a `block` node upstream rather than a `send`, and no `block`
        // is ever simple.
        "call" if node.field("block").is_none() => {
            node.field("method").is_some_and(|method| {
                !super::nodes::is_operator_method(context.source.node_text(method))
            })
        }
        // `a.b = 1` is a `send` named `b=`, which is no operator; `a[0] = 1` is `:[]=`, which is.
        "assignment" => node
            .field("left")
            .is_some_and(|left| left.kind_str() == "call"),
        _ => false,
    }
}

/// `unsafe_autocorrect?`: `and`, `or` and `not` all bind looser than the ternary, so the
/// parentheses are what make the condition one.
fn unsafe_autocorrect(context: &RuleContext<'_>, condition: Node<'_>) -> bool {
    super::nodes::children(condition)
        .into_iter()
        .any(|child| match child.kind_str() {
            "binary" | "unary" => child
                .field("operator")
                .is_some_and(|operator| {
                    matches!(context.source.node_text(operator), "and" | "or" | "not")
                }),
            _ => false,
        })
}

/// `correct_unparenthesized`: `corrector.wrap(condition, '(', ')')`.
fn wrap_in_parentheses(condition: Node<'_>) -> Vec<Edit> {
    vec![
        Edit {
            start: condition.start_byte(),
            end: condition.start_byte(),
            replacement: "(".to_owned(),
            safe: true,
        },
        Edit {
            start: condition.end_byte(),
            end: condition.end_byte(),
            replacement: ")".to_owned(),
            safe: true,
        },
    ]
}

/// `correct_parenthesized`: the two parentheses go, a space takes the closing one's place where
/// nothing else separates the condition from the `?`, and a call that was holding its arguments
/// together by the parentheses gets parentheses of its own.
fn correct_parenthesized(context: &RuleContext<'_>, condition: Node<'_>) -> Vec<Edit> {
    let open = condition.start_byte();
    let close = condition.end_byte() - 1;
    let mut edits = vec![
        Edit {
            start: open,
            end: open + 1,
            replacement: String::new(),
            safe: true,
        },
        Edit {
            start: close,
            end: condition.end_byte(),
            replacement: String::new(),
            safe: true,
        },
    ];
    // `whitespace_after?`: `bar?)? a : b` would run the `?` of the predicate into the ternary's.
    if !context
        .source
        .text()
        .as_bytes()
        .get(condition.end_byte())
        .is_some_and(u8::is_ascii_whitespace)
    {
        edits.push(Edit {
            start: condition.end_byte(),
            end: condition.end_byte(),
            replacement: " ".to_owned(),
            safe: true,
        });
    }
    if let Some(call) = super::nodes::children(condition).last().copied() {
        edits.extend(parenthesize_condition_arguments(context, call));
    }
    edits
}

/// The `defined?` keyword or the selector a call's arguments follow, and the arguments themselves.
fn call_parts<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Vec<Node<'tree>>)> {
    match node.kind_str() {
        "call" => {
            let selector = node.field("method")?;
            let arguments = super::nodes::children(node.field("arguments")?);
            Some((selector, arguments))
        }
        // `(defined? (send nil :x))`: the keyword stands where a call's selector would.
        "unary" => {
            let keyword = node.field("operator")?;
            if context.source.node_text(keyword) != "defined?" {
                return None;
            }
            Some((keyword, vec![node.field("operand")?]))
        }
        _ => None,
    }
}

/// `parenthesize_condition_arguments`, guarded by `node_args_need_parens?`.
fn parenthesize_condition_arguments(context: &RuleContext<'_>, node: Node<'_>) -> Vec<Edit> {
    let Some((selector, arguments)) = call_parts(node, context) else {
        return Vec::new();
    };
    let (Some(first), Some(last)) = (arguments.first(), arguments.last()) else {
        return Vec::new();
    };
    // `send_node.parenthesized?`: a `(` written straight against the selector already holds the
    // arguments together.
    if context.source.text().as_bytes().get(selector.end_byte()) == Some(&b'(') {
        return Vec::new();
    }
    // `send_node.dot? || send_node.safe_navigation? || unparenthesized_method_call?(send_node)`.
    let reached_by_operator = node
        .field("operator")
        .is_some_and(|operator| matches!(context.source.node_text(operator), "." | "&."));
    let named = context
        .source
        .node_text(selector)
        .starts_with(|character: char| character.is_ascii_alphabetic());
    if !reached_by_operator && !named {
        return Vec::new();
    }
    vec![
        Edit {
            start: selector.end_byte(),
            end: first.start_byte(),
            replacement: "(".to_owned(),
            safe: true,
        },
        Edit {
            start: last.end_byte(),
            end: last.end_byte(),
            replacement: ")".to_owned(),
            safe: true,
        },
    ]
}
