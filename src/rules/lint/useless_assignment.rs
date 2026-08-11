use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::variable_force::{
    Analysis, Assignment, AssignmentKind, Scope, Variable, body_node, named_children, scope_nodes,
    spurious_assignment_list,
};
use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let analysis = Analysis::run(context.root_node(), context.source);
    // `ignore_node` keeps a reported assignment from being reported a second time through an
    // assignment nested inside it, and holds for the rest of the file.
    let mut ignored: Vec<Range<usize>> = Vec::new();
    for scope in &analysis.scopes {
        for &index in &scope.variables {
            let variable = &analysis.variables[index];
            if variable.should_be_unused() {
                continue;
            }
            for position in (0..variable.assignments.len()).rev() {
                report(
                    context,
                    offenses,
                    &analysis,
                    scope,
                    variable,
                    position,
                    &mut ignored,
                );
            }
        }
    }
}

fn report(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    analysis: &Analysis<'_>,
    scope: &Scope<'_>,
    variable: &Variable<'_>,
    position: usize,
    ignored: &mut Vec<Range<usize>>,
) {
    let assignment = &variable.assignments[position];
    let node = assignment.node;
    let range = written_range(assignment);
    if variable.assignment_used(position)
        || ignored.iter().any(|ignored| covers(ignored, &range))
        || variable_in_loop_condition(node, &variable.name, context)
    {
        return;
    }
    let message = format!("Useless assignment to variable - `{}`.", variable.name)
        + &specification(context, analysis, scope, variable, node);
    let offense = context.offense(message, assignment.name.byte_range());
    offenses.push(match autocorrect(context, assignment, &variable.name) {
        Some(edit) if !uncorrectable(node) => offense.corrected_by(edit),
        _ => offense,
    });
    if assignment.value.is_some_and(chained_value) {
        ignored.push(range);
    }
}

/// What the `lvasgn` upstream covers: the name and, when there is one, the value it stores.
fn written_range(assignment: &Assignment<'_>) -> Range<usize> {
    let start = assignment.name.start_byte();
    start
        ..assignment
            .value
            .map_or(assignment.name.end_byte(), |value| value.end_byte())
}

