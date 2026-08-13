use std::cell::OnceCell;
use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use crate::rules::lint::variable_force::Analysis;
use crate::rules::metrics::fragments::Fragments;
use crate::rules::metrics::locals::Locals;
use crate::rules::naming::support::Variables;

use crate::config::Config;
use crate::diagnostic::{Offense, Severity};
use crate::directives::{CommentConfig, CopRegistry};
use crate::formatter::smart_path;
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

mod bundler;
mod gemspec;
mod layout;
mod lint;
mod metrics;
mod migration;
mod naming;
mod ordered_gem;
mod regex_cache;
mod security;
mod send_node;
mod style;
mod support;

pub(crate) use support::{push_named_children, walk_named};

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
    bundler::RULES,
    gemspec::RULES,
    layout::RULES,
    lint::RULES,
    metrics::RULES,
    migration::RULES,
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
    /// RuboCop's `autocorrect?`. See [`crate::engine::Selection::correcting`].
    correcting: bool,
    /// Set only for the cop that reads the file's directives rather than its syntax tree.
    directive_review: Option<&'a DirectiveReview<'a>>,
    /// RuboCop's `VariableForce`, run at most once per file.
    ///
    /// Upstream shares the analysis through the commissioner: `VariableForce` is one force the
    /// whole team is investigated with, and the six cops built on it are handed its result rather
    /// than each running it. Thirty-odd cops here ask about it, so the run belongs to the file,
    /// not to whichever cop asked first -- and since it reads nothing but the tree and the source,
    /// one run answers all of them identically.
    ///
    /// The context is reused across every cop of a file for this reason; see
    /// [`Self::inspecting_with`].
    analysis: OnceCell<Analysis<'a>>,
    /// The Naming department's own reading of which names are variables, run at most once per
    /// file. Five cops ask for it, and like [`Self::variable_analysis`] it depends on nothing but
    /// the tree and the source.
    variables: OnceCell<Variables>,
    /// The code the grammar swallowed, recovered once per file. The three complexity cops each
    /// used to recover it for themselves.
    fragments: OnceCell<Fragments>,
    /// Which identifiers the Metrics cops read as local variables, replayed once per file.
    metric_locals: OnceCell<Locals>,
}

/// What `Lint/RedundantCopDisableDirective` is given instead of a walk over the syntax tree.
///
/// RuboCop mobilizes that cop on its own once every other cop has finished and assigns it
/// `offenses_to_check`, because whether a `rubocop:disable` was needed can only be answered from
/// the offenses the rest of the run found. Nothing else in the registry has that shape, so the
/// input travels here rather than widening every cop's signature.
pub(crate) struct DirectiveReview<'a> {
    /// Every offense the run found in this file, including the ones a directive suppressed.
    pub offenses: &'a [Offense],
    pub comments: &'a CommentConfig,
    pub registry: &'a CopRegistry,
}

impl<'a> RuleContext<'a> {
    pub(crate) fn new(
        source: &'a SourceFile,
        ast: &'a AstIndex<'a>,
        config: &'a Config,
        rule: &'static Rule,
        severity: Severity,
        correcting: bool,
    ) -> Self {
        Self {
            source,
            ast,
            config,
            rule,
            severity,
            correcting,
            directive_review: None,
            analysis: OnceCell::new(),
            variables: OnceCell::new(),
            fragments: OnceCell::new(),
            metric_locals: OnceCell::new(),
        }
    }

    /// Points the context at the next cop of the same file, keeping everything the file's cops
    /// share -- above all [`Self::variable_analysis`], which would otherwise be run once per cop
    /// that asks for it.
    pub(crate) fn inspecting_with(&mut self, rule: &'static Rule, severity: Severity) {
        self.rule = rule;
        self.severity = severity;
    }

    /// Hands the cop that reads directives what the rest of the run found. See [`DirectiveReview`].
    pub(crate) fn reviewing_directives(mut self, review: &'a DirectiveReview<'a>) -> Self {
        self.directive_review = Some(review);
        self
    }

