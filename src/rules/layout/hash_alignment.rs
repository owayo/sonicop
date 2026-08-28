//! `Layout/HashAlignment`.

use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use super::support::{Edits, begins_its_line, hash_literals};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

const KEY_MESSAGE: &str = "Align the keys of a hash literal if they span more than one line.";
const SEPARATOR_MESSAGE: &str =
    "Align the separators of a hash literal if they span more than one line.";
const TABLE_MESSAGE: &str =
    "Align the keys and values of a hash literal if they span more than one line.";
const KWSPLAT_MESSAGE: &str =
    "Align keyword splats with the rest of the hash if it spans more than one line.";

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Style {
    Key,
    Table,
    Separator,
}

impl Style {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "key" => Some(Self::Key),
            "table" => Some(Self::Table),
            "separator" => Some(Self::Separator),
            _ => None,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Key => KEY_MESSAGE,
            Self::Table => TABLE_MESSAGE,
            Self::Separator => SEPARATOR_MESSAGE,
        }
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let rockets = styles(context, "EnforcedHashRocketStyle");
    let colons = styles(context, "EnforcedColonStyle");
    // `autocorrect_incompatible_with_other_cops?` only ever fires under the neighbouring cop's
    // fixed-indentation style, which leaves the whole hash to that cop.
    let fixed_indentation = context
        .setting_of::<String>("Layout/ArgumentAlignment", "EnforcedStyle")
        .as_deref()
        == Some("with_fixed_indentation");

    // `EnforcedLastArgumentHashStyle`: the hash a call takes as its last argument is aligned
    // against the call rather than against itself, so three of the four settings leave it alone.
    let last_argument_style = context
        .setting::<String>("EnforcedLastArgumentHashStyle")
        .unwrap_or_else(|| "always_inspect".to_owned());

    for literal in hash_literals(context) {
        let hash = Hash::new(context, &literal);
        if hash.first_pair().is_none() || hash.single_line() {
            continue;
        }
        if ignored_last_argument(&literal, context, &last_argument_style) {
            continue;
        }
        if fixed_indentation && hash.starts_beside_its_call(&literal) {
            continue;
        }
        let checkable = |style: &Style| match style {
            Style::Key => true,
            _ => !hash.pairs_on_same_line() && !hash.mixed_delimiters(),
        };
        if !rockets.iter().any(checkable) || !colons.iter().any(checkable) {
            continue;
        }
        hash.check_pairs(context, &rockets, &colons, offenses);
    }
}

fn styles(context: &RuleContext<'_>, key: &str) -> Vec<Style> {
    let configured = context
        .setting::<Vec<String>>(key)
        .or_else(|| context.setting::<String>(key).map(|value| vec![value]))
        .unwrap_or_else(|| vec!["key".to_owned()]);
    let mut styles: Vec<Style> = Vec::new();
    for value in configured {
        if let Some(style) = Style::parse(&value) {
            if !styles.contains(&style) {
                styles.push(style);
            }
        }
    }
    if styles.is_empty() {
        styles.push(Style::Key);
    }
    styles
}

/// A source position in the units every delta is measured in: a one-based line and a zero-based
/// character column.
#[derive(Clone, Copy)]
struct Position {
    offset: usize,
    line: usize,
    column: i64,
}

impl Position {
    fn new(context: &RuleContext<'_>, offset: usize) -> Self {
        let (line, column) = context.source.line_column(offset);
        Self {
            offset,
            line,
            column: column as i64 - 1,
        }
    }
}

/// One element of a hash literal: a `pair`, or the `**splat` that stands in for one.
struct Element {
    id: usize,
    range: Range<usize>,
    start: Position,
    end_line: usize,
    key_start: Position,
    key_end: Position,
    key_single_line: bool,
    key_width: i64,
    operator: Option<(Position, Position)>,
    value_start: Option<Position>,
    hash_rocket: bool,
    kwsplat: bool,
    /// `{x:}`, whose value the parser synthesizes from the key.
    omitted_value: bool,
}

