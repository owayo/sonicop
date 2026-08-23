//! `Metrics/AbcSize`, a port of `Utils::AbcSizeCalculator`.
//!
//! `CountRepeatedAttributes` is `true` in the bundled configuration, which turns
//! `Utils::RepeatedAttributeDiscount` off; [`Attributes`] is what a run setting it to `false` gets.

use std::collections::HashMap;

use tree_sitter::Node;

use super::complexity::{Allowed, CsendDiscount, Emit, Kind, Order, Walk, measured};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max = context
        .setting::<f64>("Max")
        .or_else(|| context.setting::<i64>("Max").map(|max| max as f64))
        .unwrap_or(17.0);
    let allowed = Allowed::new(context);
    let methods = measured(context, &allowed);
    if methods.is_empty() {
        return;
    }
    let fragments = context.fragments();
    let locals = context.metric_locals();
    let walk = Walk::new(context, locals, fragments, Order::Post);
    // `discount_repeated_attributes: !cop_config['CountRepeatedAttributes']`.
    let discounting = !context
        .setting::<bool>("CountRepeatedAttributes")
        .unwrap_or(true);
    for method in methods {
        let vector = Vector::of(&walk, method.body, context, locals, discounting);
        let size = vector.size();
        if size <= max {
            continue;
        }
        offenses.push(context.offense(
            format!(
                "Assignment Branch Condition size for `{}` is too high. [{vector} {}/{}]",
                method.name,
                format_g4(size),
                format_g4(max),
            ),
            method.location.byte_range(),
        ));
    }
}

/// The three counts `AbcSizeCalculator` keeps: assignments, branches -- which is to say calls --
/// and conditions.
#[derive(Default)]
struct Vector {
    assignment: u32,
    branch: u32,
    condition: u32,
}

impl Vector {
    fn of(
        walk: &Walk<'_>,
        body: tree_sitter::Node<'_>,
        context: &RuleContext<'_>,
        locals: &super::locals::Locals,
        discounting: bool,
    ) -> Self {
        let mut vector = Self::default();
        let mut discount = CsendDiscount::default();
        let mut attributes = discounting.then(Attributes::default);
        walk.run(body, &mut |emit| {
            vector.count(emit, &mut discount, context, locals, attributes.as_mut());
        });
        vector
    }

    /// `AbcSizeCalculator#calculate_node`, whose two arms are exclusive: a call is never also
    /// counted as a condition, though it may add one.
    fn count<'a>(
        &mut self,
        emit: Emit<'a>,
        discount: &mut CsendDiscount<'a>,
        context: &RuleContext<'_>,
        locals: &super::locals::Locals,
        attributes: Option<&mut Attributes>,
    ) {
        // `RepeatedAttributeDiscount#calculate_node` invalidates before the count, so an
        // assignment to a receiver does not discount the reads that follow it.
        let repeated = match attributes {
            Some(known) => {
                known.invalidate(emit, context, locals);
                matches!(emit.kind, Kind::Send | Kind::Csend)
                    && known.repeats(emit.node, context, locals)
            }
            None => false,
        };
        if self.assignment(emit, discount) {
            self.assignment += 1;
        }
        match emit.kind {
            Kind::Send | Kind::Csend if repeated => {}
            Kind::Send | Kind::Csend | Kind::Yield => self.branch_node(emit, discount),
            // A block on a method that is known not to iterate is not a decision point.
            kind if is_condition(kind) && emit.iterating != Some(false) => {
                self.condition += u32::from(emit.has_else) + 1;
            }
            _ => {}
        }
    }

    /// `AbcSizeCalculator#assignment?`, which also does the counting a compound assignment needs:
    /// neither a multiple assignment nor `+=` can be read off its own node, so the calculator
    /// looks at what the node was built from and then declines to count the node itself.
    fn assignment<'a>(&mut self, emit: Emit<'a>, discount: &mut CsendDiscount<'a>) -> bool {
        match emit.kind {
            Kind::Masgn | Kind::OpAsgn | Kind::OrAsgn | Kind::AndAsgn => {
                self.assignment += emit.miscounted as u32;
                false
            }
            Kind::For | Kind::Asgn => true,
            Kind::Send | Kind::Csend => emit.setter,
            Kind::Lvasgn => {
                discount.reset(emit.name);
                emit.capturing
            }
            Kind::Arg => emit.capturing,
            _ => false,
        }
    }

    /// `AbcSizeCalculator#evaluate_branch_nodes`. A comparison is a condition rather than a call,
    /// and `&.` is both a call and a condition unless the discount already counted one.
    fn branch_node<'a>(&mut self, emit: Emit<'a>, discount: &mut CsendDiscount<'a>) {
        if emit.comparison {
            self.condition += 1;
            return;
        }
        self.branch += 1;
        if emit.kind == Kind::Csend && !discount.repeats(emit.name) {
            self.condition += 1;
        }
    }

    fn size(&self) -> f64 {
        let square = |value: u32| f64::from(value) * f64::from(value);
        round2((square(self.assignment) + square(self.branch) + square(self.condition)).sqrt())
    }
}