    /// RuboCop's `VariableForce` result for this file, computed on the first cop that asks.
    pub(in crate::rules) fn variable_analysis(&self) -> &Analysis<'a> {
        self.analysis
            .get_or_init(|| Analysis::run(self.ast.root, self.source))
    }

    /// Which names in the file are variables, as the Naming cops read them.
    pub(in crate::rules) fn variable_roles(&self) -> &Variables {
        self.variables
            .get_or_init(|| Variables::resolve(self.ast.root, self.source))
    }

    /// The code the grammar read as something other than code, recovered once per file.
    pub(in crate::rules) fn fragments(&self) -> &Fragments {
        self.fragments.get_or_init(|| Fragments::new(self))
    }

    /// Which identifiers the Metrics cops read as local variables.
    pub(in crate::rules) fn metric_locals(&self) -> &Locals {
        self.metric_locals
            .get_or_init(|| Locals::new(self, self.fragments()))
    }
}

impl<'a> RuleContext<'a> {
    /// RuboCop's `autocorrect?`: whether this run was asked to rewrite the file. A cop only needs
    /// this to decide something it cannot decide from the source, which is rare -- normally a cop
    /// attaches its edits and lets the engine decide whether to apply them.
    pub fn correcting(&self) -> bool {
        self.correcting
    }

    /// The offenses and directive analysis `Lint/RedundantCopDisableDirective` runs on, or `None`
    /// for every other cop and for the passes that do not check directives at all.
    pub fn directive_review(&self) -> Option<&DirectiveReview<'_>> {
        self.directive_review
    }

    /// One of the cop's own configuration parameters, such as `Max` or `EnforcedStyle`.
    pub fn setting<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.config.cop_value(self.rule.name, key)
    }

    /// Another cop's configuration parameter, the way RuboCop's cops reach for
    /// `config.for_cop('Layout/HashAlignment')`. A cop whose own behaviour is defined in terms of
    /// a neighbour's configuration has to read that neighbour, not guess at its default.
    pub fn setting_of<T: serde::de::DeserializeOwned>(&self, cop: &str, key: &str) -> Option<T> {
        self.config.cop_value(cop, key)
    }

    /// Whether another cop is switched on, the way RuboCop's cops ask
    /// `config.cop_enabled?('Lint/SafeNavigationChain')`. A cop that leaves work to a neighbour
    /// has to know whether the neighbour will do it.
    pub fn cop_enabled(&self, cop: &str) -> bool {
        self.config.rule_enabled(cop)
    }

    /// The Ruby version the run analyzes as, which version-gated cops compare against.
    pub fn target_ruby_version(&self) -> RubyVersion {
        self.config.target_ruby_version()
    }

    /// The file's path as RuboCop writes it into an offense message: relative to the directory the
    /// run started in, or absolute when it lies outside. Cops that name a second location, such as
    /// the other definition `Lint/DuplicateMethods` points at, must go through this rather than
    /// print the path they were handed.
    pub fn display_path(&self) -> String {
        smart_path(self.source.path(), self.config.cwd())
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

    pub fn root_node(&self) -> Node<'a> {
        self.ast.root
    }

    pub fn nodes(&self) -> impl Iterator<Item = Node<'a>> + '_ {
        self.ast.nodes.iter().copied()
    }

    /// The named nodes of one kind, in source order. A cop that inspects a single kind should
    /// reach for this rather than filtering every node in the file: with hundreds of cops running
    /// per file, a full walk each is what turns inspection quadratic.
    pub fn nodes_of(&self, kind: &str) -> impl Iterator<Item = Node<'a>> + '_ {
        self.ast
            .of_kind(kind)
            .map(|index| self.ast.named_node(index))
    }

    /// The named nodes of any of `kinds`, in source order. The kinds are indexed separately, so
    /// their positions have to be merged to put the nodes back in the order a cop that scans the
    /// whole file would have seen them in.
    pub fn nodes_of_any(&self, kinds: &[&str]) -> impl Iterator<Item = Node<'a>> + '_ {
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
