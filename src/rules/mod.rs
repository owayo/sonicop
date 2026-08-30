use std::cell::OnceCell;
use std::collections::HashMap;
use std::ops::Range;

use tree_sitter::Node;

use crate::rules::lint::variable_force::Analysis;
use crate::rules::metrics::fragments::Fragments;
use crate::rules::metrics::locals::Locals;

use crate::config::Config;
use crate::diagnostic::{Offense, Severity};
use crate::directives::{CommentConfig, CopRegistry};
use crate::formatter::smart_path;
use crate::ruby_version::RubyVersion;
use crate::rules::node_ext::NodeExt;
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
mod node_ext;
mod ordered_gem;
mod regex_cache;
pub(crate) mod ruby_literal;
mod security;
mod send_node;
mod single_line;
mod style;
/// Reachable from `directives` too: Ruby's `\s` is one set, and the directive reader asks the
/// same question of a comment's prefix that a cop asks of the blanks beside a range.
pub(crate) mod support;
mod visibility;

pub(crate) use support::{push_named_children, push_named_children_in, walk_named};

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
    /// The code the grammar swallowed, recovered once per file. The three complexity cops each
    /// used to recover it for themselves.
    fragments: OnceCell<Fragments>,
    /// Which identifiers the Metrics cops read as local variables, replayed once per file.
    metric_locals: OnceCell<Locals>,
    /// The lexer token stream reconstructed from the tree, shared by the Layout and Style cops
    /// that inspect neighbouring tokens rather than syntax nodes.
    layout_tokens: OnceCell<Vec<layout::tokens::Token>>,
    /// The cops the run switches off through the configuration, which is what an `enable` directive
    /// has to undo.
    ///
    /// `Registry#disabled_names(config)` is the list, and it depends on the run's selection as much
    /// as on the configuration: it walks the *mobilized* registry, so `--only Foo` leaves one
    /// enabled cop in it and `--except` takes its cops out. The engine settles it once for the run
    /// instead of every cop working it out from the configuration alone.
    disabled_cops: &'a [&'static str],
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
            fragments: OnceCell::new(),
            metric_locals: OnceCell::new(),
            layout_tokens: OnceCell::new(),
            disabled_cops: &[],
        }
    }

    /// Records which cops the run switches off. See [`Self::disabled_cops`].
    pub(crate) fn with_disabled_cops(mut self, names: &'a [&'static str]) -> Self {
        self.disabled_cops = names;
        self
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
        self.analysis.get_or_init(|| {
            crate::profile::phase(crate::profile::Phase::Variables, || {
                Analysis::run(self.ast, self.source)
            })
        })
    }

    /// The code the grammar read as something other than code, recovered once per file.
    pub(in crate::rules) fn fragments(&self) -> &Fragments {
        self.fragments.get_or_init(|| {
            crate::profile::phase(crate::profile::Phase::Fragments, || Fragments::new(self))
        })
    }

    /// Which identifiers the Metrics cops read as local variables.
    pub(in crate::rules) fn metric_locals(&self) -> &Locals {
        self.metric_locals.get_or_init(|| {
            crate::profile::phase(crate::profile::Phase::MetricLocals, || {
                Locals::new(self, self.fragments())
            })
        })
    }

    /// RuboCop's lexer token stream for this file, reconstructed at most once however many cops
    /// inspect it.
    pub(in crate::rules) fn layout_tokens(&self) -> &[layout::tokens::Token] {
        self.layout_tokens.get_or_init(|| {
            crate::profile::phase(crate::profile::Phase::LayoutTokens, || {
                layout::tokens::build(self)
            })
        })
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

    /// `Registry#disabled_names(config)`: the cops the run switches off through the configuration.
    /// An `# rubocop:enable` that names one of them has something to undo even when the file
    /// disabled nothing itself.
    pub fn disabled_cops(&self) -> &[&'static str] {
        self.disabled_cops
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

    /// `Node::parent` for a node of this file's tree. See [`AstIndex::parent`].
    pub fn parent<'node>(&'node self, node: Node<'node>) -> Option<Node<'node>> {
        self.ast.parent(node)
    }

    pub fn root_node(&self) -> Node<'a> {
        self.ast.root
    }

    /// The file's node index, for the helpers that answer structural questions -- a parent above
    /// all -- outside a cop's own `check`.
    pub(in crate::rules) fn ast_index(&self) -> &'a AstIndex<'a> {
        self.ast
    }

    /// `Node::children` for a node of this file, answered from the index. See
    /// [`AstIndex::children_of`].
    pub(in crate::rules) fn children<'node>(
        &'node self,
        node: Node<'node>,
    ) -> Option<Children<'node>> {
        self.ast.children_of(node)
    }

    /// `Node::named_children` for a node of this file, answered from the index. See
    /// [`AstIndex::named_children_of`].
    pub(in crate::rules) fn named_children<'node>(
        &'node self,
        node: Node<'node>,
    ) -> Option<&'node [Node<'node>]> {
        self.ast.named_children_of(node)
    }

    /// Every named node of `node`'s subtree, `node` first. See [`AstIndex::named_descendants`].
    pub(in crate::rules) fn named_descendants<'node>(
        &'node self,
        node: Node<'node>,
    ) -> Option<&'node [Node<'node>]> {
        self.ast.named_descendants(node)
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
    ///
    /// A single kind is by far the common case and is handed straight through: the per-kind lists
    /// are already in source order, so there is nothing to merge and **the list is borrowed**.
    /// Copying it out cost an allocation and a memcpy of the whole list on every cop of every
    /// file, and the list is thousands of entries wide for a kind as ordinary as `call`.
    pub fn nodes_of_any(&self, kinds: &[&str]) -> impl Iterator<Item = Node<'a>> + '_ {
        let positions: std::borrow::Cow<'_, [u32]> = match kinds {
            [only] => std::borrow::Cow::Borrowed(self.ast.slice_of_kind(only)),
            _ => {
                // One allocation of the right size. `collect` from a `flat_map` cannot see the
                // total ahead of time and grows the vector as it goes.
                let total: usize = kinds
                    .iter()
                    .map(|kind| self.ast.slice_of_kind(kind).len())
                    .sum();
                let mut indices = Vec::with_capacity(total);
                for kind in kinds {
                    indices.extend_from_slice(self.ast.slice_of_kind(kind));
                }
                indices.sort_unstable();
                std::borrow::Cow::Owned(indices)
            }
        };
        (0..positions.len()).map(move |at| self.ast.named_node(positions[at]))
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

/// FNV-1a over the node kind names.
///
/// The keys are tree-sitter's kind strings -- short, ASCII, and looked up once per cop per file,
/// which is hundreds of thousands of times in a run. SipHash's resistance to collision attacks
/// buys nothing against a fixed set of names the grammar chose, and its setup costs more than the
/// hash of a ten-character string.
#[derive(Default)]
pub(crate) struct KindHasher(u64);

impl std::hash::Hasher for KindHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = if self.0 == 0 { OFFSET } else { self.0 };
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        self.0 = hash;
    }
}