fn covers(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

fn specification(
    context: &RuleContext<'_>,
    analysis: &Analysis<'_>,
    scope: &Scope<'_>,
    variable: &Variable<'_>,
    node: Node<'_>,
) -> String {
    if multiple_assignment(node) {
        return format!(
            " Use `_` or `_{0}` as a variable name to indicate that it won't be used.",
            variable.name
        );
    }
    if let Some(operator_assignment) = operator_assignment(node) {
        return operator_message(scope, operator_assignment, context);
    }
    similar_name_message(context, analysis, scope, variable)
}

/// The suggestion RuboCop makes when the scope holds a name close enough to be a typo of the one
/// nothing reads.
fn similar_name_message(
    context: &RuleContext<'_>,
    analysis: &Analysis<'_>,
    scope: &Scope<'_>,
    variable: &Variable<'_>,
) -> String {
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for node in scope_nodes(scope) {
        if let Some(name) = variable_like_invocation(node, context, analysis)
            && seen.insert(name.clone())
        {
            names.push(name);
        }
    }
    for &index in &scope.variables {
        let name = analysis.variables[index].name.clone();
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    match find_similar_name(&variable.name, &names) {
        Some(similar) => format!(" Did you mean `{similar}`?"),
        None => String::new(),
    }
}

/// `variable_like_method_invocation?`: a receiverless call without arguments, which reads like a
/// variable and so may well be the name that was meant.
fn variable_like_invocation(
    node: Node<'_>,
    context: &RuleContext<'_>,
    analysis: &Analysis<'_>,
) -> Option<String> {
    match node.kind() {
        // A bare name is a `send` upstream only when it did not resolve to a local; a read of a
        // local is an `lvar` and contributes nothing of its own here.
        "identifier" if !analysis.is_variable_reference(node) => {
            let parent = node.parent()?;
            let named = matches!(
                parent.kind(),
                "call"
                    | "method"
                    | "singleton_method"
                    | "optional_parameter"
                    | "keyword_parameter"
                    | "splat_parameter"
                    | "hash_splat_parameter"
                    | "block_parameter"
                    | "alias"
                    | "undef"
                    | "setter"
                    | "method_parameters"
                    | "block_parameters"
                    | "lambda_parameters"
            );
            let assigned = matches!(parent.kind(), "assignment" | "operator_assignment")
                && parent
                    .child_by_field_name("left")
                    .is_some_and(|left| left.id() == node.id());
            (!named && !assigned).then(|| context.source.node_text(node).to_owned())
        }
        "call" => {
            let method = node.child_by_field_name("method")?;
            let bare = node.child_by_field_name("receiver").is_none()
                && node
                    .child_by_field_name("arguments")
                    .is_none_or(|arguments| arguments.named_child_count() == 0);
            bare.then(|| context.source.node_text(method).to_owned())
        }
        _ => None,
    }
}

/// `operator_assignment_message`: only worth saying when the assignment is what the scope returns,
/// because then the write itself is what can be dropped.
fn operator_message(
    scope: &Scope<'_>,
    operator_assignment: Node<'_>,
    context: &RuleContext<'_>,
) -> String {
    let Some(return_value) = return_value_node(scope) else {
        return String::new();
    };
    if return_value.id() != operator_assignment.id() {
        return String::new();
    }
    let Some(operator) = operator_token(operator_assignment, context) else {
        return String::new();
    };
    let stripped = operator.strip_suffix('=').unwrap_or(&operator);
    format!(" Use `{stripped}` instead of `{operator}`.")
}

fn return_value_node<'tree>(scope: &Scope<'tree>) -> Option<Node<'tree>> {
    let body = body_node(scope)?;
    named_children(body).last().copied().or(Some(body))
}

fn operator_token(node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    Some(
        context.source.text()[left.end_byte()..right.start_byte()]
            .trim()
            .to_owned(),
    )
}

// ---------------------------------------------------------------------------
// Assignment shapes
// ---------------------------------------------------------------------------

/// Whether the write is one target of a genuine `masgn`, which cannot simply be deleted.
fn multiple_assignment(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "rest_assignment" | "destructured_left_assignment" => current = parent,
            "left_assignment_list" => return !spurious_assignment_list(parent),
            _ => return false,
        }
    }
    false
}

fn operator_assignment<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    node.parent()
        .filter(|parent| parent.kind() == "operator_assignment")
        .filter(|parent| {
            parent
                .child_by_field_name("left")
                .is_some_and(|left| left.id() == node.id())
        })
}

fn for_assignment(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "for" => return true,
            "left_assignment_list" | "rest_assignment" | "destructured_left_assignment" => {
                current = parent;
            }
            _ => return false,
        }
    }
    false
}

fn exception_assignment(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "exception_variable")
}

/// `chained_assignment?`: reporting `a = b = 1` covers the write nested inside it, and reporting
/// `a = foo(b = 1)` covers that one too. Both tests upstream are about what the value is, so an
/// assignment that stores nothing -- a `masgn` target, a `for` variable -- never chains.
fn chained_value(value: Node<'_>) -> bool {
    matches!(
        value.kind(),
        "call" | "identifier" | "binary" | "unary" | "element_reference" | "assignment"
    )
}

