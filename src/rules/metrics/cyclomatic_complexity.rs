use super::complexity::{Allowed, CsendDiscount, Emit, Kind, Order, Walk, measured};
use super::fragments::Fragments;
use super::locals::Locals;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: i64 = context.setting("Max").unwrap_or(7);
    let allowed = Allowed::new(context);
    let methods = measured(context, &allowed);
    if methods.is_empty() {
        return;
    }
    let fragments = Fragments::new(context);
    let locals = Locals::new(context, &fragments);
    let walk = Walk::new(context, &locals, &fragments, Order::Pre);
    for method in methods {
        let mut score = 1i64;
        let mut discount = CsendDiscount::default();
        walk.run(method.body, &mut |emit| {
            // `each_node(:lvasgn, *COUNTED_NODES)` visits assignments only to forget the `&.`
            // calls made on the variable before it was written.
            if emit.kind == Kind::Lvasgn {
                discount.reset(emit.name);
            } else if is_counted(emit.kind) {
                score += score_for(emit, &mut discount);
            }
        });
        if score <= max {
            continue;
        }
        offenses.push(context.offense(
            format!(
                "Cyclomatic complexity for `{}` is too high. [{score}/{max}]",
                method.name
            ),
            method.location.byte_range(),
        ));
    }
}

/// `CyclomaticComplexity#complexity_score_for`.
pub(super) fn score_for<'a>(emit: Emit<'a>, discount: &mut CsendDiscount<'a>) -> i64 {
    // A block on a method that is not known to iterate adds no path of its own.
    if emit.iterating == Some(false) {
        return 0;
    }
    if emit.kind == Kind::Csend && discount.repeats(emit.name) {
        return 0;
    }
    1
}

/// `CyclomaticComplexity::COUNTED_NODES`.
fn is_counted(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::If
            | Kind::While
            | Kind::Until
            | Kind::For
            | Kind::Csend
            | Kind::Block
            | Kind::BlockPass
            | Kind::Rescue
            | Kind::When
            | Kind::InPattern
            | Kind::And
            | Kind::Or
            | Kind::OrAsgn
            | Kind::AndAsgn
    )
}