type KindMap = HashMap<&'static str, Vec<u32>, std::hash::BuildHasherDefault<KindHasher>>;

/// The hash for a node id, which is the address tree-sitter stored the subtree at.
///
/// The keys are pointers into one arena: already well spread above the alignment, and never
/// chosen by anything but the parser. Multiplying by the 64-bit golden ratio moves the low bits
/// that alignment fixes into the top of the word, which is where the table reads from -- SipHash's
/// four rounds buy nothing here and are asked for on every node of every file.
#[derive(Default)]
pub(crate) struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }

    fn write_usize(&mut self, value: usize) {
        self.0 = (value as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

type IdMap = HashMap<usize, u32, std::hash::BuildHasherDefault<IdHasher>>;

/// The hasher every id-keyed table should be built with. See [`IdHasher`].
pub(crate) type IdHash = std::hash::BuildHasherDefault<IdHasher>;

/// A set of node ids.
pub(crate) type IdSet = std::collections::HashSet<usize, IdHash>;

/// A map from a node id.
pub(crate) type IdKeyed<V> = HashMap<usize, V, IdHash>;

pub(crate) struct AstIndex<'tree> {
    root: Node<'tree>,
    nodes: Vec<Node<'tree>>,
    named_nodes: Vec<Node<'tree>>,
    /// Positions in `named_nodes` grouped by node kind, each list in source order. Indices rather
    /// than nodes because a `Node` is eight times the size of the `u32` that finds it.
    by_kind: KindMap,
    protected_ranges: Vec<Range<usize>>,
    heredoc_ranges: Vec<Range<usize>>,
    comment_ranges: Vec<Range<usize>>,
    /// Where each node sits in `nodes`, keyed by the node's own id. Both structural answers below
    /// start here.
    positions: IdMap,
    /// Each node's parent, as its position in `nodes`. [`NO_PARENT`] stands for the root.
    ///
    /// `Node::parent` walks down from the root of the tree comparing byte ranges, which costs a
    /// pass over the children of every ancestor -- 43% of a run over RuboCop's own tree once the
    /// cheaper accessors were dealt with. The walk that builds this index already knows every
    /// node's parent, so recording it turns the question into one hash lookup.
    parent_of: Vec<u32>,
    /// Every node's named children, laid end to end, with `child_start[position]` naming where a
    /// node's run begins and `child_start[position + 1]` where it ends.
    ///
    /// `Node::named_children` opens a tree cursor and collects into a fresh `Vec` on every call.
    /// Both showed at the top of a sampling profile: the cursor iteration was the largest single
    /// cost of a run and the collect the largest allocation. The walk that fills `nodes` already
    /// visits every child, so the list is recorded rather than rebuilt.
    named_children: Vec<Node<'tree>>,
    child_start: Vec<u32>,
    /// Where each node sits in `named_nodes`, or [`NOT_NAMED`] for one that is not named.
    named_index: Vec<u32>,
    /// How many nodes each node's subtree holds, itself included. `nodes` is in pre-order, so a
    /// node's own children are found by stepping over one subtree at a time from the node after
    /// it -- which is what lets [`Self::children_of`] answer without a tree cursor.
    subtree_len: Vec<u32>,
    /// How many named nodes each node's subtree holds, itself included.
    ///
    /// `named_nodes` is in pre-order, so a node's descendants are the run that begins where the
    /// node itself sits and is this long -- which is what turns "every named node below this one"
    /// from a stack-driven walk into a slice.
    named_subtree: Vec<u32>,
}

/// One node's children, walked through the index's pre-order table. See
/// [`AstIndex::children_of`].
pub(in crate::rules) struct Children<'a> {
    nodes: &'a [Node<'a>],
    subtree_len: &'a [u32],
    next: usize,
    end: usize,
}

