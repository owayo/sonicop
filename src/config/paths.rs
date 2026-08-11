use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};
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

pub(super) fn cop_patterns(raw: &Value, name: &str, key: &str) -> Option<PathPatterns> {
    mapping_patterns(raw.as_mapping()?.get(name)?.as_mapping()?, key)
}

fn mapping_patterns(mapping: &Mapping, key: &str) -> Option<PathPatterns> {
    let patterns: Vec<String> = serde_yaml_ng::from_value(mapping.get(key)?.clone()).ok()?;
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
}

impl PathPatterns {
    fn compile(patterns: &[String]) -> Self {
        Self {
            configured: patterns.len(),
            patterns: patterns
                .iter()
                .filter_map(|pattern| CompiledPattern::compile(pattern))
                .collect(),
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
        self.matches(path, root, true, false)
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
        self.patterns
            .iter()
            .filter(|pattern| !absolute_only || pattern.absolute)
            .any(|pattern| pattern.matches(normalized, basename, hidden))
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
        })
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
                    .any(|path| ancestor.is_match(path.to_string_lossy().replace('\\', "/")))
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
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.to_path_buf());
    }
    if let Ok(relative) = strip_verbatim(path).strip_prefix(strip_verbatim(root)) {
        return Some(relative.to_path_buf());
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
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| part.starts_with('.') && part != "." && part != "..")
    })
}

/// Verbatim copy of the pre-compilation matcher, kept so the compiled matcher can be
/// proven byte-for-byte equivalent over a cross product of paths and patterns.
#[cfg(test)]
fn matches_patterns_reference(
    path: &Path,
    root: &Path,
    patterns: &[String],
    respect_hidden: bool,
) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let normalized = relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned();
    let basename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let contains_hidden_component = Path::new(&normalized).components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| part.starts_with('.') && part != "." && part != "..")
    });
    patterns.iter().any(|pattern| {
        let pattern = pattern.trim_start_matches("./");
        if respect_hidden
            && contains_hidden_component
            && !pattern.starts_with('.')
            && !pattern.contains("/.")
        {
            return false;
        }
        Glob::new(pattern)
            .map(|glob| {
                let matcher = glob.compile_matcher();
                matcher.is_match(&normalized)
                    || pattern.strip_suffix("/**/*").is_some_and(|prefix| {
                        Glob::new(prefix).is_ok_and(|prefix_glob| {
                            let prefix_matcher = prefix_glob.compile_matcher();
                            Path::new(&normalized).ancestors().skip(1).any(|ancestor| {
                                prefix_matcher
                                    .is_match(ancestor.to_string_lossy().replace('\\', "/"))
                            })
                        })
                    })
                    || (!pattern.contains('/') && matcher.is_match(basename))
            })
            .unwrap_or(false)
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

    use super::{
        PathPatterns, matches_any_reference, matches_patterns_reference, project_relative,
        strip_verbatim,
    };

    fn owned(patterns: &[&str]) -> Vec<String> {
        patterns
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect()
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
                }
            }
        }
    }
}
