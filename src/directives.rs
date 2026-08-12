use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

use crate::config::Config;
use crate::cop_name::department;
use crate::diagnostic::Offense;
use crate::engine::{Selection, is_mandatory_cop};
use crate::source::SourceFile;

#[derive(Clone, Debug, Default)]
struct Snapshot {
    all: bool,
    all_reason: Option<String>,
    cops: HashMap<String, Option<String>>,
}

#[derive(Debug, Default)]
pub struct DirectiveState {
    line_states: Vec<Snapshot>,
}

impl DirectiveState {
    pub fn parse(source: &SourceFile, comment_ranges: &[Range<usize>]) -> Self {
        let mut current = Snapshot::default();
        let mut stack = Vec::new();
        let mut line_states = Vec::with_capacity(source.line_count());
        // Where the first comment written on each line begins, as an offset into that line. Only
        // the parse knows this: a `#` can open a comment, an interpolation or nothing at all
        // depending on what it stands inside of.
        let mut comment_starts: HashMap<usize, usize> = HashMap::new();
        for range in comment_ranges {
            let (line_number, _) = source.line_column(range.start);
            let column = range.start - source.line_start(line_number);
            comment_starts
                .entry(line_number)
                .and_modify(|first| *first = (*first).min(column))
                .or_insert(column);
        }

        for line_number in 1..=source.line_count() {
            let line = source.line(line_number);
            let before = current.clone();
            let directive = comment_starts
                .get(&line_number)
                .and_then(|&start| parse_directive(line, start));

            if let Some(directive) = directive {
                if directive.inline && matches!(directive.action, Action::Disable) {
                    let mut line_only = before;
                    apply_disable(&mut line_only, &directive.cops, directive.reason);
                    line_states.push(line_only);
                    continue;
                }

                match directive.action {
                    Action::Disable => {
                        apply_disable(&mut current, &directive.cops, directive.reason);
                    }
                    Action::Enable => apply_enable(&mut current, &directive.cops),
                    Action::Push => {
                        stack.push(current.clone());
                        for (enable, cop) in directive.push_operations {
                            if enable {
                                apply_enable(&mut current, &[cop]);
                            } else {
                                apply_disable(&mut current, &[cop], directive.reason.clone());
                            }
                        }
                    }
                    Action::Pop => {
                        if let Some(snapshot) = stack.pop() {
                            current = snapshot;
                        }
                    }
                }
            }

            line_states.push(current.clone());
        }

        Self { line_states }
    }

    /// The inclusive line ranges over which `cop` is disabled, which is what
    /// `ProcessedSource#disabled_line_ranges` hands the cops that reason about their own
    /// directives. A range left open at the end of the file stops at its last line.
    pub fn disabled_line_ranges(&self, cop: &str, source: &SourceFile) -> Vec<Range<usize>> {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for line in 1..=source.line_count() {
            let Some(state) = self.line_states.get(line - 1) else {
                break;
            };
            if !(state.all
                || state.cops.contains_key(cop)
                || state.cops.contains_key(department(cop)))
            {
                continue;
            }
            match ranges.last_mut() {
                Some(last) if last.end + 1 == line => last.end = line,
                _ => ranges.push(line..line),
            }
        }
        ranges
    }

    pub fn suppression(&self, offense: &Offense, source: &SourceFile) -> Option<Option<String>> {
        // `DirectiveComment` drops `Lint/Syntax` from the cop list of every directive -- named,
        // by department and by `all` (`#parsed_cop_names`, `#exclude_lint_department_cops`) -- so
        // a file cannot turn off the report that it does not parse.
        if is_mandatory_cop(offense.cop_name) {
            return None;
        }
        let (line, _) = source.line_column(offense.start);
        let state = self.line_states.get(line.saturating_sub(1))?;
        if let Some(reason) = state.cops.get(offense.cop_name) {
            return Some(reason.clone());
        }
        // `exclude_lint_department_cops` takes the cop that reads directives out of `all` and out
        // of the `Lint` department, so only a directive that spells its name out switches it off:
        // "this cop is not disabled when disabling all cops".
        if offense.cop_name == UNDISABLEABLE_COPS[0] {
            return None;
        }
        if let Some(reason) = state.cops.get(department(offense.cop_name)) {
            return Some(reason.clone());
        }
        state.all.then(|| state.all_reason.clone())
    }
}

fn apply_disable(state: &mut Snapshot, cops: &[String], reason: Option<String>) {
    if cops.iter().any(|cop| cop == "all") {
        state.all = true;
        state.all_reason = reason;
    } else {
        state
            .cops
            .extend(cops.iter().cloned().map(|cop| (cop, reason.clone())));
    }
}