impl Element {
    fn new(context: &RuleContext<'_>, node: Node<'_>) -> Self {
        let text = context.source.text();
        let kwsplat = node.kind_str() != "pair";
        let key = if kwsplat {
            node
        } else {
            node.field("key").unwrap_or(node)
        };
        let operator = (!kwsplat).then(|| operator_of(node)).flatten();
        let hash_rocket = operator
            .as_ref()
            .is_some_and(|range| &text[range.clone()] == "=>");
        let value = if kwsplat {
            Some(node.byte_range())
        } else {
            node.field("value").map(|value| value.byte_range())
        };
        Self {
            id: node.id(),
            range: node.byte_range(),
            start: Position::new(context, node.start_byte()),
            end_line: context.source.line_column(node.end_byte()).0,
            key_start: Position::new(context, key.start_byte()),
            key_end: Position::new(context, key.end_byte()),
            key_single_line: key.start_position().row == key.end_position().row,
            key_width: text[key.byte_range()].chars().count() as i64,
            operator: operator.map(|range| {
                (
                    Position::new(context, range.start),
                    Position::new(context, range.end),
                )
            }),
            value_start: value.map(|range| Position::new(context, range.start)),
            hash_rocket,
            kwsplat,
            omitted_value: !kwsplat && text[node.byte_range()].ends_with(':'),
        }
    }

    /// `HashElementNode#same_line?`, which counts a shared line at either end.
    fn same_line(&self, other: &Self) -> bool {
        self.end_line == other.start.line || self.start.line == other.end_line
    }
}

struct Hash {
    elements: Vec<Element>,
    first_line: usize,
    last_line: usize,
    parent_kind: &'static str,
    starts_beside: bool,
}

