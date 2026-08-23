use crate::diagnostic::Offense;
use crate::rules::RuleContext;

use super::ambiguity::scan;

/// `AMBIGUITIES`, longest prefix first so that `**` is not read as `*`.
const AMBIGUITIES: &[(&str, &str, &str)] = &[
    ("**", "keyword splat", "an exponent"),
    ("*", "splat", "a multiplication"),
    ("&", "block", "a binary AND"),
    ("+", "positive number", "an addition"),
    ("-", "negative number", "a subtraction"),
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let prefixes: Vec<&str> = AMBIGUITIES.iter().map(|(prefix, _, _)| *prefix).collect();
    for ambiguity in scan(context, &prefixes) {
        let operator = context.source.slice(ambiguity.operator.clone());
        let Some((_, actual, possible)) = AMBIGUITIES
            .iter()
            .find(|(prefix, _, _)| *prefix == operator)
        else {
            continue;
        };
        // `find_offense_node_by` reaches `*`, `**` and `&` as `splat`, `kwsplat` and `block_pass`
        // nodes, which say nothing about the call they sit in. A unary `+` or `-` is found through
        // `each_node(:send)` instead -- and a `csend` is not a `send`, so `foo&.* -1` is not one.
        if matches!(operator, "+" | "-")
            && !crate::rules::send_node::is_plain_send(ambiguity.owner, context)
        {
            continue;
        }
        offenses.push(
            context
                .offense(
                    format!(
                        "Ambiguous {actual} operator. Parenthesize the method arguments if it's \
                         surely a {actual} operator, or add a whitespace to the right of the \
                         `{operator}` if it should be {possible}."
                    ),
                    ambiguity.operator.clone(),
                )
                .corrections_anchored_at(ambiguity.owner.byte_range())
                .corrected_by_all(ambiguity.parenthesize(context)),
        );
    }
}