fn apply_enable(state: &mut Snapshot, cops: &[String]) {
    if cops.iter().any(|cop| cop == "all") {
        state.all = false;
        state.all_reason = None;
        state.cops.clear();
    } else {
        for cop in cops {
            state.cops.remove(cop);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Disable,
    Enable,
    Push,
    Pop,
}

#[derive(Debug)]
struct Directive {
    action: Action,
    cops: Vec<String>,
    push_operations: Vec<(bool, String)>,
    reason: Option<String>,
    inline: bool,
}

fn parse_directive(line: &str, comment_start: usize) -> Option<Directive> {
    let comment = line.get(comment_start..)?;
    let marker_end = marker_end(comment)?;
    let command = comment[marker_end..].trim_start();
    let mode_end = command.find(char::is_whitespace).unwrap_or(command.len());
    let mode = &command[..mode_end];
    let remainder = command[mode_end..].trim();
    let (arguments, reason) =
        remainder
            .split_once("--")
            .map_or((remainder, None), |(arguments, reason)| {
                let reason = reason.trim();
                (
                    arguments.trim(),
                    (!reason.is_empty()).then(|| reason.to_owned()),
                )
            });
    let action = match mode {
        "disable" | "todo" => Action::Disable,
        "enable" => Action::Enable,
        "push" => Action::Push,
        "pop" => Action::Pop,
        _ => return None,
    };
    let inline = !line[..comment_start].trim().is_empty();

    if matches!(action, Action::Push) {
        let push_operations = arguments
            .split_whitespace()
            .filter_map(|specification| {
                let (operator, name) = specification.split_at_checked(1)?;
                matches!(operator, "+" | "-").then(|| (operator == "+", name.to_owned()))
            })
            .collect();
        return Some(Directive {
            action,
            cops: Vec::new(),
            push_operations,
            reason,
            inline,
        });
    }
    if matches!(action, Action::Pop) {
        return Some(Directive {
            action,
            cops: Vec::new(),
            push_operations: Vec::new(),
            reason,
            inline,
        });
    }
    let cops = cop_list(arguments);
    if cops.is_empty() {
        return None;
    }
    Some(Directive {
        action,
        cops,
        push_operations: Vec::new(),
        reason,
        inline,
    })
}

/// The cop names a `disable`/`enable`/`todo` directive lists.
///
/// `DirectiveComment::DIRECTIVE_COMMENT_REGEXP` matches `(all|COP(?:\s*,\s*COP)*)` and is not
/// anchored, so it stops at the first word that is not a cop name and leaves the rest of the
/// comment as prose: `# rubocop:disable Lint/UselessAssignment kept for the closure` disables the
/// cop it names. Reading the whole remainder as one name would silently ignore such a directive.
fn cop_list(arguments: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = arguments.trim_start();
    loop {
        let Some(length) = cop_name_length(rest) else {
            return names;
        };
        names.push(rest[..length].to_owned());
        let after = rest[length..].trim_start();
        match after.strip_prefix(',') {
            Some(next) => rest = next.trim_start(),
            None => return names,
        }
    }
}

/// The length of the cop name `text` starts with, following `COP_NAME_PATTERN`: one or more
/// `[A-Za-z]\w+` segments separated by slashes.
fn cop_name_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    loop {
        let start = index;
        if !bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            index += 1;
        }
        // The pattern is `[A-Za-z]\w+`, so a one-character segment does not match.
        if index - start < 2 {
            return None;
        }
        if bytes.get(index) != Some(&b'/') {
            return Some(index);
        }
        index += 1;
    }
}

fn marker_end(comment: &str) -> Option<usize> {
    let bytes = comment.as_bytes();
    let mut index = 0;
    if bytes.get(index) != Some(&b'#') {
        return None;
    }
    index += 1;
    skip_ascii_whitespace(bytes, &mut index);
    if !comment[index..].starts_with("rubocop") {
        return None;
    }
    index += "rubocop".len();
    skip_ascii_whitespace(bytes, &mut index);
    if bytes.get(index) != Some(&b':') {
        return None;
    }
    Some(index + 1)
}

fn skip_ascii_whitespace(bytes: &[u8], index: &mut usize) {
    while bytes
        .get(*index)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        *index += 1;
    }
}

/// The two cops a directive can never name.
///
/// `DirectiveComment#exclude_lint_department_cops` drops them from `all` and from the `Lint`
/// department, so a file can neither turn off the parse report nor the cop that reads its own
/// directives by naming their department.
const UNDISABLEABLE_COPS: [&str; 2] = ["Lint/RedundantCopDisableDirective", "Lint/Syntax"];