impl std::fmt::Display for Vector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "<{}, {}, {}>",
            self.assignment, self.branch, self.condition
        )
    }
}

/// `CyclomaticComplexity::COUNTED_NODES`, which is what the calculator calls `CONDITION_NODES`.
/// `csend` is in that list too, but a call never reaches this arm.
fn is_condition(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::If
            | Kind::While
            | Kind::Until
            | Kind::For
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

/// `Float#round(2)`, which rounds halves away from zero rather than to even, and corrects for the
/// value the multiplication by 100 landed on rather than the one it was meant to.
fn round2(value: f64) -> f64 {
    let scale = 100.0;
    let mut rounded = (value * scale).round();
    if value > 0.0 && (rounded + 0.5) / scale <= value {
        rounded += 1.0;
    }
    rounded / scale
}

/// Ruby's `format('%.4g', value)`, which is not the platform's `printf`.
///
/// Ruby rounds to four significant digits with its own `dtoa`, which breaks a tie towards the even
/// digit and strips the trailing zeros rounding leaves behind only when the value it started from
/// falls short of the half-way point. `130.05` therefore prints as `130.0` while `100.05`, whose
/// nearest double lands just below, prints as `100`.
fn format_g4(value: f64) -> String {
    const PRECISION: usize = 4;
    if value == 0.0 {
        return "0".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let magnitude = value.abs();
    // The shortest representation that reads back as this double is the decimal Ruby rounds.
    let text = format!("{magnitude}");
    let (mantissa, suffix) = text.split_once(['e', 'E']).unwrap_or((text.as_str(), "0"));
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits: Vec<u8> = integer
        .bytes()
        .chain(fraction.bytes())
        .map(|byte| byte - b'0')
        .collect();
    let mut exponent = integer.len() as i32 - 1 + suffix.parse::<i32>().unwrap_or(0);
    while digits.first() == Some(&0) {
        digits.remove(0);
        exponent -= 1;
    }
    if digits.is_empty() {
        return "0".to_owned();
    }
    let mut strip = true;
    if digits.len() > PRECISION {
        let rest = digits[PRECISION + 1..].iter().any(|digit| *digit != 0);
        let tie = digits[PRECISION] == 5 && !rest;
        let even = digits[PRECISION - 1] % 2 == 0;
        let round_up = digits[PRECISION] > 5 || (digits[PRECISION] == 5 && rest) || (tie && !even);
        // A tie the value approaches from above keeps the zeros: `dtoa` takes its rounding branch
        // but declines to move a digit that is already even.
        if tie && even && above_decimal(magnitude, &digits, exponent) {
            strip = false;
        }
        digits.truncate(PRECISION);
        if round_up {
            // Trailing nines are dropped rather than carried, so `1099` becomes `11`.
            loop {
                match digits.pop() {
                    None => {
                        digits.push(1);
                        exponent += 1;
                        break;
                    }
                    Some(9) => {}
                    Some(digit) => {
                        digits.push(digit + 1);
                        break;
                    }
                }
            }
        }
    }
    if strip {
        while digits.len() > 1 && digits.last() == Some(&0) {
            digits.pop();
        }
    }
    let written: String = digits.iter().map(|digit| (digit + b'0') as char).collect();
    if exponent < -4 || exponent >= PRECISION as i32 {
        let head = &written[..1];
        let tail = &written[1..];
        let point = if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        };
        let mark = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{head}{point}e{mark}{:02}", exponent.abs());
    }
    if exponent >= 0 {
        let split = (exponent + 1) as usize;
        let integer = format!("{written:0<split$}");
        let (integer, fraction) = integer.split_at(split);
        let point = if fraction.is_empty() {
            String::new()
        } else {
            format!(".{fraction}")
        };
        return format!("{sign}{integer}{point}");
    }
    let zeros = "0".repeat((-exponent - 1) as usize);
    format!("{sign}0.{zeros}{written}")
}

/// Whether the double lies strictly above the decimal its shortest representation spells out.
///
/// The comparison has to be exact, so both sides are scaled to integers: a double is a whole
/// number of powers of two, and the decimal a whole number of powers of ten.
fn above_decimal(value: f64, digits: &[u8], exponent: i32) -> bool {
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & 0x000f_ffff_ffff_ffff;
    let (mantissa, power) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | 0x0010_0000_0000_0000, biased - 1075)
    };
    let significand: u128 = digits
        .iter()
        .fold(0u128, |total, digit| total * 10 + u128::from(*digit));
    let scale = exponent - digits.len() as i32 + 1;
    let left = scaled(u128::from(mantissa), power.max(0), (-scale).max(0));
    let right = scaled(significand, (-power).max(0), scale.max(0));
    match (left, right) {
        (Some(left), Some(right)) => left > right,
        _ => false,
    }
}

