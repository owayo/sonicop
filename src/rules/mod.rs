use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use crate::config::Config;
use crate::diagnostic::{Offense, Severity};
use crate::ruby_version::RubyVersion;
use crate::source::SourceFile;

/// Registers one department's cops. Each entry names the module the cop lives in, the cop's own
/// name within the department, and the severity it reports at unless the configuration overrides
/// it.
///
/// This is the department's only source of truth: the module declaration, the qualified cop name
/// and the default severity all come from the single line here, so a cop file never repeats its
/// own name. A cop that spelled its name a second time could disagree with the registry, and
/// nothing in the type system would catch it -- the offense would simply be attributed to a cop
/// that never ran, and directives and severity overrides would both consult the wrong entry.
macro_rules! department_rules {
    ($department:literal; $($module:ident => ($cop:literal, $severity:ident)),+ $(,)?) => {
        $(mod $module;)+

        pub(crate) static RULES: &[$crate::rules::Rule] = &[
            $($crate::rules::Rule::new(
                concat!($department, "/", $cop),
                $crate::diagnostic::Severity::$severity,
                $module::check,
            ),)+
        ];
    };
}

mod layout;
mod lint;
mod metrics;
mod naming;
mod security;
mod style;
mod support;

pub(crate) use support::{first_identifier, push_named_children, walk_named};

/// A cop: its qualified name, the severity it reports at by default, and the function that
/// inspects one file.
#[derive(Clone, Copy)]
pub(crate) struct Rule {
    pub name: &'static str,
    pub severity: Severity,
    pub check: fn(&RuleContext<'_>, &mut Vec<Offense>),
}

impl Rule {
    pub(crate) const fn new(
        name: &'static str,
        severity: Severity,
        check: fn(&RuleContext<'_>, &mut Vec<Offense>),
    ) -> Self {
        Self {
            name,
            severity,
            check,
        }
    }
}

/// Every department's registry, in the order cops run. Offenses are sorted before they are
/// reported, so this order is not user-visible; it only has to stay deterministic.
static RULE_GROUPS: &[&[Rule]] = &[
    layout::RULES,
    lint::RULES,
    metrics::RULES,
    naming::RULES,
    security::RULES,
    style::RULES,
];

pub(crate) fn rules() -> impl Iterator<Item = &'static Rule> {
    RULE_GROUPS.iter().copied().flatten()
}

pub fn rule_names() -> impl Iterator<Item = &'static str> {
    rules().map(|rule| rule.name)
}

/// What one cop sees of one file: the source, the indexed syntax tree, and the configuration
/// resolved for that cop.
///
/// The cop's identity lives here rather than in the cop's own code, so [`Self::setting`] and
/// [`Self::offense`] address the right configuration key and stamp the right name without the cop
/// ever naming itself.
pub(crate) struct RuleContext<'a> {
    pub source: &'a SourceFile,
    ast: &'a AstIndex<'a>,
    config: &'a Config,
    rule: &'static Rule,
    /// `rule.severity` unless the configuration overrode it for this cop.
    severity: Severity,
}

impl<'a> RuleContext<'a> {
    pub(crate) fn new(
        source: &'a SourceFile,
        ast: &'a AstIndex<'a>,
        config: &'a Config,
        rule: &'static Rule,
        severity: Severity,
    ) -> Self {
        Self {
            source,
            ast,
            config,
            rule,
            severity,
        }
    }
}

