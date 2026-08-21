use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};
use serde_yaml_ng::{Mapping, Value};

pub(super) fn compile_excludes(raw: &Value) -> HashMap<String, PathPatterns> {
    let Some(mapping) = raw.as_mapping() else {
        return HashMap::new();
    };
    mapping
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str()?;
            let patterns = mapping_patterns(value.as_mapping()?, "Exclude")?;
            Some((name.to_owned(), patterns))
        })
        .collect()
}

/// Every cop's own `Include` list.
///
/// `AllCops/Include` chooses the files a run inspects; a cop's own `Include` chooses which of them
/// it applies to, which is how the `Bundler` and `Gemspec` departments stay off ordinary Ruby.
/// A cop that names no `Include` has no entry here and applies to everything.
pub(super) fn compile_includes(raw: &Value) -> HashMap<String, PathPatterns> {
    let Some(mapping) = raw.as_mapping() else {
        return HashMap::new();
    };
    mapping
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str()?;
            let patterns = mapping_patterns(value.as_mapping()?, "Include")?;
            Some((name.to_owned(), patterns))
        })
        .collect()
}

pub(super) fn cop_patterns(raw: &Value, name: &str, key: &str) -> Option<PathPatterns> {
    mapping_patterns(raw.as_mapping()?.get(name)?.as_mapping()?, key)
}

fn mapping_patterns(mapping: &Mapping, key: &str) -> Option<PathPatterns> {
    let value = mapping.get(key)?;
    // `Array(...)`: a lone scalar counts as a one-element list. `Exclude: tmp/**/*`, written
    // without the list dashes, is a common enough slip that it has to mean something, and Ruby's
    // own idiom for reading these lists says what. Upstream does not survive the shape at all --
    // `Config#make_excludes_absolute` raises `undefined method 'map!' for an instance of String` --
    // and this project answers a malformed configuration rather than reproducing a crash. What it
    // must not do is the third thing, which is what deserializing straight into `Vec<String>` and
    // dropping the error did: forgetting the entry leaves an `Exclude` silently unenforced, so the
    // files the user ruled out get inspected and nothing says why.
    let patterns: Vec<String> = match value.as_str() {
        Some(pattern) => vec![pattern.to_owned()],
        None => serde_yaml_ng::from_value(value.clone()).ok()?,
    };
    Some(PathPatterns::compile(&patterns))
}

/// `Include`/`Exclude` globs compiled once when the configuration is built.
///
/// `globset` builds a regular expression per glob, so compiling inside the match
/// call meant cop-count x file-count regex builds per run.
#[derive(Clone, Debug, Default)]
pub(super) struct PathPatterns {
    /// Patterns that fail to compile are dropped but still counted, so an
    /// unusable `Include` list keeps meaning "nothing matches" rather than
    /// degrading into "no list configured".
    configured: usize,
    patterns: Vec<CompiledPattern>,
    /// Every pattern, asked in one go.
    all: PatternSet,
    /// The patterns written with a capital, which is the subset
    /// `matches_uppercase_includes` walks.
    uppercase: PatternSet,
    /// Whether any pattern is absolute. Only those are ever held against the absolute spelling of a
    /// path, so a list without one -- which is every list RuboCop ships and nearly every list a
    /// project writes -- skips that second pass and the `PathBuf` it would build per file.
    has_absolute: bool,
}

/// One list of globs folded into `globset`'s set form so a path can be tested against all of them
/// at once.
///
/// `globset` sorts a set into literal, basename, extension and prefix tables and only falls back to
/// a regex for the patterns none of those cover, so asking the 46 default `Include` globs together
/// costs about what asking one of them separately did. Walking `patterns` one at a time is kept for
/// the cases the sets cannot answer -- a hidden path, where each glob has its own say about whether
/// it reaches the dot -- so the sets are an accelerator, never the authority.
#[derive(Clone, Debug, Default)]
struct PatternSet {
    /// Matched against the project-relative path.
    full: GlobSet,
    /// The separator-free patterns, matched against the basename as well.
    basename: GlobSet,
    /// The `dir` half of a `dir/**/*` pattern, matched against each ancestor of the path.
    ancestor: GlobSet,
    /// Whether `ancestor` holds anything, so the ancestor walk is skipped when it does not.
    has_ancestor: bool,
    /// Whether every builder produced a set. A failure only costs speed: the caller falls back to
    /// walking the patterns one at a time, which is what it did before the sets existed.
    usable: bool,
}

impl PatternSet {
    fn build<'a>(patterns: impl Iterator<Item = &'a CompiledPattern>) -> Self {
        let mut full = GlobSetBuilder::new();
        let mut basename = GlobSetBuilder::new();
        let mut ancestor = GlobSetBuilder::new();
        let mut has_ancestor = false;
        for pattern in patterns {
            full.add(pattern.matcher.glob().clone());
            if pattern.basename {
                basename.add(pattern.matcher.glob().clone());
            }
            if let Some(prefix) = pattern.ancestor.as_ref() {
                ancestor.add(prefix.glob().clone());
                has_ancestor = true;
            }
        }
        match (full.build(), basename.build(), ancestor.build()) {
            (Ok(full), Ok(basename), Ok(ancestor)) => Self {
                full,
                basename,
                ancestor,
                has_ancestor,
                usable: true,
            },
            _ => Self::default(),
        }
    }

    /// The same question `CompiledPattern::matches` answers for one pattern, asked of the whole set.
    /// Only reachable where every pattern would have been asked with `hidden` false, so the
    /// `skips_hidden` shortcut has no say here.
    fn matches(&self, normalized: &str, basename: &str) -> bool {
        self.full.is_match(normalized)
            || (self.has_ancestor
                && Path::new(normalized)
                    .ancestors()
                    .skip(1)
                    .any(|path| self.ancestor.is_match(slashed(path).as_ref())))
            || self.basename.is_match(basename)
    }
}