/// `value * 2^twos * 10^tens`, or `None` when that does not fit.
fn scaled(value: u128, twos: i32, tens: i32) -> Option<u128> {
    let value = value.checked_mul(2u128.checked_pow(u32::try_from(twos).ok()?)?)?;
    value.checked_mul(10u128.checked_pow(u32::try_from(tens).ok()?)?)
}

/// `Utils::RepeatedAttributeDiscount`: the attribute reads already seen, so a second read of the
/// same one is not counted as another branch.
///
/// Upstream keys the tree by the receiver node itself and makes the `nil` and `self` receivers
/// share one entry, which is what lets `bar`, `self.bar` and `bar` count once between them. The
/// keys here spell the same identity as a string.
#[derive(Default)]
struct Attributes {
    tree: HashMap<String, Attributes>,
}

impl Attributes {
    /// `discount_repeated_attribute?`: walks the receiver chain, recording what it has not seen.
    /// True only when every step of the chain was already there.
    fn repeats(
        &mut self,
        node: Node<'_>,
        context: &RuleContext<'_>,
        locals: &super::locals::Locals,
    ) -> bool {
        match key_path(node, context, locals) {
            Some(path) => self.record(&path),
            None => false,
        }
    }

    fn record(&mut self, path: &[String]) -> bool {
        let Some((head, rest)) = path.split_first() else {
            return true;
        };
        let known = self.tree.contains_key(head);
        let deeper = self.tree.entry(head.clone()).or_default().record(rest);
        known & deeper
    }

    /// `update_repeated_attribute`: writing to a receiver invalidates what was read through it.
    fn invalidate(
        &mut self,
        emit: Emit<'_>,
        context: &RuleContext<'_>,
        locals: &super::locals::Locals,
    ) {
        let Some((path, method)) = setter_to_getter(emit, context, locals) else {
            return;
        };
        let Some(branch) = self.navigate(&path) else {
            return;
        };
        match method {
            Some(method) => drop(branch.tree.remove(&method)),
            None => branch.tree.clear(),
        }
    }