/// The cop `CommentConfig` refuses to record a directive for once it is explicitly enabled, so
/// that a file cannot switch off the cop that objects to its directives.
const DISABLE_COPS_DIRECTIVE_COP: &str = "Style/DisableCopsWithinSourceCodeDirective";

/// The line a range starts at when the configuration, rather than a comment, turned the cop off.
///
/// `CommentConfig::CONFIG_DISABLED_LINE_RANGE_MIN` is `-Float::INFINITY`, which is what
/// `Lint/RedundantCopDisableDirective` tests for to leave such a range alone.
pub const CONFIG_DISABLED_LINE: i64 = i64::MIN;

/// The line a range ends at when nothing re-enables the cop before the end of the file, which
/// RuboCop writes as `Float::INFINITY`.
pub const END_OF_FILE_LINE: i64 = i64::MAX;

/// One inclusive span of lines over which a cop is switched off, as `CommentConfig` records it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineRange {
    pub begin: i64,
    pub end: i64,
}

impl LineRange {
    pub fn covers(self, line: i64) -> bool {
        self.begin <= line && line <= self.end
    }

    /// `Range#cover?` over another range, which is how `ignore_offense?` asks whether a directive
    /// sits wholly inside the span where this very cop is switched off.
    pub fn contains(self, other: Self) -> bool {
        self.covers(other.begin) && self.covers(other.end)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectiveMode {
    Disable,
    Todo,
    Enable,
    Push,
    Pop,
}

impl DirectiveMode {
    fn parse(mode: &str) -> Option<Self> {
        match mode {
            "disable" => Some(Self::Disable),
            "todo" => Some(Self::Todo),
            "enable" => Some(Self::Enable),
            "push" => Some(Self::Push),
            "pop" => Some(Self::Pop),
            _ => None,
        }
    }
}

/// `DirectiveComment::DIRECTIVE_COMMENT_REGEXP`.
///
/// RuboCop builds it by writing every literal space in the pattern and then replacing each one
/// with `\s*`, so `# rubocop : disable` tolerates any spacing around the marker while the gaps
/// written as `\s+` stay mandatory. Ruby's `\s` and `\w` are ASCII-only for a UTF-8 source, which
/// the character classes here spell out because Rust's are not.
fn directive_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        const SPACE: &str = r"[ \t\r\n\x0b\x0c]";
        let name = r"(?:[A-Za-z][0-9A-Za-z_]+/)*[A-Za-z][0-9A-Za-z_]+";
        let names = format!(r"(?:{name}{SPACE}*,{SPACE}*)*{name}");
        let push_pop = format!(r"(?:[+\-]{name}(?:{SPACE}+[+\-]{name})*)");
        let header =
            format!(r"#{SPACE}*rubocop{SPACE}*:{SPACE}*((?:disable|enable|todo|push|pop))\b");
        let arguments = format!(r"(?:{SPACE}+(all|{names})|{SPACE}+({push_pop}))?");
        Regex::new(&format!("{header}{arguments}")).expect("the directive pattern is a literal")
    })
}

/// A `# rubocop:` comment, parsed the way `RuboCop::DirectiveComment` parses one.
#[derive(Clone, Debug)]
pub struct DirectiveComment {
    /// The span the directive pattern matched, which is the range the cop reports.
    pub range: Range<usize>,
    pub line: usize,
    pub mode: DirectiveMode,
    /// The `all`, cop list or push argument capture, verbatim.
    cops: Option<String>,
    /// `DirectiveComment#single_line?`: the directive does not open the comment, so it only
    /// applies to its own line.
    single_line: bool,
}

impl DirectiveComment {
    fn parse(source: &SourceFile, comment: Range<usize>, line: usize) -> Option<Self> {
        let text = source.slice(comment.clone());
        let captures = directive_regex().captures(text)?;
        let whole = captures.get(0).expect("group zero always participates");
        // `DirectiveComment#initialize` throws the match away when everything before it is a `#`
        // and whitespace, which is a directive that has itself been commented out.
        let prefix = &text[..whole.start()];
        if prefix.starts_with('#')
            && prefix[1..]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
        {
            return None;
        }
        Some(Self {
            range: comment.start + whole.start()..comment.start + whole.end(),
            line,
            mode: DirectiveMode::parse(captures.get(1)?.as_str())?,
            cops: captures
                .get(2)
                .or_else(|| captures.get(3))
                .map(|capture| capture.as_str().to_owned()),
            single_line: whole.start() != 0,
        })
    }

    pub fn disabled(&self) -> bool {
        matches!(self.mode, DirectiveMode::Disable | DirectiveMode::Todo)
    }

    pub fn all_cops(&self) -> bool {
        self.cops.as_deref() == Some("all")
    }