/// `path` as text with Windows separators folded to `/`, borrowing whenever there is nothing to
/// fold -- which is every path on a Unix checkout.
fn slashed(path: &Path) -> Cow<'_, str> {
    match path.to_string_lossy() {
        Cow::Borrowed(text) if !text.contains('\\') => Cow::Borrowed(text),
        Cow::Borrowed(text) => Cow::Owned(text.replace('\\', "/")),
        Cow::Owned(text) if !text.contains('\\') => Cow::Owned(text),
        Cow::Owned(text) => Cow::Owned(text.replace('\\', "/")),
    }
}

#[derive(Clone, Debug)]
struct CompiledPattern {
    matcher: GlobMatcher,
    /// Set for `dir/**/*`, which also matches paths under an ancestor matching `dir`.
    ancestor: Option<GlobMatcher>,
    absolute: bool,
    /// A separator-free pattern such as `Gemfile` also matches by basename.
    basename: bool,
    /// A pattern naming no dot-component does not opt into hidden paths.
    skips_hidden: bool,
    /// The pattern's dot-prefixed segments, such as `.git` in `.git/**/*`, compiled on their own so
    /// they can be asked whether they reach a hidden part of a path.
    dot_segments: Vec<GlobMatcher>,
    /// `/[A-Z]/.match?(pattern)`: whether the pattern names a file by a capitalised name of its
    /// own, which is what `allowed_camel_case_file?` selects on.
    has_uppercase: bool,
}

impl PathPatterns {
    fn compile(patterns: &[String]) -> Self {
        let compiled: Vec<CompiledPattern> = patterns
            .iter()
            .filter_map(|pattern| CompiledPattern::compile(pattern))
            .collect();
        let all = PatternSet::build(compiled.iter());
        let uppercase = PatternSet::build(compiled.iter().filter(|pattern| pattern.has_uppercase));
        let has_absolute = compiled.iter().any(|pattern| pattern.absolute);
        Self {
            configured: patterns.len(),
            patterns: compiled,
            all,
            uppercase,
            has_absolute,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.configured == 0
    }

    pub(super) fn matches_any(&self, path: &Path, root: &Path) -> bool {
        // Relative patterns must not reach paths outside the project root.
        let absolute_only = path.is_absolute() && project_relative(path, root).is_none();
        self.matches(path, root, false, absolute_only)
    }

    pub(super) fn matches_includes(&self, path: &Path, root: &Path) -> bool {
        self.matches_includes_inner(path, root, false)
    }

    /// `file_to_include?(file) { |pattern| /[A-Z]/.match?(pattern.to_s) }`: the same walk over the
    /// `Include` list, restricted to the patterns written with a capital. A file that only reaches
    /// the run through one of those -- `Rakefile`, `Gemfile` -- is named after the pattern rather
    /// than after Ruby's conventions, so `Naming/FileName` lets it be.
    pub(super) fn matches_uppercase_includes(&self, path: &Path, root: &Path) -> bool {
        self.matches_includes_inner(path, root, true)
    }

    fn matches_includes_inner(&self, path: &Path, root: &Path, uppercase_only: bool) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let relative = project_relative(path, root).unwrap_or_else(|| path.to_path_buf());
        let relative = relative.to_string_lossy().replace('\\', "/");
        let basename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if self.matches_includes_spelling(
            relative.trim_start_matches("./"),
            basename,
            uppercase_only,
            false,
        ) {
            return true;
        }
        // `match_relative_or_absolute_path?`: an `Include` pattern written absolutely is held
        // against the absolute spelling of the file, which is the only spelling it can ever match.
        // Only the absolute patterns are asked -- upstream picks the spelling per pattern, and
        // handing a relative pattern the absolute path would let `**/vendor/**/*` match by way of a
        // directory above the project root that the user never named.
        if !self.has_absolute {
            return false;
        }
        let Some(absolute) = absolute_form(path) else {
            return false;
        };
        self.matches_includes_spelling(slashed(&absolute).as_ref(), basename, uppercase_only, true)
    }