/// Assignments RuboCop declines to correct: removing one target of `x = 1, y = 2` is a syntax
/// error, and rewriting `x ||= 1` to `x = 1` can raise `NameError`.
fn uncorrectable(node: Node<'_>) -> bool {
    if node.parent().is_some_and(|parent| {
        parent.kind() == "operator_assignment" && matches!(operator_of(parent), Some("||=" | "&&="))
    }) {
        return true;
    }
    let mut current = Some(node);
    while let Some(candidate) = current {
        if candidate.kind() == "assignment"
            && candidate
                .child_by_field_name("left")
                .is_some_and(|left| left.kind() == "identifier")
            && candidate
                .child_by_field_name("right")
                .is_some_and(|right| matches!(right.kind(), "array" | "right_assignment_list"))
            && contains_assignment(candidate)
        {
            return true;
        }
        current = candidate.parent();
    }
    false
}

fn contains_assignment(node: Node<'_>) -> bool {
    named_children(node).into_iter().any(|child| {
        matches!(child.kind(), "assignment" | "operator_assignment") || contains_assignment(child)
    })
}

fn operator_of(node: Node<'_>) -> Option<&'static str> {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        let child = cursor.node();
        if !child.is_named() {
            return Some(child.kind());
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

/// `variable_in_loop_condition?`: a write the loop's own condition reads runs again before the
/// condition is tested, so it is not dead.
fn variable_in_loop_condition(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    let mut current = node;
    let mut loop_node = None;
    while let Some(parent) = current.parent() {
        if matches!(parent.kind(), "method" | "singleton_method") {
            return false;
        }
        if loop_node.is_none()
            && matches!(
                parent.kind(),
                "while" | "until" | "while_modifier" | "until_modifier" | "for"
            )
        {
            loop_node = Some(parent);
        }
        current = parent;
    }
    let Some(condition) = loop_node.and_then(|node| node.child_by_field_name("condition")) else {
        return false;
    };
    reads_name(condition, name, context)
}

/// Whether the subtree reads a local of this name: an `lvar` upstream, which rules out the name of
/// a method being called and the target of an assignment.
fn reads_name(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    if node.kind() == "identifier" {
        if context.source.node_text(node) != name {
            return false;
        }
        let Some(parent) = node.parent() else {
            return true;
        };
        let target = matches!(parent.kind(), "assignment" | "operator_assignment")
            && parent
                .child_by_field_name("left")
                .is_some_and(|left| left.id() == node.id());
        let method = parent.kind() == "call"
            && parent
                .child_by_field_name("method")
                .is_some_and(|method| method.id() == node.id());
        return !target && !method;
    }
    named_children(node)
        .into_iter()
        .any(|child| reads_name(child, name, context))
}

// ---------------------------------------------------------------------------
// Autocorrection
// ---------------------------------------------------------------------------

fn autocorrect(context: &RuleContext<'_>, assignment: &Assignment<'_>, name: &str) -> Option<Edit> {
    let node = assignment.node;
    if exception_assignment(node) {
        let clause = node.parent()?.parent()?;
        let start = clause.child_by_field_name("exceptions").map_or_else(
            || clause.start_byte() + "rescue".len(),
            |list| list.end_byte(),
        );
        return Some(removal(start, node.end_byte()));
    }
    if multiple_assignment(node) || for_assignment(node) {
        return Some(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: "_".to_owned(),
            safe: true,
        });
    }
    if let Some(assignment) = operator_assignment(node) {
        let right = assignment.child_by_field_name("right")?;
        let text = context.source.text();
        let end = text[..right.start_byte()].trim_end().len();
        return Some(removal(end - 1, end));
    }
    if assignment.kind == AssignmentKind::RegexpNamedCapture {
        let regexp = assignment.name;
        let source = context.source.node_text(regexp);
        let group = format!("(?<{name}>");
        let start = source.find(&group)?;
        return Some(Edit {
            start: regexp.start_byte() + start,
            end: regexp.start_byte() + start + group.len(),
            replacement: "(?:".to_owned(),
            safe: true,
        });
    }
    Some(removal(
        assignment.name.start_byte(),
        assignment.value?.start_byte(),
    ))
}

fn removal(start: usize, end: usize) -> Edit {
    Edit {
        start,
        end,
        replacement: String::new(),
        safe: true,
    }
}

// ---------------------------------------------------------------------------
// Name similarity
// ---------------------------------------------------------------------------

/// `DidYouMean::SpellChecker#correct`, which RuboCop calls to build the "Did you mean" hint.
/// Reproducing the message means reproducing the ranking, so the two distance functions below are
/// ported from Ruby's own implementation rather than replaced with equivalents.
fn find_similar_name(target: &str, names: &[String]) -> Option<String> {
    let input = normalize(target);
    let threshold = if input.chars().count() > 3 {
        0.834
    } else {
        0.77
    };
    let mut words: Vec<&String> = names
        .iter()
        .filter(|name| name.as_str() != target)
        .filter(|name| jaro_winkler(&normalize(name), &input) >= threshold)
        .collect();
    words.sort_by(|a, b| {
        jaro_winkler(a, &input)
            .partial_cmp(&jaro_winkler(b, &input))
            .expect("distances are never NaN")
    });
    words.reverse();

    let mistype_threshold = (input.chars().count() as f64 * 0.25).ceil() as usize;
    if let Some(word) = words
        .iter()
        .find(|word| levenshtein(&normalize(word), &input) <= mistype_threshold)
    {
        return Some((*word).clone());
    }
    words
        .iter()
        .find(|word| {
            let word = normalize(word);
            let length = input.chars().count().min(word.chars().count());
            levenshtein(&word, &input) < length
        })
        .map(|word| (*word).clone())
}

fn normalize(name: &str) -> String {
    name.to_lowercase().replace('@', "")
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, left) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let cost = usize::from(left != right);
            let candidate = (row[j + 1] + 1).min(row[j] + 1).min(previous + cost);
            previous = row[j + 1];
            row[j + 1] = candidate;
        }
    }
    row[b.len()]
}