    /// `DirectiveComment#disabled_all?`.
    pub fn disabled_all(&self) -> bool {
        self.disabled() && self.all_cops()
    }

    /// The cop names exactly as the comment spells them, departments left unexpanded.
    ///
    /// `raw_cop_names` splits on `/,\s*/`, so the whitespace a name is followed by stays attached
    /// to it -- which is what makes `# rubocop:disable Foo , Bar` count `Foo ` as its own entry.
    pub fn raw_cop_names(&self) -> Vec<&str> {
        let Some(cops) = &self.cops else {
            return Vec::new();
        };
        cops.split(',')
            .enumerate()
            .map(|(index, name)| match index {
                0 => name,
                _ => name.trim_start_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c']),
            })
            .collect()
    }

    /// `DirectiveComment#directive_count`, which decides whether a redundant cop takes the whole
    /// comment with it or only its own name.
    pub fn directive_count(&self) -> usize {
        self.raw_cop_names().len()
    }

    /// Whether the comment names a department that `cop` belongs to.
    pub fn in_directive_department(&self, cop: &str, registry: &CopRegistry) -> bool {
        self.raw_cop_names()
            .into_iter()
            .filter(|name| registry.is_department(name))
            .any(|department| cop.starts_with(department))
    }

    /// `DirectiveComment#overridden_by_department?`: the comment names both the department and the
    /// cop, so the cop's own entry is what speaks for it.
    pub fn overridden_by_department(&self, cop: &str, registry: &CopRegistry) -> bool {
        self.in_directive_department(cop, registry) && self.raw_cop_names().contains(&cop)
    }

    /// The cops the comment switches, with departments and `all` expanded.
    fn cop_names(&self, registry: &CopRegistry) -> Vec<String> {
        if self.all_cops() {
            return registry.all_directive_cop_names();
        }
        let mut names = Vec::new();
        for raw in self.raw_cop_names() {
            if registry.is_department(raw) {
                names.extend(registry.names_for_department(raw));
            } else if !UNDISABLEABLE_COPS[1].eq(raw) {
                names.push(raw.to_owned());
            }
        }
        names
    }

    /// `DirectiveComment#push_args`: the `+`/`-` operations a push directive carries, in the order
    /// each operator first appears.
    fn push_args(&self) -> Vec<(char, Vec<&str>)> {
        let Some(cops) = &self.cops else {
            return Vec::new();
        };
        if !matches!(self.mode, DirectiveMode::Push) {
            return Vec::new();
        }
        let mut args: Vec<(char, Vec<&str>)> = Vec::new();
        for specification in cops.split_whitespace() {
            let mut characters = specification.chars();
            let Some(operator) = characters.next() else {
                continue;
            };
            let name = characters.as_str();
            match args.iter_mut().find(|(existing, _)| *existing == operator) {
                Some((_, names)) => names.push(name),
                None => args.push((operator, vec![name])),
            }
        }
        args
    }
}

/// What the run knows about the cops that exist, which is what `CommentConfig` and
/// `Lint/RedundantCopDisableDirective` consult to expand a department, qualify a bare name and
/// tell a real cop from a typo.
///
/// RuboCop reads two different registries here. Department expansion and the "unknown cop" check
/// go to `Cop::Registry.global`, so `--except` cannot change what a directive means or how an
/// offense reads; the set of configuration-disabled cops comes from the run's own mobilized
/// registry, so a cop the run was told to skip is not one whose directives are pre-empted.
pub struct CopRegistry {
    names: Vec<String>,
    known: HashSet<String>,
    departments: HashSet<String>,
    by_department: HashMap<String, Vec<String>>,
    /// Qualified names grouped by their `Badge#cop_name`, for `Registry.qualified_cop_name`.
    by_cop_name: HashMap<String, Vec<String>>,
    /// Every cop the configuration turns off, which is what `expected_final_disable?` asks about.
    config_disabled: HashSet<String>,
    /// `Registry#disabled_names`: the same, restricted to the cops the run mobilized.
    disabled: Vec<String>,
    /// Whether `Style/DisableCopsWithinSourceCodeDirective` is explicitly on, which stops a
    /// directive from being recorded against it at all.
    prevents_directive_disabling: bool,
}