impl Hash {
    fn new(context: &RuleContext<'_>, literal: &[Node<'_>]) -> Self {
        let elements: Vec<Element> = literal
            .iter()
            .map(|node| Element::new(context, *node))
            .collect();
        let first = literal[0];
        let last = literal[literal.len() - 1];
        // `node.single_line?` is asked of the **hash**, whose braces reach past its pairs. Only a
        // brace-less hash has no node of its own, and there the pairs are its whole extent.
        let braced = first
            .parent_of(context)
            .filter(|parent| parent.kind_str() == "hash");
        let (span_start, span_end) = match braced {
            Some(hash) => (hash.start_byte(), hash.end_byte()),
            None => (first.start_byte(), last.end_byte()),
        };
        Self {
            first_line: context.source.line_column(span_start).0,
            last_line: context.source.line_column(span_end).0,
            parent_kind: first
                .parent_of(context)
                .map_or("", |parent| parent.kind_str()),
            starts_beside: starts_beside_its_call(context, first, &elements),
            elements,
        }
    }

    fn pairs(&self) -> impl Iterator<Item = &Element> {
        self.elements.iter().filter(|element| !element.kwsplat)
    }

    fn first_pair(&self) -> Option<&Element> {
        self.pairs().next()
    }

    fn single_line(&self) -> bool {
        self.first_line == self.last_line
    }

    fn pairs_on_same_line(&self) -> bool {
        let pairs: Vec<&Element> = self.pairs().collect();
        pairs
            .windows(2)
            .any(|window| window[0].same_line(window[1]))
    }

    fn mixed_delimiters(&self) -> bool {
        let mut rockets = false;
        let mut colons = false;
        for pair in self.pairs() {
            if pair.hash_rocket {
                rockets = true;
            } else {
                colons = true;
            }
        }
        rockets && colons
    }

    fn starts_beside_its_call(&self, _literal: &[Node<'_>]) -> bool {
        self.parent_kind == "argument_list" && self.starts_beside
    }

    fn check_pairs(
        &self,
        context: &RuleContext<'_>,
        rockets: &[Style],
        colons: &[Style],
        offenses: &mut Vec<Offense>,
    ) {
        let first = self.first_pair().expect("checked by the caller");
        // `offenses_by` is keyed by alignment class and keeps the order its keys were first
        // written in, which is what settles the tie when two styles report as many offenses.
        let mut order: Vec<Style> = Vec::new();
        let mut by_style: HashMap<Style, Vec<usize>> = HashMap::new();
        let mut deltas: HashMap<(Style, usize), Deltas> = HashMap::new();
        let mut kwsplats: Vec<(usize, Deltas)> = Vec::new();

        let note = |order: &mut Vec<Style>, by_style: &mut HashMap<Style, Vec<usize>>, style| {
            if !order.contains(&style) {
                order.push(style);
            }
            by_style.entry(style).or_default();
        };

        for style in self.alignment_for(first, rockets, colons) {
            note(&mut order, &mut by_style, style);
            let delta = self.deltas_for_first_pair(style, first);
            if !delta.is_zero() {
                deltas.insert((style, first.id), delta);
                by_style.entry(style).or_default().push(first.id);
            }
        }

        for current in &self.elements {
            if current.kwsplat {
                let delta = if begins_its_line(context, current.range.start) {
                    Deltas {
                        key: Some(self.key_delta(first, current, false)),
                        ..Deltas::default()
                    }
                } else {
                    Deltas::default()
                };
                if !delta.is_zero() {
                    kwsplats.push((current.id, delta));
                }
                continue;
            }
            for style in self.alignment_for(current, rockets, colons) {
                note(&mut order, &mut by_style, style);
                let delta = self.deltas(context, style, first, current);
                if !delta.is_zero() {
                    deltas.insert((style, current.id), delta);
                    by_style.entry(style).or_default().push(current.id);
                }
            }
        }

        // Keyword splats are pulled out of the table and reported under their own message before
        // the winning style's offenses are.
        for (id, delta) in kwsplats {
            self.report(context, id, delta, KWSPLAT_MESSAGE, offenses);
        }

        let Some(style) = order
            .iter()
            .copied()
            .min_by_key(|style| by_style[style].len())
        else {
            return;
        };
        let mut reported: Vec<usize> = Vec::new();
        for id in &by_style[&style] {
            if reported.contains(id) {
                continue;
            }
            reported.push(*id);
            let correction_delta = self
                .elements
                .iter()
                .find(|element| element.id == *id)
                .and_then(|element| {
                    self.alignment_for(element, rockets, colons)
                        .first()
                        .copied()
                        .map(|correction_style| match element.id == first.id {
                            true => self.deltas_for_first_pair(correction_style, element),
                            false => self.deltas(context, correction_style, first, element),
                        })
                })
                .unwrap_or(deltas[&(style, *id)]);
            // Detection chooses the configured style with the fewest offenses, but RuboCop's
            // corrector still applies the first configured style. This can deliberately leave a
            // reported table-style offense unchanged while another offense moves by key style.
            self.report(context, *id, correction_delta, style.message(), offenses);
        }
    }

    fn alignment_for(&self, element: &Element, rockets: &[Style], colons: &[Style]) -> Vec<Style> {
        if element.hash_rocket {
            rockets.to_vec()
        } else {
            colons.to_vec()
        }
    }

    fn report(
        &self,
        context: &RuleContext<'_>,
        id: usize,
        delta: Deltas,
        message: &str,
        offenses: &mut Vec<Offense>,
    ) {
        let Some(element) = self.elements.iter().find(|element| element.id == id) else {
            return;
        };
        offenses.push(
            context
                .offense(message, element.range.clone())
                .corrected_by_all(corrections(context, element, delta)),
        );
    }

    fn deltas_for_first_pair(&self, style: Style, first: &Element) -> Deltas {
        match style {
            Style::Key => Deltas {
                key: None,
                separator: Some(key_separator_delta(first)),
                value: Some(key_value_delta(first)),
            },
            Style::Table => {
                let separator = self.value_separator_delta(style, first, first, 0);
                Deltas {
                    key: None,
                    separator: Some(separator),
                    value: Some(self.table_value_delta(first, first) - separator),
                }
            }
            Style::Separator => Deltas::default(),
        }
    }

    fn deltas(
        &self,
        context: &RuleContext<'_>,
        style: Style,
        first: &Element,
        current: &Element,
    ) -> Deltas {
        match style {
            Style::Key => {
                if !begins_its_line(context, current.range.start) {
                    return Deltas::default();
                }
                Deltas {
                    key: Some(self.key_delta(first, current, false)),
                    separator: Some(key_separator_delta(current)),
                    value: Some(key_value_delta(current)),
                }
            }
            _ => {
                let key = self.key_delta(first, current, style == Style::Separator);
                let separator = self.value_separator_delta(style, first, current, key);
                let value = match style {
                    Style::Table => self.table_value_delta(first, current),
                    _ => separator_value_delta(first, current),
                } - key
                    - separator;
                Deltas {
                    key: Some(key),
                    separator: Some(separator),
                    value: Some(value),
                }
            }
        }
    }

    fn key_delta(&self, first: &Element, current: &Element, right: bool) -> i64 {
        if first.same_line(current) {
            return 0;
        }
        if right && (first.kwsplat || current.kwsplat) {
            return 0;
        }
        if right {
            first.key_end.column - current.key_end.column
        } else {
            first.key_start.column - current.key_start.column
        }
    }

    fn value_separator_delta(
        &self,
        style: Style,
        first: &Element,
        current: &Element,
        key_delta: i64,
    ) -> i64 {
        if !current.hash_rocket {
            return 0;
        }
        let rocket = match style {
            Style::Table => {
                let Some((operator, _)) = &current.operator else {
                    return 0;
                };
                self.target_operator_column(first) - operator.column
            }
            _ => delimiter_delta(first, current),
        };
        rocket - key_delta
    }

    fn table_value_delta(&self, first: &Element, current: &Element) -> i64 {
        if current.omitted_value {
            return 0;
        }
        let Some(value) = &current.value_start else {
            return 0;
        };
        let correct = self.target_operator_column(first) + self.max_delimiter_width() - 1;
        correct - value.column
    }

    /// `target_operator_column`: the shared key margin plus the widest single-line key, or a
    /// multiline key's own end column when that reaches further.
    fn target_operator_column(&self, first: &Element) -> i64 {
        let mut candidates: Vec<i64> = self
            .pairs()
            .filter(|pair| !pair.key_single_line)
            .map(|pair| pair.key_end.column + 1)
            .collect();
        let key_width = self
            .pairs()
            .filter(|pair| pair.key_single_line)
            .map(|pair| pair.key_width)
            .max()
            .unwrap_or(0);
        if key_width > 0 {
            candidates.push(first.start.column + key_width + 1);
        }
        candidates.into_iter().max().unwrap_or(0)
    }

    fn max_delimiter_width(&self) -> i64 {
        self.pairs()
            .map(|pair| if pair.hash_rocket { 4 } else { 2 })
            .max()
            .unwrap_or(0)
    }
}