/// `DidYouMean::Jaro.distance`, ported with its own matching window and transposition count.
fn jaro(a: &str, b: &str) -> f64 {
    let (short, long) = {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        if a.len() > b.len() { (b, a) } else { (a, b) }
    };
    let (length1, length2) = (short.len(), long.len());
    if length1 == 0 {
        return 0.0;
    }
    let range = if length2 > 3 { length2 / 2 - 1 } else { 0 };
    let mut flags1 = vec![false; length1];
    let mut flags2 = vec![false; length2];
    let mut matches = 0.0_f64;
    for i in 0..length1 {
        let last = i + range;
        let mut j = i.saturating_sub(range);
        while j <= last {
            if j < length2 && !flags2[j] && short[i] == long[j] {
                flags2[j] = true;
                flags1[i] = true;
                matches += 1.0;
                break;
            }
            j += 1;
        }
    }
    let mut transpositions = 0.0_f64;
    let mut k = 0;
    for i in 0..length1 {
        if !flags1[i] {
            continue;
        }
        let mut j = k;
        let mut index = k;
        let mut next = None;
        while j < length2 {
            index = j;
            if flags2[j] {
                next = Some(j + 1);
                break;
            }
            j += 1;
        }
        let Some(next) = next else { break };
        k = next;
        if index < length2 && short[i] != long[index] {
            transpositions += 1.0;
        }
    }
    let transpositions = (transpositions / 2.0).floor();
    if matches == 0.0 {
        return 0.0;
    }
    (matches / length1 as f64 + matches / length2 as f64 + (matches - transpositions) / matches)
        / 3.0
}

fn jaro_winkler(a: &str, b: &str) -> f64 {
    let distance = jaro(a, b);
    if distance <= 0.7 {
        return distance;
    }
    let b: Vec<char> = b.chars().collect();
    let mut prefix = 0;
    for character in a.chars() {
        if prefix < 4 && b.get(prefix) == Some(&character) {
            prefix += 1;
        } else {
            break;
        }
    }
    distance + prefix as f64 * 0.1 * (1.0 - distance)
}