    /// One spelling of a path put to the `Include` list. `absolute_only` restricts the question to
    /// the absolute patterns and takes the slow walk, because the sets are built over the whole
    /// list and there is no set for that subset.
    fn matches_includes_spelling(
        &self,
        normalized: &str,
        basename: &str,
        uppercase_only: bool,
        absolute_only: bool,
    ) -> bool {
        // RuboCop's `match_path?` matches without `FNM_DOTMATCH`, so a wildcard never reaches a dot
        // component -- only a segment the pattern spells out literally does. On top of that,
        // `hidden_file_in_not_hidden_dir?` lets a dot *file* through as long as it sits in a real,
        // non-hidden directory. So a dot directory is invisible unless named, and a top-level dot
        // file needs naming too.
        let (directories, file) = normalized.rsplit_once('/').unwrap_or(("", normalized));
        let hidden_file = hidden_segment(file);
        let hidden_directory = directories.split('/').any(hidden_segment);
        // With nothing hidden about the path, every pattern would clear the two dot tests below on
        // its own, and what is left is the plain glob question the set can answer in one go.
        if !absolute_only {
            let set = if uppercase_only {
                &self.uppercase
            } else {
                &self.all
            };
            if set.usable && !hidden_directory && !(hidden_file && directories.is_empty()) {
                return set.matches(normalized, basename);
            }
        }
        let hidden_directories: Vec<&str> = directories
            .split('/')
            .filter(|part| hidden_segment(part))
            .collect();
        self.patterns.iter().any(|pattern| {
            if uppercase_only && !pattern.has_uppercase {
                return false;
            }
            if absolute_only && !pattern.absolute {
                return false;
            }
            if !hidden_directories
                .iter()
                .all(|part| pattern.reaches_hidden(part))
            {
                return false;
            }
            if hidden_file && directories.is_empty() && !pattern.reaches_hidden(file) {
                return false;
            }
            pattern.matches(normalized, basename, false)
        })
    }

    fn matches(&self, path: &Path, root: &Path, respect_hidden: bool, absolute_only: bool) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let relative = project_relative(path, root).unwrap_or_else(|| path.to_path_buf());
        let relative = relative.to_string_lossy().replace('\\', "/");
        let normalized = relative.trim_start_matches("./");
        let basename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let hidden = respect_hidden && has_hidden_component(Path::new(normalized));
        // `hidden` and `absolute_only` are the only things that put a pattern out of the running on
        // its own; without them the set answers for the whole list at once.
        let matched = if self.all.usable && !hidden && !absolute_only {
            self.all.matches(normalized, basename)
        } else {
            self.patterns
                .iter()
                .filter(|pattern| !absolute_only || pattern.absolute)
                .any(|pattern| pattern.matches(normalized, basename, hidden))
        };
        if matched {
            return true;
        }
        // `file_to_exclude?` expands the file before matching, so an `Exclude` written absolutely
        // reaches project files upstream. The same restriction as on the `Include` side applies:
        // only the absolute patterns get to see the absolute spelling.
        if !self.has_absolute {
            return false;
        }
        let Some(absolute) = absolute_form(path) else {
            return false;
        };
        let absolute = slashed(&absolute);
        let hidden = respect_hidden && has_hidden_component(Path::new(absolute.as_ref()));
        self.patterns
            .iter()
            .filter(|pattern| pattern.absolute)
            .any(|pattern| pattern.matches(absolute.as_ref(), basename, hidden))
    }
}

impl CompiledPattern {
    fn compile(pattern: &str) -> Option<Self> {
        let absolute = Path::new(pattern).is_absolute();
        let pattern = pattern.trim_start_matches("./");
        Some(Self {
            matcher: Glob::new(pattern).ok()?.compile_matcher(),
            ancestor: pattern
                .strip_suffix("/**/*")
                .and_then(|prefix| Glob::new(prefix).ok())
                .map(|prefix| prefix.compile_matcher()),
            absolute,
            basename: !pattern.contains('/'),
            skips_hidden: !pattern.starts_with('.') && !pattern.contains("/."),
            dot_segments: pattern
                .split('/')
                .filter(|segment| segment.starts_with('.'))
                .filter_map(|segment| Glob::new(segment).ok())
                .map(|glob| glob.compile_matcher())
                .collect(),
            has_uppercase: pattern
                .chars()
                .any(|character| character.is_ascii_uppercase()),
        })
    }

    /// Whether the pattern can reach a hidden path segment. A wildcard cannot -- RuboCop matches
    /// without `FNM_DOTMATCH` -- so only a segment of the pattern that itself begins with a dot
    /// does, and it still has to match the segment.
    fn reaches_hidden(&self, segment: &str) -> bool {
        self.dot_segments
            .iter()
            .any(|matcher| matcher.is_match(segment))
    }

    fn matches(&self, normalized: &str, basename: &str, hidden: bool) -> bool {
        if hidden && self.skips_hidden {
            return false;
        }
        self.matcher.is_match(normalized)
            || self.ancestor.as_ref().is_some_and(|ancestor| {
                Path::new(normalized)
                    .ancestors()
                    .skip(1)
                    .any(|path| ancestor.is_match(slashed(path).as_ref()))
            })
            || (self.basename && self.matcher.is_match(basename))
    }
}