impl CopRegistry {
    pub fn new(config: &Config, selection: &Selection) -> Self {
        let mut names: Vec<String> = config.known_cop_names().map(ToOwned::to_owned).collect();
        names.sort_unstable();
        let mut departments = HashSet::new();
        let mut by_department: HashMap<String, Vec<String>> = HashMap::new();
        let mut by_cop_name: HashMap<String, Vec<String>> = HashMap::new();
        for name in &names {
            let department = department(name).to_owned();
            departments.insert(department.clone());
            by_department
                .entry(department)
                .or_default()
                .push(name.clone());
            let cop_name = name.rsplit('/').next().unwrap_or(name).to_owned();
            by_cop_name.entry(cop_name).or_default().push(name.clone());
        }
        // `Registry#enabled_cop_name?`, which reads the configuration alone.
        let config_disabled: HashSet<String> = names
            .iter()
            .filter(|name| {
                let enabled = config.rule_enabled_with_pending(
                    name,
                    selection.enable_pending,
                    selection.disable_pending,
                ) && (!selection.safe_only || config.rule_safe(name));
                !enabled
            })
            .cloned()
            .collect();
        // `Registry#disabled_names` runs over the mobilized registry, which `--except` has already
        // been taken out of. `--only` never reaches here: it turns the whole cop off.
        let disabled = names
            .iter()
            .filter(|name| {
                config_disabled.contains(name.as_str())
                    && !selection
                        .except
                        .iter()
                        .any(|except| crate::cop_name::selector_matches(except, name))
            })
            .cloned()
            .collect();
        Self {
            known: names.iter().cloned().collect(),
            names,
            departments,
            by_department,
            by_cop_name,
            config_disabled,
            disabled,
            prevents_directive_disabling: config
                .cop_value::<bool>(DISABLE_COPS_DIRECTIVE_COP, "Enabled")
                == Some(true),
        }
    }

    pub fn is_department(&self, name: &str) -> bool {
        self.departments.contains(name)
    }

    pub fn knows(&self, name: &str) -> bool {
        self.known.contains(name)
    }

