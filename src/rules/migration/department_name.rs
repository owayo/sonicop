use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// `DISABLE_COMMENT_FORMAT`, split into the directive and what it names.
static DISABLE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(# *rubocop *: *((dis|en)able|todo) +)(.*)").unwrap());

/// The scan upstream writes as `/[^,]+|\W+/`, which walks a comma-separated list one name and one
/// separator at a time. Ruby's `\W` is ASCII-only, so the class is spelled out rather than left to
/// Rust's Unicode-aware `\W`.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^,]+|[^0-9A-Za-z_]+").unwrap());

/// A name that reaches past what a department name can hold ends the scan, because whatever
/// follows is prose rather than another cop.
static UNEXPECTED_CHARACTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^A-Za-z/, ]").unwrap());

/// `Registry.global.departments` for a run with no plugins loaded, which is every department the
/// bundled configuration knows.
const DEPARTMENTS: &[&str] = &[
    "Bundler",
    "Gemspec",
    "Layout",
    "Lint",
    "Metrics",
    "Migration",
    "Naming",
    "Security",
    "Style",
];

/// `RuboCop::ConfigObsoletion.legacy_cop_names`: cops that have since been renamed, removed or
/// moved out into an extension. A directive naming one of them can still be qualified, by the
/// department the cop had when it was written.
const LEGACY_COP_NAMES: &[&str] = &[
    "Layout/AlignArguments",
    "Layout/AlignArray",
    "Layout/AlignHash",
    "Layout/AlignParameters",
    "Layout/IndentArray",
    "Layout/IndentAssignment",
    "Layout/IndentFirstArgument",
    "Layout/IndentFirstArrayElement",
    "Layout/IndentFirstHashElement",
    "Layout/IndentFirstParameter",
    "Layout/IndentHash",
    "Layout/IndentHeredoc",
    "Layout/LeadingBlankLines",
    "Layout/Tab",
    "Layout/TrailingBlankLines",
    "Lint/BlockAlignment",
    "Lint/DefEndAlignment",
    "Lint/DuplicatedKey",
    "Lint/EndAlignment",
    "Lint/EndInMethod",
    "Lint/Eval",
    "Lint/HandleExceptions",
    "Lint/MultipleCompare",
    "Lint/StringConversionInInterpolation",
    "Lint/UnneededCopDisableDirective",
    "Lint/UnneededCopEnableDirective",
    "Lint/UnneededRequireStatement",
    "Lint/UnneededSplatExpansion",
    "Metrics/LineLength",
    "Naming/PredicateName",
    "Naming/UncommunicativeBlockParamName",
    "Naming/UncommunicativeMethodParamName",
    "Style/AccessorMethodName",
    "Style/AsciiIdentifiers",
    "Style/ClassAndModuleCamelCase",
    "Style/ConstantName",
    "Style/DeprecatedHashMethods",
    "Style/FileName",
    "Style/FlipFlop",
    "Style/MethodCallParentheses",
    "Style/MethodName",
    "Style/OpMethod",
    "Style/PredicateName",
    "Style/SingleSpaceBeforeFirstArg",
    "Style/UnneededCapitalW",
    "Style/UnneededCondition",
    "Style/UnneededInterpolation",
    "Style/UnneededPercentQ",
    "Style/UnneededSort",
    "Style/VariableName",
    "Style/VariableNumber",
    "Gemspec/DateAssignment",
    "Layout/SpaceAfterControlKeyword",
    "Layout/SpaceBeforeModifierKeyword",
    "Lint/InvalidCharacterLiteral",
    "Lint/RescueWithoutErrorClass",
    "Lint/SpaceBeforeFirstArg",
    "Lint/UselessComparison",
    "Style/BracesAroundHashParameters",
    "Style/MethodMissingSuper",
    "Style/SpaceAfterControlKeyword",
    "Style/SpaceBeforeModifierKeyword",
    "Style/TrailingComma",
    "Style/TrailingCommaInLiteral",
    "Style/MethodMissing",
    "Performance/*",
    "Rails/*",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for comment in context.comment_ranges() {
        let text = context.source.slice(comment.clone());
        let Some(directive) = DISABLE_COMMENT.captures(text) else {
            continue;
        };
        // Upstream counts in characters, because its positions are character offsets.
        let mut offset = directive[1].chars().count();
        for token in TOKEN.find_iter(&directive[4]) {
            let token = token.as_str();
            let name = token.trim();
            if !valid_content_token(name) {
                report(comment.start, offset, name, context, offenses);
            }
            if UNEXPECTED_CHARACTER.is_match(token) {
                break;
            }
            offset += token.chars().count();
        }
    }
}

fn report(
    comment_start: usize,
    offset: usize,
    name: &str,
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
) {
    let start = advance(comment_start, offset, context);
    let end = advance(start, name.chars().count(), context);
    let offense = context.offense("Department name is missing.", start..end);
    // The correction only exists when the name can be qualified. Upstream reaches
    // `corrector.replace(range, nil)` when it cannot, which leaves the corrector empty and the
    // offense reported as one nothing can fix.
    offenses.push(match qualified_cop_name(name, context) {
        Some(qualified) => offense.corrected_by(Edit {
            start,
            end,
            replacement: qualified,
            safe: true,
        }),
        None => offense,
    });
}

/// The byte offset `count` characters past `start`.
fn advance(start: usize, count: usize, context: &RuleContext<'_>) -> usize {
    context.source.text()[start..]
        .char_indices()
        .nth(count)
        .map_or(context.source.len(), |(offset, _)| start + offset)
}

/// `valid_content_token?`: a separator, something already qualified, `all`, or a department name.
///
/// The first two tests are both partial matches upstream, so anything holding a non-word character
/// passes -- which covers every qualified name -- and so does anything that merely contains `all`.
fn valid_content_token(token: &str) -> bool {
    token.contains(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        || token.contains("all")
        || DEPARTMENTS.contains(&token)
}

/// `Registry.global.qualified_cop_name` followed by `qualified_legacy_cop_name`: the department
/// that has a cop by this name, or the department the cop had before it was renamed away.
fn qualified_cop_name(name: &str, context: &RuleContext<'_>) -> Option<String> {
    let mut qualified = DEPARTMENTS
        .iter()
        .map(|department| format!("{department}/{name}"))
        .filter(|candidate| is_cop(candidate, context));
    // Upstream raises on an ambiguous name rather than picking one, so a name two departments
    // answer to is left uncorrected here.
    match (qualified.next(), qualified.next()) {
        (Some(only), None) => Some(only),
        (Some(_), Some(_)) => None,
        _ => LEGACY_COP_NAMES
            .iter()
            .find(|legacy| legacy.split('/').nth(1) == Some(name))
            .map(|legacy| (*legacy).to_owned()),
    }
}

/// Whether the configuration knows a cop by this name. Every cop upstream registers carries a
/// `Description` in `config/default.yml`, and nothing else in the file is keyed by a cop name.
fn is_cop(name: &str, context: &RuleContext<'_>) -> bool {
    context.setting_of::<String>(name, "Description").is_some()
}