/// `path` expressed relative to the project root, or `None` when it lies outside.
///
/// `strip_prefix` compares text, and on Windows the same directory has more than one spelling:
/// `fs::canonicalize` -- which is how the project root is resolved -- returns a `\\?\` verbatim
/// path and expands 8.3 short names, while a path taken from the current directory or the command
/// line keeps whatever spelling the caller used. A plain `strip_prefix` therefore fails for every
/// file, every `Include`/`Exclude` pattern is then matched against an absolute path instead of a
/// project-relative one, and a project living under a dot-named directory has all of its files
/// treated as hidden. The text comparisons are tried first because they cover every normal case;
/// only when they disagree is it worth asking the filesystem.
pub(super) fn project_relative(path: &Path, root: &Path) -> Option<PathBuf> {
    // `strip_prefix` compares component by component, so a `..` left in the path survives into the
    // result: `sonicop ../lib` run from a subdirectory produced `sub/../lib/a.rb` where upstream
    // produces `lib/a.rb`, and every wildcard pattern then refused the file over the dot segment.
    // Upstream never sees one because `PathUtil.relative_path` runs the path through
    // `File.expand_path` first, so the fold happens here too.
    let folded = without_parent_segments(path);
    let path = folded.as_ref();
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.to_path_buf());
    }
    if let Ok(relative) = strip_verbatim(path).strip_prefix(strip_verbatim(root)) {
        return Some(relative.to_path_buf());
    }
    // A path spelled relative to the working directory -- which is every path a run started with
    // `sonicop .` walks -- shares no text with an absolute root, so both comparisons above miss and
    // the file falls through to `canonicalize`. That is two syscalls per question and three
    // questions per file, which cost more than inspecting the file did. Joining the working
    // directory answers the same question without asking the filesystem anything; `Components`
    // drops the `.` segments a walk introduces, and the join is folded again because a leading `..`
    // only has something to cancel once the working directory is in front of it.
    if path.is_relative() {
        if let Some(joined) = working_directory().map(|cwd| cwd.join(path)) {
            let joined = without_parent_segments(&joined);
            let joined = joined.as_ref();
            if let Ok(relative) = joined.strip_prefix(root) {
                return Some(relative.to_path_buf());
            }
            if let Ok(relative) = strip_verbatim(joined).strip_prefix(strip_verbatim(root)) {
                return Some(relative.to_path_buf());
            }
        }
    }
    let resolved = fs::canonicalize(path).ok()?;
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    resolved
        .strip_prefix(&root)
        .ok()
        .or_else(|| {
            strip_verbatim(&resolved)
                .strip_prefix(strip_verbatim(&root))
                .ok()
        })
        .map(Path::to_path_buf)
}

/// `path` with its `..` segments folded away, borrowed whenever it has none -- which is every path
/// a `sonicop .` run walks.
///
/// This is for `project_relative`, whose comparisons are all `strip_prefix` and therefore run over
/// `Components`, which already hides the `.` segments a walk introduces. Only `..` needs settling
/// there, and settling it is lexical, exactly as `File.expand_path` is: a `..` that crosses a
/// symlink lands where the text says rather than where the link points. That is upstream's answer
/// as well, and it is what keeps this from costing a `readlink` per file. Matching against the
/// *text* of a path needs more than this -- see `folded_components`.
fn without_parent_segments(path: &Path) -> Cow<'_, Path> {
    if !path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        // `strip_prefix` -- the only thing the fold is for here -- compares `Components`, and those
        // already drop the `.` segments a walk introduces. A path with no `..` in it is therefore
        // its own folded form as far as `project_relative` is concerned, and can be borrowed.
        return Cow::Borrowed(path);
    }
    Cow::Owned(folded_components(path))
}

/// `path` rebuilt out of its components, with `.` and `..` folded away.
///
/// Unlike `without_parent_segments` this always rebuilds, because a glob is matched against the
/// *text* of a path: `Components` hides the `./` a `sonicop .` walk leaves in the middle of
/// `<cwd>/./lib/a.rb`, but `to_string_lossy` does not, and no pattern spelled `<root>/lib/**/*.rb`
/// can match a string with that `./` still in it.
fn folded_components(path: &Path) -> PathBuf {
    let mut kept: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match kept.last() {
                // `a/../b` is `b`.
                Some(Component::Normal(_)) => {
                    kept.pop();
                }
                // `/..` is `/`: a path cannot climb past its own root.
                Some(Component::RootDir) => {}
                // A leading `..`, or one following another, has nothing to cancel and has to
                // survive -- `File.expand_path` keeps those too, once a relative path has run out
                // of segments to undo.
                _ => kept.push(component),
            },
            other => kept.push(other),
        }
    }
    kept.iter().collect()
}

/// `File.expand_path(path)`: the absolute spelling of `path`, folded.
///
/// `Config#file_to_exclude?` matches the absolute path and `Config#file_to_include?` matches the
/// relative *or* the absolute one, so a pattern written absolutely reaches project files upstream.
/// The working directory is what a relative path is joined to, because that is what
/// `File.expand_path` uses -- not the project root, which a run started outside the project does
/// not sit in.
fn absolute_form(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(folded_components(path));
    }
    Some(folded_components(&working_directory()?.join(path)))
}

/// The working directory, asked of the kernel once.
///
/// Nothing in the program changes it, and `project_relative` needs it for every file a relative
/// argument expands to, so the answer is kept rather than fetched again.
fn working_directory() -> Option<&'static Path> {
    static CWD: OnceLock<Option<PathBuf>> = OnceLock::new();
    CWD.get_or_init(|| std::env::current_dir().ok()).as_deref()
}