    /// Whether the configuration turns the cop off, independently of what the run selected.
    pub fn config_disabled(&self, name: &str) -> bool {
        self.config_disabled.contains(name)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    fn all_directive_cop_names(&self) -> Vec<String> {
        self.names
            .iter()
            .filter(|name| !UNDISABLEABLE_COPS.contains(&name.as_str()))
            .cloned()
            .collect()
    }

    /// Every cop in `department`, minus the two a `Lint` directive may not reach.
    fn names_for_department(&self, department: &str) -> Vec<String> {
        let names = self
            .by_department
            .get(department)
            .cloned()
            .unwrap_or_default();
        match department {
            "Lint" => names
                .into_iter()
                .filter(|name| !UNDISABLEABLE_COPS.contains(&name.as_str()))
                .collect(),
            _ => names,
        }
    }

    /// Every cop in `department`, whatever its name, which is what a push directive expands to.
    fn push_names_for_department(&self, department: &str) -> Vec<String> {
        self.by_department
            .get(department)
            .cloned()
            .unwrap_or_default()
    }

    /// `Registry.qualified_cop_name`: a name written without its department, or under the wrong
    /// one, resolves to the cop that carries it when exactly one cop does.
    fn qualified_cop_name(&self, name: &str) -> String {
        if self.known.contains(name) {
            return name.to_owned();
        }
        let cop_name = name.rsplit('/').next().unwrap_or(name);
        match self.by_cop_name.get(cop_name) {
            Some(candidates) if candidates.len() == 1 => candidates[0].clone(),
            _ => name.to_owned(),
        }
    }
}

/// One cop's disabled spans as `CommentConfig` builds them up: the ranges already closed, and the
/// line an open one started at.
#[derive(Clone, Debug, Default)]
struct CopAnalysis {
    ranges: Vec<LineRange>,
    start: Option<i64>,
}

/// `RuboCop::CommentConfig`: which cops each line of a file has switched off, and by which comment.
pub struct CommentConfig {
    ranges: BTreeMap<String, Vec<LineRange>>,
    /// Every comment in the file, in source order.
    comments: Vec<CommentEntry>,
    /// The comment `ProcessedSource#comment_at_line` answers with, by line.
    by_line: HashMap<usize, usize>,
}

pub struct CommentEntry {
    range: Range<usize>,
    line: usize,
    comment_only_line: bool,
    directive: Option<DirectiveComment>,
}

impl CommentConfig {
    /// `CommentConfig#analyze`.
    pub fn analyze(
        source: &SourceFile,
        comment_ranges: &[Range<usize>],
        registry: &CopRegistry,
    ) -> Self {
        let mut comments = Vec::with_capacity(comment_ranges.len());
        let mut by_line = HashMap::with_capacity(comment_ranges.len());
        for range in comment_ranges {
            let (line, _) = source.line_column(range.start);
            by_line.insert(line, comments.len());
            comments.push(CommentEntry {
                comment_only_line: is_comment_only_line(source, comment_ranges, line),
                directive: DirectiveComment::parse(source, range.clone(), line),
                range: range.clone(),
                line,
            });
        }
        let mut config = Self {
            ranges: BTreeMap::new(),
            comments,
            by_line,
        };
        // `CommentConfig#initialize` gives up before parsing anything when the word cannot occur,
        // which is also what keeps `disabled_line_ranges` empty for most files.
        if source.text().contains("rubocop") {
            config.ranges = config.build_ranges(registry);
        }
        config
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn disabled_line_ranges(&self) -> &BTreeMap<String, Vec<LineRange>> {
        &self.ranges
    }

    /// `ProcessedSource#comment_at_line`.
    pub fn comment_at_line(&self, line: i64) -> Option<&CommentEntry> {
        usize::try_from(line)
            .ok()
            .and_then(|line| self.by_line.get(&line))
            .map(|index| &self.comments[*index])
    }

    /// The comment that starts at `offset`, which is the identity a comment is grouped under while
    /// the redundant cops found for it are collected.
    pub fn comment_at_offset(&self, offset: usize) -> Option<&CommentEntry> {
        self.comments
            .iter()
            .find(|comment| comment.range.start == offset)
    }

    fn build_ranges(&self, registry: &CopRegistry) -> BTreeMap<String, Vec<LineRange>> {
        let directives: Vec<(&DirectiveComment, bool)> = self
            .comments
            .iter()
            .filter_map(|comment| Some((comment.directive.as_ref()?, comment.comment_only_line)))
            .collect();
        let mut analyses: BTreeMap<String, CopAnalysis> = BTreeMap::new();
        for name in self.injected_cops(&directives, registry) {
            // `inject_disabled_cops_directives` feeds a synthetic block-form disable whose line is
            // `-Float::INFINITY`, which leaves the cop with an open range starting there.
            analyses.insert(
                name,
                CopAnalysis {
                    ranges: Vec::new(),
                    start: Some(CONFIG_DISABLED_LINE),
                },
            );
        }

        let mut stack: Vec<BTreeMap<String, CopAnalysis>> = Vec::new();
        for (directive, comment_only_line) in directives {
            let line = directive.line as i64;
            match directive.mode {
                DirectiveMode::Push => {
                    stack.push(analyses.clone());
                    for (operator, names) in directive.push_args() {
                        for name in names {
                            let expanded = if registry.is_department(name) {
                                registry.push_names_for_department(name)
                            } else {
                                vec![name.to_owned()]
                            };
                            for cop in expanded {
                                apply_push_operation(
                                    &mut analyses,
                                    operator,
                                    registry.qualified_cop_name(&cop),
                                    line,
                                );
                            }
                        }
                    }
                }
                DirectiveMode::Pop => {
                    if let Some(restore) = stack.pop() {
                        pop_state(&mut analyses, &restore, line);
                    }
                }
                _ => {
                    for cop in directive.cop_names(registry) {
                        let name = registry.qualified_cop_name(cop.trim());
                        let analysis = analyses.entry(name).or_default();
                        *analysis = analyze_cop(analysis, directive, comment_only_line);
                    }
                }
            }
        }

        analyses
            .into_iter()
            .filter(|(cop, _)| {
                !(registry.prevents_directive_disabling && cop == DISABLE_COPS_DIRECTIVE_COP)
            })
            .map(|(cop, analysis)| {
                let mut ranges = analysis.ranges;
                if let Some(start) = analysis.start {
                    ranges.push(LineRange {
                        begin: start,
                        end: END_OF_FILE_LINE,
                    });
                }
                (cop, ranges)
            })
            .collect()
    }

    /// The configuration-disabled cops worth seeding an open range for.
    ///
    /// RuboCop seeds every one of them, which for a default configuration means a couple of
    /// hundred entries in every file that mentions `rubocop`. A seed only ever changes what the
    /// analysis says about a cop some directive also touches, so the rest are left out; the one
    /// case where a directive reaches a cop it does not name is `push`/`pop`, whose restore point
    /// carries every key it saw.
    fn injected_cops(
        &self,
        directives: &[(&DirectiveComment, bool)],
        registry: &CopRegistry,
    ) -> Vec<String> {
        if registry.disabled.is_empty() {
            return Vec::new();
        }
        if directives.iter().any(|(directive, _)| {
            matches!(directive.mode, DirectiveMode::Push | DirectiveMode::Pop)
        }) {
            return registry.disabled.clone();
        }
        let touched: HashSet<String> = directives
            .iter()
            .flat_map(|(directive, _)| directive.cop_names(registry))
            .map(|cop| registry.qualified_cop_name(cop.trim()))
            .collect();
        registry
            .disabled
            .iter()
            .filter(|name| touched.contains(name.as_str()))
            .cloned()
            .collect()
    }
}

impl CommentEntry {
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn comment_only_line(&self) -> bool {
        self.comment_only_line
    }