impl RuleContext<'_> {
    /// One of the cop's own configuration parameters, such as `Max` or `EnforcedStyle`.
    pub fn setting<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.config.cop_value(self.rule.name, key)
    }

    /// The Ruby version the run analyzes as, which version-gated cops compare against.
    pub fn target_ruby_version(&self) -> RubyVersion {
        self.config.target_ruby_version()
    }

    /// Reports `range` under this cop's name and severity. Every cop offense is built here.
    pub fn offense(&self, message: impl Into<String>, range: Range<usize>) -> Offense {
        Offense::new(
            self.rule.name,
            self.severity,
            message,
            range.start,
            range.end,
        )
    }

    pub fn root_node(&self) -> Node<'_> {
        self.ast.root
    }

    pub fn nodes(&self) -> impl Iterator<Item = Node<'_>> + '_ {
        self.ast.nodes.iter().copied()
    }

    /// The named nodes of one kind, in source order. A cop that inspects a single kind should
    /// reach for this rather than filtering every node in the file: with hundreds of cops running
    /// per file, a full walk each is what turns inspection quadratic.
    pub fn nodes_of(&self, kind: &str) -> impl Iterator<Item = Node<'_>> + '_ {
        self.ast
            .of_kind(kind)
            .map(|index| self.ast.named_node(index))
    }

    /// The named nodes of any of `kinds`, in source order. The kinds are indexed separately, so
    /// their positions have to be merged to put the nodes back in the order a cop that scans the
    /// whole file would have seen them in.
    pub fn nodes_of_any(&self, kinds: &[&str]) -> impl Iterator<Item = Node<'_>> + '_ {
        let mut indices: Vec<u32> = kinds
            .iter()
            .flat_map(|kind| self.ast.of_kind(kind))
            .collect();
        indices.sort_unstable();
        indices.into_iter().map(|index| self.ast.named_node(index))
    }

    pub fn protected_ranges(&self) -> &[Range<usize>] {
        &self.ast.protected_ranges
    }

    pub fn in_heredoc(&self, range: Range<usize>) -> bool {
        self.heredoc_count(range) > 0
    }

    pub fn heredoc_count(&self, range: Range<usize>) -> usize {
        self.ast
            .heredoc_ranges
            .iter()
            .filter(|heredoc| heredoc.start < range.end && range.start < heredoc.end)
            .count()
    }

    /// Comment spans in source order. A cop that reasons about what a *line* holds needs these:
    /// RuboCop's token stream excludes comments, so a trailing comment must not change whether a
    /// line counts as ending in some token.
    pub fn comment_ranges(&self) -> &[Range<usize>] {
        &self.ast.comment_ranges
    }
}

/// Node kinds whose byte range spans literal text rather than code. The
/// byte-scanning cops (`Style/Semicolon`, `Layout/SpaceAfterComma`,
/// `Layout/SpaceInsideParens`) must not report punctuation found inside them.
const PROTECTED_LITERAL_KINDS: &[&str] = &[
    "comment",
    "string",
    "symbol",
    "simple_symbol",
    "heredoc_body",
    "regex",
    "subshell",
    "bare_string",
];

pub(crate) struct AstIndex<'tree> {
    root: Node<'tree>,
    nodes: Vec<Node<'tree>>,
    named_nodes: Vec<Node<'tree>>,
    /// Positions in `named_nodes` grouped by node kind, each list in source order. Indices rather
    /// than nodes because a `Node` is eight times the size of the `u32` that finds it.
    by_kind: HashMap<&'static str, Vec<u32>>,
    protected_ranges: Vec<Range<usize>>,
    heredoc_ranges: Vec<Range<usize>>,
    comment_ranges: Vec<Range<usize>>,
}

impl<'tree> AstIndex<'tree> {
    pub fn new(root: Node<'tree>) -> Self {
        let mut index = Self {
            root,
            nodes: Vec::new(),
            named_nodes: Vec::new(),
            by_kind: HashMap::new(),
            protected_ranges: Vec::new(),
            heredoc_ranges: Vec::new(),
            comment_ranges: Vec::new(),
        };
        index.collect(root);
        index.protected_ranges.sort_by_key(|range| range.start);
        merge_touching_ranges(&mut index.protected_ranges);
        index.heredoc_ranges.sort_by_key(|range| range.start);
        index
    }

    pub fn comment_ranges(&self) -> &[Range<usize>] {
        &self.comment_ranges
    }