    fn navigate(&mut self, path: &[String]) -> Option<&mut Self> {
        let mut current = self;
        for key in path {
            current = current.tree.get_mut(key)?;
        }
        Some(current)
    }
}

/// `setter_to_getter`, as the receiver chain to invalidate and the one attribute under it -- or
/// the whole branch, when a variable itself was written.
fn setter_to_getter(
    emit: Emit<'_>,
    context: &RuleContext<'_>,
    locals: &super::locals::Locals,
) -> Option<(Vec<String>, Option<String>)> {
    match emit.kind {
        // `(lvasgn :my_var _)` and its three siblings: everything read through the name goes.
        Kind::Lvasgn | Kind::Asgn => {
            let target = emit.node.field("left")?;
            Some((vec![root_key(target, context, locals)?], None))
        }
        // `foo.bar = 1` and `foo.bar ||= 1` both name one attribute of one receiver.
        Kind::Send | Kind::Csend if emit.setter => {
            let receiver = emit.node.field("receiver");
            let method = context.source.node_text(emit.node.field("method")?);
            let path = match receiver {
                Some(receiver) => key_path(receiver, context, locals)?,
                None => vec![ROOT.to_owned()],
            };
            Some((path, Some(method.trim_end_matches('=').to_owned())))
        }
        Kind::OpAsgn | Kind::OrAsgn | Kind::AndAsgn => {
            let target = emit.node.field("left")?;
            match target.kind_str() {
                "call" => {
                    let method = context.source.node_text(target.field("method")?);
                    let path = match target.field("receiver") {
                        Some(receiver) => key_path(receiver, context, locals)?,
                        None => vec![ROOT.to_owned()],
                    };
                    Some((path, Some(method.to_owned())))
                }
                _ => Some((vec![root_key(target, context, locals)?], None)),
            }
        }
        _ => None,
    }
}

/// The entry `nil` and `self` share upstream, by sharing one hash between the two keys.
const ROOT: &str = "self";

/// `find_attributes`, flattened: the chain of names a read walks through, or `None` when the node
/// is not a series of argument-less calls over a root the tree can key on.
fn key_path(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &super::locals::Locals,
) -> Option<Vec<String>> {
    if let Some(root) = root_key(node, context, locals) {
        return Some(vec![root]);
    }
    match node.kind_str() {
        // `attribute_call?` is `(call _receiver _method)` -- two children, so no arguments.
        "call" if node.field("arguments").is_none() && node.field("block").is_none() => {
            let method = context.source.node_text(node.field("method")?);
            let mut path = match node.field("receiver") {
                Some(receiver) => key_path(receiver, context, locals)?,
                None => vec![ROOT.to_owned()],
            };
            path.push(method.to_owned());
            Some(path)
        }
        // A bare name that is not a local is `(send nil :name)`.
        "identifier" => Some(vec![
            ROOT.to_owned(),
            context.source.node_text(node).to_owned(),
        ]),
        _ => None,
    }
}

/// `root_node?`: the receivers the tree keys on directly rather than walking through.
fn root_key(
    node: Node<'_>,
    context: &RuleContext<'_>,
    locals: &super::locals::Locals,
) -> Option<String> {
    let text = || context.source.node_text(node);
    match node.kind_str() {
        "self" => Some(ROOT.to_owned()),
        "identifier" if locals.is_lvar(node) => Some(format!("lvar:{}", text())),
        "instance_variable" => Some(format!("ivar:{}", text())),
        "class_variable" => Some(format!("cvar:{}", text())),
        "global_variable" => Some(format!("gvar:{}", text())),
        "constant" | "scope_resolution" => Some(format!("const:{}", text())),
        _ => None,
    }
}