impl<'a> Iterator for Children<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Node<'a>> {
        if self.next >= self.end {
            return None;
        }
        let node = self.nodes[self.next];
        self.next += self.subtree_len[self.next] as usize;
        Some(node)
    }
}

/// The value [`AstIndex::parent_of`] carries for the root.
const NO_PARENT: u32 = u32::MAX;

/// The value [`AstIndex::named_index`] carries for a node that is not named.
const NOT_NAMED: u32 = u32::MAX;

impl<'tree> AstIndex<'tree> {
    pub fn new(root: Node<'tree>) -> Self {
        // The tree knows how many nodes it holds, so every per-node table is allocated once at the
        // right size. Growing them by doubling costs a dozen reallocations and copies per file,
        // and eight workers doing that at once is contention on the allocator rather than work.
        let count = root.descendant_count();
        let mut index = Self {
            root,
            nodes: Vec::with_capacity(count),
            named_nodes: Vec::with_capacity(count),
            by_kind: KindMap::default(),
            protected_ranges: Vec::new(),
            heredoc_ranges: Vec::new(),
            comment_ranges: Vec::new(),
            positions: IdMap::with_capacity_and_hasher(count, Default::default()),
            parent_of: Vec::with_capacity(count),
            named_children: Vec::new(),
            child_start: Vec::new(),
            named_index: Vec::with_capacity(count),
            subtree_len: Vec::new(),
            named_subtree: Vec::new(),
        };
        index.collect(root);
        index.index_children();
        index.index_subtrees();
        index.protected_ranges.sort_by_key(|range| range.start);
        merge_touching_ranges(&mut index.protected_ranges);
        index.heredoc_ranges.sort_by_key(|range| range.start);
        index
    }