    fn of_kind(&self, kind: &str) -> impl Iterator<Item = u32> + '_ {
        self.by_kind
            .get(kind)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .copied()
    }

    fn named_node(&self, index: u32) -> Node<'tree> {
        self.named_nodes[index as usize]
    }

    /// Visits every node in depth-first pre-order. Iterative on purpose: rayon
    /// worker stacks are far smaller than the main thread's, and a recursive
    /// walk aborts the whole process on deeply nested input.
    fn collect(&mut self, root: Node<'tree>) {
        let mut cursor = root.walk();
        loop {
            self.visit(cursor.node());
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return;
                }
            }
        }
    }

    fn visit(&mut self, node: Node<'tree>) {
        self.nodes.push(node);
        if node.is_named() {
            // A file with more than u32::MAX named nodes would need tens of gigabytes of source;
            // the cast cannot lose information for anything a parser will accept.
            let index = self.named_nodes.len() as u32;
            self.named_nodes.push(node);
            self.by_kind.entry(node.kind()).or_default().push(index);
        }
        if PROTECTED_LITERAL_KINDS.contains(&node.kind()) {
            self.protected_ranges.push(node.byte_range());
        }
        if node.kind() == "heredoc_body" {
            self.heredoc_ranges.push(node.byte_range());
        }
        if node.kind() == "comment" {
            self.comment_ranges.push(node.byte_range());
        }
    }
}

/// Collapses overlapping and touching ranges of a start-sorted list so that no
/// offset is covered by more than one entry. `source::is_protected` inspects
/// only the last range starting at or before its offset, so an inner range that
/// outlived its enclosing one would make the enclosed offsets look unprotected.
fn merge_touching_ranges(ranges: &mut Vec<Range<usize>>) {
    if ranges.is_empty() {
        return;
    }
    let mut merged = 0;
    for index in 1..ranges.len() {
        if ranges[index].start <= ranges[merged].end {
            ranges[merged].end = ranges[merged].end.max(ranges[index].end);
        } else {
            merged += 1;
            ranges[merged] = ranges[index].clone();
        }
    }
    ranges.truncate(merged + 1);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{merge_touching_ranges, rule_names, rules};
    use crate::config::Config;
    use crate::source::is_protected;

    #[test]
    fn merges_nested_and_touching_ranges() {
        let mut ranges = vec![0..10, 3..6, 10..14, 20..25];
        merge_touching_ranges(&mut ranges);
        assert_eq!(ranges, vec![0..14, 20..25]);
    }

    #[test]
    fn leaves_disjoint_ranges_alone() {
        let mut ranges = vec![0..2, 5..7];
        merge_touching_ranges(&mut ranges);
        assert_eq!(ranges, vec![0..2, 5..7]);
    }

    // `is_protected` only consults the last range starting at or before the
    // offset, so an unmerged inner range hides the offsets after it.
    #[test]
    fn merging_keeps_offsets_under_an_inner_range_protected() {
        let nested = vec![0..10, 3..6];
        assert!(!is_protected(7, &nested));
        let mut merged = nested;
        merge_touching_ranges(&mut merged);
        assert!(is_protected(7, &merged));
        assert!(!is_protected(10, &merged));
    }

    #[test]
    fn every_cop_is_registered_once() {
        let mut seen = HashSet::new();
        for name in rule_names() {
            assert!(seen.insert(name), "{name} is registered twice");
        }
    }

    /// A registered name that the bundled RuboCop configuration does not know would be
    /// unreachable: `--only` rejects it, and it would carry no defaults.
    #[test]
    fn every_registered_cop_exists_in_the_default_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(None, directory.path()).unwrap();
        let known: HashSet<&str> = config.known_cop_names().collect();
        for name in rule_names() {
            assert!(known.contains(name), "{name} is not a RuboCop cop");
        }
    }

    #[test]
    fn every_cop_name_is_qualified_by_a_department() {
        for name in rule_names() {
            let (department, cop) = name.split_once('/').expect("{name} has no department");
            assert!(!department.is_empty(), "{name} has an empty department");
            assert!(!cop.is_empty(), "{name} has an empty cop name");
        }
    }

    /// The registry is a static built from the department tables, so iteration order cannot vary
    /// between runs; autocorrect ordering and `--debug` output both rely on that.
    #[test]
    fn registration_order_is_stable() {
        let first: Vec<&str> = rule_names().collect();
        let second: Vec<&str> = rules().map(|rule| rule.name).collect();
        assert_eq!(first, second);
    }
}