/// `KeyAlignment#separator_delta`: a rocket sits one space past its key.
fn key_separator_delta(pair: &Element) -> i64 {
    if !pair.hash_rocket {
        return 0;
    }
    let Some((operator, _)) = &pair.operator else {
        return 0;
    };
    pair.key_end.column + 1 - operator.column
}

/// `KeyAlignment#value_delta`: a value sits one space past its separator.
fn key_value_delta(pair: &Element) -> i64 {
    let (Some((_, operator_end)), Some(value)) = (&pair.operator, &pair.value_start) else {
        return 0;
    };
    if pair.omitted_value || pair.key_start.line != value.line {
        return 0;
    }
    operator_end.column + 1 - value.column
}

fn delimiter_delta(first: &Element, current: &Element) -> i64 {
    if first.same_line(current) || first.hash_rocket != current.hash_rocket {
        return 0;
    }
    let (Some((a, _)), Some((b, _))) = (&first.operator, &current.operator) else {
        return 0;
    };
    a.column - b.column
}

fn separator_value_delta(first: &Element, current: &Element) -> i64 {
    if current.omitted_value {
        return 0;
    }
    if first.same_line(current) || first.kwsplat || current.kwsplat {
        return 0;
    }
    let (Some(a), Some(b)) = (&first.value_start, &current.value_start) else {
        return 0;
    };
    a.column - b.column
}

fn corrections(context: &RuleContext<'_>, element: &Element, delta: Deltas) -> Vec<Edit> {
    let mut edits = Edits::new(context.source.text());
    if element.kwsplat || element.omitted_value || element.value_start.is_none() {
        edits.adjust(element.range.start, delta.key.unwrap_or(0));
    } else {
        let mut key_delta = delta.key.unwrap_or(0);
        if key_delta < -element.key_start.column {
            key_delta = -element.key_start.column;
        }
        edits.adjust(element.key_start.offset, key_delta);
        if let Some((operator, _)) = &element.operator {
            edits.adjust(operator.offset, delta.separator.unwrap_or(0));
        }
        if let Some(value) = &element.value_start {
            edits.adjust(value.offset, delta.value.unwrap_or(0));
        }
    }
    edits.finish()
}