/// Drops Windows' `\\?\` extended-length marker so that two spellings of one path can be compared.
/// Other platforms never carry the prefix, so this is the identity there.
fn strip_verbatim(path: &Path) -> &Path {
    let text = match path.to_str() {
        Some(text) => text,
        None => return path,
    };
    text.strip_prefix(r"\\?\UNC\")
        .map(Path::new)
        .or_else(|| text.strip_prefix(r"\\?\").map(Path::new))
        .unwrap_or(path)
}

/// A path counts as hidden when any component below the project root begins with
/// a dot.
///
/// `Config::path_hidden` and the `Include` matcher ask exactly this question of
/// the same project-relative path, so both walk the components here instead of
/// each carrying its own copy of the rule.
pub(super) fn has_hidden_component(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_str().is_some_and(hidden_segment))
}

/// Whether one path segment is hidden.
///
/// `.` and `..` begin with a dot without being hidden, and both can reach a matcher: a path outside
/// the project root keeps its `..` segments, and a walk hands out `./`-prefixed spellings.
/// Upstream draws the same line -- `file_to_include?` asks
/// `relative_file_path.start_with?('.') && !relative_file_path.start_with?('..')`. Reading a `..`
/// as hidden put every wildcard pattern out of the running, which is how `sonicop ../lib` came to
/// inspect nothing at all.
fn hidden_segment(segment: &str) -> bool {
    segment.starts_with('.') && segment != "." && segment != ".."
}

/// Verbatim copy of the pre-compilation matcher, kept so the compiled matcher can be
/// proven byte-for-byte equivalent over a cross product of paths and patterns.
///
/// It compiles a `Glob` per pattern per question and knows nothing of `PatternSet`, `has_absolute`
/// or the dot-segment tables, so the accelerators the real matcher is built out of have somewhere
/// to be checked against. What it does share is `absolute_form` and `hidden_segment`: those decide
/// *which path* is being matched rather than *how*, and a second copy of them would only prove the
/// two copies agree.
#[cfg(test)]
fn matches_patterns_reference(
    path: &Path,
    root: &Path,
    patterns: &[String],
    respect_hidden: bool,
) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let relative = relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned();
    let absolute = absolute_form(path).map(|path| slashed(&path).into_owned());
    let basename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    patterns.iter().any(|pattern| {
        // `match_relative_or_absolute_path?`: the absolute spelling of the path is in play for an
        // absolute pattern and for nothing else.
        let mut spellings = vec![relative.as_str()];
        if Path::new(pattern.as_str()).is_absolute()
            && let Some(absolute) = absolute.as_deref()
        {
            spellings.push(absolute);
        }
        let pattern = pattern.trim_start_matches("./");
        // Only a dot-prefixed segment of the pattern reaches a dot component, and a dot file at the
        // top level needs the same. Below the top level a dot file is reached anyway.
        let reaches = |segment: &str| {
            pattern.split('/').any(|part| {
                part.starts_with('.')
                    && Glob::new(part).is_ok_and(|glob| glob.compile_matcher().is_match(segment))
            })
        };
        spellings.into_iter().any(|normalized| {
            let (directories, file) = normalized.rsplit_once('/').unwrap_or(("", normalized));
            let hidden_directories: Vec<&str> = directories
                .split('/')
                .filter(|part| hidden_segment(part))
                .collect();
            let hidden_file = hidden_segment(file);
            if respect_hidden
                && (!hidden_directories.iter().all(|part| reaches(part))
                    || (hidden_file && directories.is_empty() && !reaches(file)))
            {
                return false;
            }
            Glob::new(pattern)
                .map(|glob| {
                    let matcher = glob.compile_matcher();
                    matcher.is_match(normalized)
                        || pattern.strip_suffix("/**/*").is_some_and(|prefix| {
                            Glob::new(prefix).is_ok_and(|prefix_glob| {
                                let prefix_matcher = prefix_glob.compile_matcher();
                                Path::new(normalized).ancestors().skip(1).any(|ancestor| {
                                    prefix_matcher
                                        .is_match(ancestor.to_string_lossy().replace('\\', "/"))
                                })
                            })
                        })
                        || (!pattern.contains('/') && matcher.is_match(basename))
                })
                .unwrap_or(false)
        })
    })
}

#[cfg(test)]
fn matches_any_reference(path: &Path, root: &Path, patterns: &[String]) -> bool {
    if path.is_absolute() && path.strip_prefix(root).is_err() {
        let absolute_patterns: Vec<_> = patterns
            .iter()
            .filter(|pattern| Path::new(pattern.as_str()).is_absolute())
            .cloned()
            .collect();
        return matches_patterns_reference(path, root, &absolute_patterns, false);
    }
    matches_patterns_reference(path, root, patterns, false)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_yaml_ng::Value;

    use super::{
        PathPatterns, hidden_segment, mapping_patterns, matches_any_reference,
        matches_patterns_reference, project_relative, slashed, strip_verbatim,
    };

    fn owned(patterns: &[&str]) -> Vec<String> {
        patterns
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect()
    }

    /// `AllCops`'s `Include`/`Exclude` read the way the configuration hands it over, so the shape
    /// of the YAML is part of what is under test.
    fn all_cops_patterns(yaml: &str, key: &str) -> Option<PathPatterns> {
        let value: Value = serde_yaml_ng::from_str(yaml).unwrap();
        let all_cops = value.as_mapping().unwrap().get("AllCops").unwrap();
        mapping_patterns(all_cops.as_mapping().unwrap(), key)
    }

    fn includes(path: &str, root: &str, patterns: &[&str]) -> bool {
        PathPatterns::compile(&owned(patterns)).matches_includes(Path::new(path), Path::new(root))
    }

    fn excludes(path: &str, root: &str, patterns: &[&str]) -> bool {
        PathPatterns::compile(&owned(patterns)).matches_any(Path::new(path), Path::new(root))
    }

    /// `[project root, a path outside it, an absolute pattern reaching it]`. Forward slashes work
    /// on Windows too, and a glob treats a backslash as an escape, so the pattern keeps `/`.
    ///
    /// `cfg!` rather than `#[cfg]` so that both spellings are type checked everywhere. An item
    /// behind `#[cfg(windows)]` is compiled only on Windows, which is where a mistake in it would
    /// first appear -- and this repository is developed on Unix.
    fn outside_root_case() -> [&'static str; 3] {
        if cfg!(windows) {
            ["C:/p", "C:/other/x.rb", "C:/other/**/*"]
        } else {
            ["/p", "/other/x.rb", "/other/**/*"]
        }
    }

    /// Windows は同じディレクトリを複数の綴りで表す。プロジェクトルートは `fs::canonicalize`
    /// 由来の `\\?\` 付きになる一方、検査対象は `current_dir` やコマンドラインの綴りのまま
    /// 届くため、素の `strip_prefix` は全ファイルで失敗する。綴りを揃えるこの部分だけは
    /// 文字列処理なので、Windows でなくても固定できる。
    #[test]
    fn the_verbatim_marker_is_dropped_before_comparing() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\p")), Path::new(r"C:\p"));
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share")),
            Path::new(r"server\share")
        );
        // 付いていないものは素通し。
        assert_eq!(strip_verbatim(Path::new(r"C:\p")), Path::new(r"C:\p"));
        assert_eq!(strip_verbatim(Path::new("/p")), Path::new("/p"));
    }

    #[test]
    fn a_path_under_the_root_is_relative_to_it() {
        assert_eq!(
            project_relative(Path::new("/p/lib/a.rb"), Path::new("/p")),
            Some(PathBuf::from("lib/a.rb"))
        );
    }

    /// `sonicop .` が渡すのは `./lib/a.rb` のような相対パスで、絶対のルートとは文字列が
    /// 重ならない。作業ディレクトリを継ぎ足して解決できなければ、ファイルごとに
    /// `canonicalize` へ落ちて探索が検査より高くつく。存在しないパスで確かめているのは、
    /// 継ぎ足しで答えが出たことを `canonicalize` の成功と取り違えないため。
    #[test]
    fn a_relative_path_resolves_against_the_working_directory() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            project_relative(Path::new("nowhere/lib/a.rb"), &cwd.join("nowhere")),
            Some(PathBuf::from("lib/a.rb"))
        );
        // 走査が挟む `.` は `Components` が落とすので、綴りの違いは結果に出ない。
        assert_eq!(
            project_relative(Path::new("./nowhere/lib/a.rb"), &cwd.join("nowhere")),
            Some(PathBuf::from("lib/a.rb"))
        );
    }

    /// プロジェクト外は None のままでなければ、相対パターンが外部へ届いてしまう。
    #[test]
    fn a_path_outside_the_root_has_no_project_relative_form() {
        assert_eq!(
            project_relative(Path::new("/other/x.rb"), Path::new("/p/does-not-exist")),
            None
        );
    }

    #[test]
    fn exclude_patterns_follow_rubocop_path_semantics() {
        // `dir/**/*` matches everything below the directory.
        assert!(excludes("/p/vendor/bundle/x.rb", "/p", &["vendor/**/*"]));
        assert!(excludes("/p/vendor/bundle", "/p", &["vendor/**/*"]));
        assert!(!excludes("/p/vendor", "/p", &["vendor/**/*"]));
        // A separator-free pattern also matches the basename at any depth.
        assert!(excludes("/p/a/b/Gemfile", "/p", &["Gemfile"]));
        assert!(!excludes("/p/vendor/bundle/x.rb", "/p", &["vendor"]));
        // Relative patterns must not reach outside the project root. The guard turns on `Path::
        // is_absolute`, and a Windows absolute path needs a drive letter, so `/other/x.rb` would
        // be a relative path there and leave the rule untested. Spell the case per platform.
        let [outside_root, outside, outside_pattern] = outside_root_case();
        assert!(excludes(outside, outside_root, &[outside_pattern]));
        assert!(!excludes(outside, outside_root, &["**/*.rb"]));
        assert!(excludes("/p/x.rb", "/p", &["./x.rb"]));
        // An uncompilable pattern never matches.
        assert!(!excludes("/p/x.rb", "/p", &["["]));
    }

    #[test]
    fn include_patterns_opt_out_of_hidden_paths() {
        assert!(!includes("/p/.git/config.rb", "/p", &["**/*.rb"]));
        assert!(includes("/p/.git/config.rb", "/p", &[".git/**/*"]));
        assert!(includes("/p/.git/config.rb", "/p", &["**/.git/**/*"]));
        assert!(includes("/p/a/x.rb", "/p", &["**/*.rb"]));
        // Excludes ignore the hidden rule entirely.
        assert!(excludes("/p/.git/config.rb", "/p", &["**/*.rb"]));
    }

    /// `.` and `..` begin with a dot without being hidden, and a matcher meets both: a walk hands
    /// out `./`-prefixed spellings, and a file outside the project root keeps its `..` segments.
    /// Reading either as hidden put every pattern that does not spell a dot -- which is the whole
    /// of the default `Include` list -- out of the running, and `sonicop ../lib` inspected nothing.
    #[test]
    fn a_parent_directory_segment_is_not_hidden() {
        assert!(!hidden_segment("."));
        assert!(!hidden_segment(".."));
        assert!(!hidden_segment("lib"));
        // The exemption is those two names and nothing else, which is where `hidden_dir?` and
        // `hidden_file?` draw it too -- both ask only `start_with?('.')` of a whole segment.
        assert!(hidden_segment(".git"));
        assert!(hidden_segment("..."));
        assert!(hidden_segment(".rubocop.yml"));
        // A path that keeps its `..` -- one outside the project root -- is still reachable by an
        // ordinary wildcard, which is what upstream does once it switches to the absolute spelling.
        assert!(includes("../other/x.rb", "/p", &["**/*.rb"]));
        // A genuinely hidden directory still has to be named by the pattern.
        assert!(!includes("/p/.git/config.rb", "/p", &["**/*.rb"]));
        assert!(includes("/p/.git/config.rb", "/p", &[".git/**/*"]));
    }

    /// `PathUtil.relative_path` expands the path before comparing it with the base directory, so a
    /// project-relative spelling never keeps a `..`. `strip_prefix` compares components and leaves
    /// one in place, so `sonicop ../lib` run from `sub/` produced `sub/../lib/a.rb` where upstream
    /// produces `lib/a.rb`.
    #[test]
    fn a_parent_directory_target_folds_before_the_root_is_stripped() {
        assert_eq!(
            project_relative(Path::new("/p/sub/../lib/a.rb"), Path::new("/p")),
            Some(PathBuf::from("lib/a.rb"))
        );
        // Folding must not invent a hit: `..` that climbs out of the root still leaves the file
        // outside it.
        assert_eq!(
            project_relative(
                Path::new("/p/../other/x.rb"),
                Path::new("/p/does-not-exist")
            ),
            None
        );
        // The spelling a walk actually produces is relative to the working directory, and a leading
        // `..` only has something to cancel once that directory is in front of it. Non-existent
        // paths again, so that an answer can only have come from the lexical fold.
        let cwd = std::env::current_dir().unwrap();
        let sibling = cwd.parent().unwrap().join("nowhere");
        assert_eq!(
            project_relative(Path::new("../nowhere/lib/a.rb"), &sibling),
            Some(PathBuf::from("lib/a.rb"))
        );
    }

    /// `file_to_exclude?` matches the absolute path and `file_to_include?` the relative *or* the
    /// absolute one, so a pattern written absolutely reaches project files upstream. Asking only
    /// the project-relative spelling meant an absolute `Exclude` was quietly ignored and an
    /// absolute `Include` matched nothing at all.
    #[test]
    fn an_absolute_pattern_reaches_a_file_inside_the_project() {
        // A root under the working directory, so the relative spellings below resolve against it.
        // Nothing here exists; the answers can only come from the lexical expansion.
        let cwd = std::env::current_dir().unwrap();
        let root = cwd.join("nowhere/deep");
        let root_text = slashed(&root).into_owned();
        let file = root.join("lib/a.rb");
        let temporary = root.join("tmp/t.rb");

        let include = PathPatterns::compile(&[format!("{root_text}/lib/**/*.rb")]);
        assert!(include.matches_includes(&file, &root));
        assert!(!include.matches_includes(&temporary, &root));
        // The same file spelled the way a walk hands it over. `./` is the spelling `sonicop .`
        // produces, and `Components` hides it from `strip_prefix` while leaving it in the text a
        // glob is matched against, so it needs its own case.
        assert!(include.matches_includes(Path::new("nowhere/deep/lib/a.rb"), &root));
        assert!(include.matches_includes(Path::new("./nowhere/deep/lib/a.rb"), &root));

        let exclude = PathPatterns::compile(&[format!("{root_text}/tmp/**/*")]);
        assert!(exclude.matches_any(&temporary, &root));
        assert!(exclude.matches_any(Path::new("./nowhere/deep/tmp/t.rb"), &root));
        assert!(!exclude.matches_any(&file, &root));

        // The other side of the boundary: a *relative* pattern is never held against the absolute
        // spelling. `**/nowhere/**/*` names a directory that only exists above the project root, so
        // matching it there would exclude every file in the project over a name the user never
        // wrote about.
        assert!(!excludes(
            "lib/a.rb",
            root.to_str().unwrap(),
            &["**/nowhere/**/*"]
        ));
        assert!(!includes(
            "lib/a.rb",
            root.to_str().unwrap(),
            &["**/nowhere/**/*"]
        ));
        // And a relative pattern still matches the relative spelling, absolutely as before.
        assert!(excludes("lib/a.rb", root.to_str().unwrap(), &["lib/**/*"]));
    }

    /// `Array(...)`: a lone scalar is a one-element list. Upstream raises on the shape
    /// (`undefined method 'map!' for an instance of String`), and the answer this project gives
    /// instead must not be "the entry never existed" -- a forgotten `Exclude` inspects the files
    /// the user ruled out and says nothing about it.
    #[test]
    fn a_scalar_pattern_list_is_read_as_one_element() {
        let scalar = all_cops_patterns("AllCops:\n  Exclude: tmp/**/*\n", "Exclude").unwrap();
        let list = all_cops_patterns("AllCops:\n  Exclude:\n    - tmp/**/*\n", "Exclude").unwrap();
        for patterns in [&scalar, &list] {
            assert!(!patterns.is_empty());
            assert!(patterns.matches_any(Path::new("/p/tmp/t.rb"), Path::new("/p")));
            assert!(!patterns.matches_any(Path::new("/p/lib/a.rb"), Path::new("/p")));
        }

        let scalar = all_cops_patterns("AllCops:\n  Include: lib/**/*.rb\n", "Include").unwrap();
        let list =
            all_cops_patterns("AllCops:\n  Include:\n    - lib/**/*.rb\n", "Include").unwrap();
        for patterns in [&scalar, &list] {
            assert!(patterns.matches_includes(Path::new("/p/lib/a.rb"), Path::new("/p")));
            assert!(!patterns.matches_includes(Path::new("/p/tmp/t.rb"), Path::new("/p")));
        }

        // A key that is not written at all still has no list, which is a different thing from an
        // empty one: `path_included` reads it as "every file".
        assert!(all_cops_patterns("AllCops:\n  Exclude: tmp/**/*\n", "Include").is_none());
        // An empty list stays empty rather than turning into a one-element list of nothing.
        let empty = all_cops_patterns("AllCops:\n  Exclude: []\n", "Exclude").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn an_unusable_include_list_still_counts_as_configured() {
        let patterns = PathPatterns::compile(&owned(&["["]));
        assert!(!patterns.is_empty());
        assert!(!includes("/p/x.rb", "/p", &["["]));
    }

    #[test]
    fn compiled_matcher_agrees_with_the_reference_implementation() {
        let patterns = [
            "**/*.rb",
            "*.rb",
            "Gemfile",
            "./x.rb",
            "vendor/**/*",
            "**/vendor/**/*",
            "db/**/*",
            "**/node_modules/**/*",
            ".git/**/*",
            "**/.*",
            "/abs/**/*",
            "/abs/x.rb",
            // Absolute patterns that reach *into* the project root, which is the shape a hand
            // written `Include`/`Exclude` takes and the one the relative-only matcher never saw.
            "/p/**/*.rb",
            "/p/.git/**/*",
            "spec/**/*_spec.rb",
            "a/*/c",
            "[",
            "tmp",
            "**/tmp/**/*",
            "lib/**/*.rb",
        ];
        let paths = [
            "/p/x.rb",
            "/p/a/x.rb",
            "/p/a/b/c.rb",
            "/p/vendor",
            "/p/vendor/bundle",
            "/p/vendor/bundle/x.rb",
            "/p/.git/config.rb",
            "/p/a/.hidden/x.rb",
            "/p/Gemfile",
            "/p/a/Gemfile",
            "/p/db/migrate",
            "/p/db/migrate/1.rb",
            "/p/node_modules/a/b.rb",
            "/p/a/node_modules",
            "/p/spec/models/user_spec.rb",
            "/p/tmp/deep/dir/file.rb",
            "/abs/x.rb",
            "/abs/deep/x.rb",
            "/other/x.rb",
            "relative/x.rb",
            "/p/a/c",
            "/p/a/b/c",
        ];
        let root = Path::new("/p");
        for window in 1..=3 {
            for start in 0..patterns.len() {
                let selected: Vec<String> = patterns
                    .iter()
                    .cycle()
                    .skip(start)
                    .take(window)
                    .map(|pattern| (*pattern).to_owned())
                    .collect();
                let compiled = PathPatterns::compile(&selected);
                for path in paths {
                    let path = Path::new(path);
                    assert_eq!(
                        compiled.matches_includes(path, root),
                        matches_patterns_reference(path, root, &selected, true),
                        "includes {path:?} against {selected:?}"
                    );
                    assert_eq!(
                        compiled.matches_any(path, root),
                        matches_any_reference(path, root, &selected),
                        "excludes {path:?} against {selected:?}"
                    );
                    // Selecting on `has_uppercase` has to mean the same thing as handing the
                    // matcher only the patterns written with a capital in the first place. The two
                    // reach the question by different routes -- one filters a compiled set, the
                    // other compiles a filtered list -- so they are worth holding against each
                    // other for every window.
                    let capitals: Vec<String> = selected
                        .iter()
                        .filter(|pattern| {
                            pattern
                                .chars()
                                .any(|character| character.is_ascii_uppercase())
                        })
                        .cloned()
                        .collect();
                    assert_eq!(
                        compiled.matches_uppercase_includes(path, root),
                        PathPatterns::compile(&capitals).matches_includes(path, root),
                        "uppercase includes {path:?} against {selected:?}"
                    );
                }
            }
        }
    }
}