    pub fn comment_ranges(&self) -> &[Range<usize>] {
        &self.comment_ranges
    }

    fn of_kind(&self, kind: &str) -> impl Iterator<Item = u32> + '_ {
        self.slice_of_kind(kind).iter().copied()
    }

    /// The positions for one kind, in source order.
    fn slice_of_kind(&self, kind: &str) -> &[u32] {
        self.by_kind.get(kind).map_or(&[][..], Vec::as_slice)
    }

    fn named_node(&self, index: u32) -> Node<'tree> {
        self.named_nodes[index as usize]
    }

    /// `Node::parent`, answered from [`Self::parents`].
    ///
    /// A node the index does not know -- one belonging to a tree parsed beside this one, which
    /// `Metrics`' recovered fragments are -- is asked of the parser itself, so the answer is the
    /// one `Node::parent` would have given whatever tree the node came from.
    fn parent<'node>(&'node self, node: Node<'node>) -> Option<Node<'node>> {
        match self.positions.get(&node.id()) {
            Some(&position) => match self.parent_of[position as usize] {
                NO_PARENT => None,
                index => Some(self.nodes[index as usize]),
            },
            None => node.parent(),
        }
    }

    /// The named nodes of one kind, in source order -- the same list [`RuleContext::nodes_of`]
    /// hands a cop, for the helpers that hold an index rather than a context.
    pub(in crate::rules) fn nodes_of_kind<'a>(
        &'a self,
        kind: &str,
    ) -> impl Iterator<Item = Node<'tree>> + 'a {
        self.of_kind(kind).map(|index| self.named_node(index))
    }

    /// Every child of `node`, named or not, in the order the parser wrote them.
    ///
    /// `Node::children` opens a tree cursor for each node it is asked about, and a sampling
    /// profile of a run over RuboCop's own tree put that cursor's iteration first among every
    /// cost. The pre-order table already holds the children: the first sits right after the node,
    /// and each next one a whole subtree further on.
    pub(in crate::rules) fn children_of<'node>(
        &'node self,
        node: Node<'node>,
    ) -> Option<Children<'node>> {
        let position = *self.positions.get(&node.id())? as usize;
        Some(Children {
            nodes: &self.nodes,
            subtree_len: &self.subtree_len,
            next: position + 1,
            end: position + self.subtree_len[position] as usize,
        })
    }

    /// Every named node of `node`'s subtree, `node` itself first and the rest in depth-first
    /// pre-order -- the order a stack-driven walk produces.
    ///
    /// `named_nodes` is filled in that order, so the answer is a run of it rather than a walk.
    pub(in crate::rules) fn named_descendants<'node>(
        &'node self,
        node: Node<'node>,
    ) -> Option<&'node [Node<'node>]> {
        let position = *self.positions.get(&node.id())? as usize;
        let start = match self.named_index[position] {
            NOT_NAMED => return None,
            index => index as usize,
        };
        Some(&self.named_nodes[start..start + self.named_subtree[position] as usize])
    }

    /// The named children of a node of this file's tree, as they were recorded when the index was
    /// built. A node the index does not know is walked with a cursor, which is what
    /// `Node::named_children` would have done for every node.
    pub(in crate::rules) fn named_children_of<'node>(
        &'node self,
        node: Node<'node>,
    ) -> Option<&'node [Node<'node>]> {
        let position = *self.positions.get(&node.id())? as usize;
        let start = self.child_start[position] as usize;
        let end = self.child_start[position + 1] as usize;
        Some(&self.named_children[start..end])
    }

    /// [`Self::parent`] for a node of the tree this index was built from, answered without tying
    /// the result to the borrow of the index.
    ///
    /// A helper that hands the parent back to its caller, and the variable force, which holds
    /// nodes for as long as the file is inspected, both need the tree's own lifetime rather than
    /// the shorter one `parent` gives.
    /// The tree this index was built from.
    pub(in crate::rules) fn root_node(&self) -> Node<'tree> {
        self.root
    }

    pub(in crate::rules) fn parent_in_tree(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        match self.positions.get(&node.id()) {
            Some(&position) => match self.parent_of[position as usize] {
                NO_PARENT => None,
                index => Some(self.nodes[index as usize]),
            },
            None => node.parent(),
        }
    }

    /// Groups the named children of every node into one run each, from the parents the walk
    /// recorded. A counting pass and a placing pass, so no node's list is grown as it fills.
    fn index_children(&mut self) {
        let count = self.nodes.len();
        // One slot past the end, so a node's run is `child_start[i]..child_start[i + 1]` without
        // a bounds check on the last node.
        self.child_start = vec![0u32; count + 1];
        for position in 0..count {
            if self.named_index[position] == NOT_NAMED {
                continue;
            }
            let parent = self.parent_of[position];
            if parent == NO_PARENT {
                continue;
            }
            self.child_start[parent as usize + 1] += 1;
        }
        for index in 1..=count {
            self.child_start[index] += self.child_start[index - 1];
        }
        // `child_start` now holds each node's end; filling backwards from it leaves it holding
        // the start again, which is the usual way to build a compressed adjacency list.
        let total = self.child_start[count] as usize;
        self.named_children = vec![self.root; total];
        let mut cursor = self.child_start.clone();
        for position in 0..count {
            if self.named_index[position] == NOT_NAMED {
                continue;
            }
            let parent = self.parent_of[position];
            if parent == NO_PARENT {
                continue;
            }
            let slot = &mut cursor[parent as usize];
            self.named_children[*slot as usize] = self.nodes[position];
            *slot += 1;
        }
    }

    /// Counts the named nodes of every subtree. A node always sits before its children in
    /// `nodes`, so one pass from the end folds each count into its parent's.
    fn index_subtrees(&mut self) {
        self.named_subtree = self
            .named_index
            .iter()
            .map(|index| u32::from(*index != NOT_NAMED))
            .collect();
        self.subtree_len = vec![1u32; self.nodes.len()];
        for position in (1..self.nodes.len()).rev() {
            let parent = self.parent_of[position];
            if parent != NO_PARENT {
                self.named_subtree[parent as usize] += self.named_subtree[position];
                self.subtree_len[parent as usize] += self.subtree_len[position];
            }
        }
    }

    /// Visits every node in depth-first pre-order. Iterative on purpose: rayon
    /// worker stacks are far smaller than the main thread's, and a recursive
    /// walk aborts the whole process on deeply nested input.
    fn collect(&mut self, root: Node<'tree>) {
        let mut cursor = root.walk();
        let mut ancestors: Vec<u32> = Vec::new();
        loop {
            let here = self.nodes.len() as u32;
            self.visit(
                cursor.node(),
                ancestors.last().copied().unwrap_or(NO_PARENT),
            );
            if cursor.goto_first_child() {
                ancestors.push(here);
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return;
                }
                ancestors.pop();
            }
        }
    }

    fn visit(&mut self, node: Node<'tree>, parent: u32) {
        // Read once. Each call goes through the C API for the node's symbol, and this used to ask
        // four times for every node of every file.
        let kind = node.kind_str();
        let named = node.is_named();
        self.positions.insert(node.id(), self.nodes.len() as u32);
        self.parent_of.push(parent);
        self.named_index.push(match named {
            true => self.named_nodes.len() as u32,
            false => NOT_NAMED,
        });
        self.nodes.push(node);
        if named {
            // A file with more than u32::MAX named nodes would need tens of gigabytes of source;
            // the cast cannot lose information for anything a parser will accept.
            let index = self.named_nodes.len() as u32;
            self.named_nodes.push(node);
            self.by_kind.entry(kind).or_default().push(index);
        }
        if PROTECTED_LITERAL_KINDS.contains(&kind) {
            self.protected_ranges.push(node.byte_range());
        }
        if kind == "heredoc_body" {
            self.heredoc_ranges.push(node.byte_range());
        }
        if kind == "comment" && !inside_literal_text(node) {
            self.comment_ranges.push(node.byte_range());
        }
    }
}