#[derive(Clone, Copy, Default)]
struct Deltas {
    key: Option<i64>,
    separator: Option<i64>,
    value: Option<i64>,
}

impl Deltas {
    fn is_zero(self) -> bool {
        [self.key, self.separator, self.value]
            .into_iter()
            .flatten()
            .all(|delta| delta == 0)
    }
}

fn operator_of(pair: Node<'_>) -> Option<Range<usize>> {
    let mut cursor = pair.walk();
    pair.children(&mut cursor)
        .find(|child| matches!(child.kind_str(), ":" | "=>"))
        .map(|child| child.byte_range())
}

/// `same_line?(selector, node.pairs.first)` with the cop's own override, which also accepts the
/// anchor's last line.
fn starts_beside_its_call(
    context: &RuleContext<'_>,
    first: Node<'_>,
    elements: &[Element],
) -> bool {
    let Some(pair) = elements.iter().find(|element| !element.kwsplat) else {
        return false;
    };
    let Some(parent) = first.parent_of(context) else {
        return false;
    };
    let Some(call) = parent.parent_of(context) else {
        return false;
    };
    let anchor = first
        .prev_named_sibling()
        .or_else(|| call.field("method"))
        .unwrap_or(call);
    let anchor_first = context.source.line_column(anchor.start_byte()).0;
    let anchor_last = context.source.line_column(anchor.end_byte()).0;
    anchor_first == pair.start.line || anchor_last == pair.start.line
}

/// `on_send` and `ignore_hash_argument?`: whether this literal is the hash a call takes last, and
/// the setting says to leave it be.
///
/// Upstream reaches the call and marks the hash with `ignore_node`; here the literal is asked
/// which call, if any, it is the last argument of. A braced hash is a node of its own, so the walk
/// starts one level higher for it than for a brace-less run of pairs.
fn ignored_last_argument(elements: &[Node<'_>], context: &RuleContext<'_>, style: &str) -> bool {
    if style == "always_inspect" {
        return false;
    }
    let (Some(first), Some(final_element)) = (elements.first(), elements.last()) else {
        return false;
    };
    let Some(parent) = first.parent_of(context) else {
        return false;
    };
    let (list, hash_node, braces) = match parent.kind_str() {
        "hash" => (parent.parent_of(context), Some(parent), true),
        "argument_list" | "element_reference" => (Some(parent), None, false),
        _ => return false,
    };
    let Some(list) = list else { return false };
    let wanted = hash_node.unwrap_or(*final_element);
    // `node.last_argument`. A comment is a node here and nothing at all upstream.
    let is_last_argument = match list.kind_str() {
        // `on_send`, `on_csend`, `on_super` and `on_yield`: an array literal is none of them.
        "argument_list" | "element_reference" => {
            let mut cursor = list.walk();
            list.named_children(&mut cursor)
                .filter(|child| !matches!(child.kind_str(), "comment" | "heredoc_body"))
                .last()
                .is_some_and(|node| node.id() == wanted.id())
        }
        // `x.foo = { … }` and `x[k] = { … }` are `send :foo=` and `send :[]=` to upstream's
        // parser, so the hash is the call's last argument there. The grammar files both as
        // assignments, which is why the walk has to look through one.
        "assignment" => {
            list.field("left")
                .is_some_and(|left| matches!(left.kind_str(), "call" | "element_reference"))
                && list.field("right") == Some(wanted)
        }
        // `a << { … }`: an operator method is a send too, and the hash is its only argument.
        // `&&`, `||`, `and` and `or` are not sends -- upstream builds `and`/`or` nodes for them --
        // so a hash to the right of one is nobody's argument.
        "binary" => {
            list.field("operator").is_some_and(|operator| {
                !matches!(
                    &context.source.text()[operator.byte_range()],
                    "&&" | "||" | "and" | "or"
                )
            }) && list.field("right") == Some(wanted)
        }
        _ => false,
    };
    if !is_last_argument {
        return false;
    }
    match style {
        "always_ignore" => true,
        "ignore_explicit" => braces,
        "ignore_implicit" => !braces,
        _ => false,
    }
}