    pub fn directive(&self) -> Option<&DirectiveComment> {
        self.directive.as_ref()
    }
}

/// `NameSimilarity.find_similar_name`, which is `DidYouMean::SpellChecker` over the cop registry.
///
/// A cop name a directive gets wrong is worth a suggestion rather than a bare "unknown cop", and
/// the suggestion is part of the message, so the ranking has to be the one Ruby's `did_you_mean`
/// computes rather than a nearest-neighbour of our own choosing.
pub fn find_similar_name<'a>(target: &str, dictionary: &'a [String]) -> Option<&'a str> {
    let normalized_target = normalize_for_similarity(target);
    let threshold = if normalized_target.chars().count() > 3 {
        0.834
    } else {
        0.77
    };
    let mut words: Vec<&'a str> = dictionary
        .iter()
        .map(String::as_str)
        // `find_similar_names` takes the name itself out of the dictionary first.
        .filter(|word| *word != target)
        .filter(|word| {
            jaro_winkler_distance(&normalize_for_similarity(word), &normalized_target) >= threshold
        })
        .collect();
    // The ranking key is the unnormalized word, unlike the filter above.
    words.sort_by(|left, right| {
        jaro_winkler_distance(left, &normalized_target)
            .partial_cmp(&jaro_winkler_distance(right, &normalized_target))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    words.reverse();

    // Mistypes first: a small edit distance relative to the length of what was written.
    let allowed = normalized_target.chars().count().div_ceil(4);
    if let Some(word) = words.iter().find(|word| {
        levenshtein_distance(&normalize_for_similarity(word), &normalized_target) <= allowed
    }) {
        return Some(word);
    }
    // Then misspells: closer than the shorter of the two names is long.
    words.into_iter().find(|word| {
        let word = normalize_for_similarity(word);
        let length = normalized_target.chars().count().min(word.chars().count());
        levenshtein_distance(&word, &normalized_target) < length
    })
}

fn normalize_for_similarity(name: &str) -> String {
    name.to_lowercase().replace('@', "")
}

/// `DidYouMean::Levenshtein.distance`, ported from the Text gem implementation it carries.
fn levenshtein_distance(left: &str, right: &str) -> usize {
    let first: Vec<char> = left.chars().collect();
    let second: Vec<char> = right.chars().collect();
    let (n, m) = (first.len(), second.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut d: Vec<usize> = (0..=m).collect();
    let mut x = 0;
    for (index, character) in first.iter().enumerate() {
        let mut i = index + 1;
        for j in 0..m {
            let cost = usize::from(*character != second[j]);
            x = (d[j + 1] + 1).min(i + 1).min(d[j] + cost);
            d[j] = i;
            i = x;
        }
        d[m] = x;
    }
    x
}

/// `DidYouMean::Jaro.distance`.
fn jaro_distance(left: &str, right: &str) -> f64 {
    let mut first: Vec<char> = left.chars().collect();
    let mut second: Vec<char> = right.chars().collect();
    if first.len() > second.len() {
        std::mem::swap(&mut first, &mut second);
    }
    let (length1, length2) = (first.len(), second.len());
    if length1 == 0 {
        return 0.0;
    }
    let range = if length2 > 3 { length2 / 2 - 1 } else { 0 };
    let mut flags1 = vec![false; length1];
    let mut flags2 = vec![false; length2];
    let mut matches = 0.0;

    for i in 0..length1 {
        let last = i + range;
        let mut j = i.saturating_sub(range);
        while j <= last && j < length2 {
            if !flags2[j] && first[i] == second[j] {
                flags2[j] = true;
                flags1[i] = true;
                matches += 1.0;
                break;
            }
            j += 1;
        }
    }

    let mut transpositions: f64 = 0.0;
    let mut k = 0;
    for i in 0..length1 {
        if !flags1[i] {
            continue;
        }
        let mut index = k;
        let mut j = k;
        while j < length2 {
            index = j;
            if flags2[j] {
                k = j + 1;
                break;
            }
            j += 1;
        }
        if index < length2 && first[i] != second[index] {
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

/// `DidYouMean::JaroWinkler.distance`.
fn jaro_winkler_distance(left: &str, right: &str) -> f64 {
    const WEIGHT: f64 = 0.1;
    const THRESHOLD: f64 = 0.7;
    let jaro = jaro_distance(left, right);
    if jaro <= THRESHOLD {
        return jaro;
    }
    let second: Vec<char> = right.chars().collect();
    let mut prefix = 0;
    for character in left.chars() {
        if prefix < 4 && second.get(prefix) == Some(&character) {
            prefix += 1;
        } else {
            break;
        }
    }
    jaro + (prefix as f64 * WEIGHT * (1.0 - jaro))
}

/// `CommentConfig#comment_only_line?`: no token that is not a comment starts on the line.
fn is_comment_only_line(source: &SourceFile, comment_ranges: &[Range<usize>], line: usize) -> bool {
    let range = source.line_range(line);
    let mut cursor = range.start;
    for comment in comment_ranges {
        if comment.end <= cursor || comment.start >= range.end {
            continue;
        }
        if source
            .slice(cursor..comment.start.max(cursor))
            .trim()
            .is_empty()
        {
            cursor = comment.end.max(cursor);
        } else {
            return false;
        }
    }
    source
        .slice(cursor..range.end.max(cursor))
        .trim()
        .is_empty()
}

/// `CommentConfig#analyze_cop`.
fn analyze_cop(
    analysis: &CopAnalysis,
    directive: &DirectiveComment,
    comment_only_line: bool,
) -> CopAnalysis {
    let line = directive.line as i64;
    // A directive that shares its line with code, or that does not open its comment, only ever
    // covers that one line.
    if !comment_only_line || directive.single_line {
        if !directive.disabled() {
            return analysis.clone();
        }
        let mut ranges = analysis.ranges.clone();
        ranges.push(LineRange {
            begin: line,
            end: line,
        });
        return CopAnalysis {
            ranges,
            start: analysis.start,
        };
    }
    let mut ranges = analysis.ranges.clone();
    if let Some(start) = analysis.start {
        ranges.push(LineRange {
            begin: start,
            end: line,
        });
    }
    CopAnalysis {
        ranges,
        start: directive.disabled().then_some(line),
    }
}

/// `CommentConfig#apply_cop_op`.
fn apply_push_operation(
    analyses: &mut BTreeMap<String, CopAnalysis>,
    operator: char,
    cop: String,
    line: i64,
) {
    let analysis = analyses.entry(cop).or_default();
    match (operator, analysis.start) {
        ('-', None) => analysis.start = Some(line),
        ('+', Some(start)) => {
            analysis.ranges.push(LineRange {
                begin: start,
                end: line,
            });
            analysis.start = None;
        }
        _ => {}
    }
}

/// `CommentConfig#pop_state`.
fn pop_state(
    analyses: &mut BTreeMap<String, CopAnalysis>,
    restore: &BTreeMap<String, CopAnalysis>,
    line: i64,
) {
    let cops: Vec<String> = restore
        .keys()
        .chain(analyses.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for cop in cops {
        let current = analyses.entry(cop.clone()).or_default();
        if let Some(start) = current.start {
            current.ranges.push(LineRange {
                begin: start,
                end: line - 1,
            });
        }
        current.start = restore
            .get(&cop)
            .and_then(|analysis| analysis.start)
            .map(|_| line);
    }
}

#[cfg(test)]
mod tests {
    use crate::diagnostic::{Offense, Severity};
    use crate::source::SourceFile;

    use super::DirectiveState;

    #[test]
    fn handles_block_inline_reason_and_push_pop_directives() {
        let source = SourceFile::new(
            "test.rb",
            "# rubocop:disable Layout -- legacy\n\
             a = 1  \n\
             # rubocop:enable Layout\n\
             b = 2  # rubocop:disable Layout/TrailingWhitespace\n\
             # rubocop:push -Layout/TrailingWhitespace\n\
             c = 3  \n\
             # rubocop:pop\n\
             d = 4  \n"
                .to_owned(),
        );
        let comment_ranges: Vec<_> = (1..=source.line_count())
            .filter_map(|line_number| {
                let line = source.line(line_number);
                let local_start = line.find('#')?;
                let start = source.line_start(line_number) + local_start;
                Some(start..start + line[local_start..].trim_end().len())
            })
            .collect();
        let directives = DirectiveState::parse(&source, &comment_ranges);
        for (line, expected) in [(2, true), (4, true), (6, true), (8, false)] {
            let offense = Offense::new(
                "Layout/TrailingWhitespace",
                Severity::Convention,
                "test",
                source.line_start(line),
                source.line_start(line) + 1,
            );
            assert_eq!(
                directives.suppression(&offense, &source).is_some(),
                expected
            );
        }
        let offense = Offense::new(
            "Layout/TrailingWhitespace",
            Severity::Convention,
            "test",
            source.line_start(2),
            source.line_start(2) + 1,
        );
        assert_eq!(
            directives.suppression(&offense, &source),
            Some(Some("legacy".to_owned()))
        );
    }
}