/// Whether the grammar found the comment inside the text of a literal, where Ruby has no comments
/// at all.
///
/// A `#` that opens no interpolation is ordinary text in a heredoc, but the scanner takes `##{x}` for
/// a comment running to the end of the line -- and upstream's parser reads the same bytes as part of
/// the string. A comment written *inside* an interpolation is a real one, so the search stops there.
fn inside_literal_text(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind_str() {
            "interpolation" => return false,
            kind if LITERAL_TEXT_KINDS.contains(&kind) => return true,
            _ => current = parent,
        }
    }
    false
}

/// The node kinds whose contents are text rather than code.
const LITERAL_TEXT_KINDS: &[&str] = &[
    "heredoc_body",
    "string",
    "regex",
    "subshell",
    "bare_string",
    "string_array",
    "symbol_array",
    "delimited_symbol",
];

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

    use super::{AstIndex, merge_touching_ranges, rule_names, rules};
    use crate::config::Config;
    use crate::rules::node_ext::NodeExt;
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

    /// The index answers a parent and a child list from its own arrays rather than from the
    /// parser, so the two have to agree for every node of a real file -- a cop reaching for either
    /// would otherwise see a tree the grammar never built.
    #[test]
    fn the_index_answers_what_the_parser_answers() {
        let source = "# frozen_string_literal: true\n                      class Foo < Bar\n                        def baz(a = 1, *rest, key:, &block)\n                          @x ||= a.map { |v| \"#{v}-#{rest.first}\" }\n                          text = <<~DOC\n    hello #{key}\n  DOC\n                          [1, 2].each_with_object({}) { |n, memo| memo[n] = n }\n                        rescue StandardError => error\n                          raise error\n                        ensure\n                          puts text\n                        end\n                      end\n";
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .expect("the Ruby grammar loads");
        let tree = parser.parse(source, None).expect("the source parses");
        let index = AstIndex::new(tree.root_node());

        let mut seen = 0;
        for node in &index.nodes {
            let mut cursor = node.walk();
            let expected: Vec<tree_sitter::Node<'_>> = node.named_children(&mut cursor).collect();
            let recorded = index
                .named_children_of(*node)
                .expect("every node of this tree is indexed");
            let mut all = node.walk();
            let all_expected: Vec<tree_sitter::Node<'_>> = node.children(&mut all).collect();
            let all_recorded: Vec<tree_sitter::Node<'_>> = index
                .children_of(*node)
                .expect("every node of this tree is indexed")
                .collect();
            assert_eq!(
                all_recorded,
                all_expected,
                "{} reported different children",
                node.kind_str()
            );
            assert_eq!(
                recorded,
                expected,
                "{} reported different named children",
                node.kind_str()
            );
            assert_eq!(
                index.parent_in_tree(*node).map(|found| found.id()),
                node.parent().map(|found| found.id()),
                "{} reported a different parent",
                node.kind_str()
            );
            seen += 1;
        }
        assert!(
            seen > 100,
            "the sample file should exercise more than {seen} nodes"
        );
    }

    /// A node from another tree -- the extra parses `Metrics` makes to recover what the grammar
    /// swallowed -- is not in the index, and both accessors have to fall back to the parser rather
    /// than answer for a node they never saw.
    #[test]
    fn a_node_from_another_tree_falls_back_to_the_parser() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .expect("the Ruby grammar loads");
        let indexed = parser.parse("foo(1)\n", None).expect("the source parses");
        let index = AstIndex::new(indexed.root_node());

        let other = parser
            .parse("bar(2, 3)\n", None)
            .expect("the source parses");
        let call = other
            .root_node()
            .named_child(0)
            .expect("the program has a statement");
        assert!(index.named_children_of(call).is_none());
        let mut cursor = call.walk();
        let expected: Vec<tree_sitter::Node<'_>> = call.named_children(&mut cursor).collect();
        assert_eq!(
            crate::rules::send_node::named_children_in(call, &index).into_owned(),
            expected
        );
        assert_eq!(
            index.parent_in_tree(call).map(|found| found.id()),
            call.parent().map(|found| found.id())
        );
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
