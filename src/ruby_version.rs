use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use tree_sitter::{Node, Parser};

const KNOWN_RUBIES: &[RubyVersion] = &[
    RubyVersion::new(2, 0),
    RubyVersion::new(2, 1),
    RubyVersion::new(2, 2),
    RubyVersion::new(2, 3),
    RubyVersion::new(2, 4),
    RubyVersion::new(2, 5),
    RubyVersion::new(2, 6),
    RubyVersion::new(2, 7),
    RubyVersion::new(3, 0),
    RubyVersion::new(3, 1),
    RubyVersion::new(3, 2),
    RubyVersion::new(3, 3),
    RubyVersion::new(3, 4),
    RubyVersion::new(4, 0),
    RubyVersion::new(4, 1),
];

const DEFAULT_TARGET_RUBY: RubyVersion = RubyVersion::new(2, 7);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RubyVersion {
    major: u16,
    minor: u16,
}

impl RubyVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.trim().trim_start_matches("ruby-").split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        Some(Self::new(major, minor))
    }
}

impl fmt::Display for RubyVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TargetRubySource {
    Environment,
    Configuration,
    Gemspec(PathBuf),
    RubyVersionFile(PathBuf),
    MiseToml(PathBuf),
    ToolVersions(PathBuf),
    BundlerLock(PathBuf),
    Default,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedTargetRuby {
    pub(crate) version: RubyVersion,
    pub(crate) source: TargetRubySource,
}

pub(crate) fn resolve_target_ruby(
    configured: Option<RubyVersion>,
    base_directory: &Path,
) -> Result<ResolvedTargetRuby> {
    if let Some(value) = std::env::var_os("RUBOCOP_TARGET_RUBY_VERSION") {
        let value = value
            .into_string()
            .map_err(|_| anyhow::anyhow!("RUBOCOP_TARGET_RUBY_VERSION is not valid UTF-8"))?;
        let version = RubyVersion::parse(&value)
            .with_context(|| format!("invalid RUBOCOP_TARGET_RUBY_VERSION: {value}"))?;
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::Environment,
        });
    }
    if let Some(version) = configured {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::Configuration,
        });
    }
    if let Some(path) = find_single_gemspec(base_directory)
        && let Some(version) = target_from_gemspec(&path)?
    {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::Gemspec(path),
        });
    }
    if let Some(path) = find_upwards(base_directory, ".ruby-version")
        && let Some(version) = version_file_value(&path, None)
    {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::RubyVersionFile(path),
        });
    }
    if let Some(path) = find_upwards(base_directory, "mise.toml")
        && let Some(version) = version_file_value(&path, Some("ruby ="))
    {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::MiseToml(path),
        });
    }
    if let Some(path) = find_upwards(base_directory, ".tool-versions")
        && let Some(version) = version_file_value(&path, Some("ruby "))
    {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::ToolVersions(path),
        });
    }
    for filename in ["Gemfile.lock", "gems.locked"] {
        if let Some(path) = find_upwards(base_directory, filename)
            && let Some(version) = target_from_lockfile(&path)
        {
            return Ok(ResolvedTargetRuby {
                version,
                source: TargetRubySource::BundlerLock(path),
            });
        }
    }
    Ok(ResolvedTargetRuby {
        version: DEFAULT_TARGET_RUBY,
        source: TargetRubySource::Default,
    })
}

fn find_upwards(start: &Path, filename: &str) -> Option<PathBuf> {
    start.ancestors().find_map(|directory| {
        let candidate = directory.join(filename);
        candidate.is_file().then_some(candidate)
    })
}

fn find_single_gemspec(start: &Path) -> Option<PathBuf> {
    for directory in start.ancestors() {
        // An unreadable ancestor must not abort the walk: a gemspec may still live above it.
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        let mut candidates = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "gemspec")
            });
        let first = candidates.next();
        if first.is_some() && candidates.next().is_none() {
            return first;
        }
    }
    None
}

/// A candidate file read as UTF-8, or `None` when it cannot be.
///
/// Every caller here is looking at a file it merely *guessed* at: `find_single_gemspec` and
/// `find_upwards` walk `start.ancestors()` to the filesystem root, so the candidates include files
/// that have nothing to do with the project -- a gemspec two directories above the checkout,
/// written in Latin-1 by somebody else. Propagating the read error let one such stranger abort an
/// unrelated run with "failed to read ... as UTF-8", which is an answer no source of a target Ruby
/// version is entitled to give.
///
/// Coming up empty is what `TargetRuby::GemspecFile#version_from_gemspec_file` does with a file it
/// cannot make sense of -- it returns `nil` when `processed_source.valid_syntax?` is false and the
/// walk carries on. The two version *files* are a different story: `RubyVersionFile#find_version`
/// runs `File.read(file).match(pattern)` and `BundlerLockFile#find_version` runs `File.foreach`, so
/// upstream raises `invalid byte sequence in UTF-8` and exits. This project does not reproduce
/// upstream crashes (see the notes at the end of `tests/conformance/known_divergences.yml`), so
/// they get the gemspec's answer instead.
///
/// Giving up on the candidate is chosen over a lossy decode because a file that is not valid UTF-8
/// is not a file a Ruby version can honestly be read out of: the replacement characters a lossy
/// decode inserts would be handed to the Ruby parser as if the gemspec had been written with them.
fn readable_source(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn target_from_gemspec(path: &Path) -> Result<Option<RubyVersion>> {
    let Some(source) = readable_source(path) else {
        return Ok(None);
    };
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .context("failed to initialize the Ruby parser")?;
    let tree = parser
        .parse(&source, None)
        .context("Ruby parser returned no syntax tree for gemspec")?;
    if tree.root_node().has_error() {
        return Ok(None);
    }

    let Some(value) = required_ruby_version_value(tree.root_node(), &source) else {
        return Ok(None);
    };
    let Some(requirements) = literal_requirements(value, &source) else {
        return Ok(None);
    };
    let parsed = requirements
        .iter()
        .map(|requirement| Requirement::parse(requirement))
        .collect::<Option<Vec<_>>>();
    Ok(parsed.and_then(|requirements| {
        KNOWN_RUBIES.iter().copied().find(|version| {
            requirements
                .iter()
                .all(|requirement| requirement.matches(*version))
        })
    }))
}

fn required_ruby_version_value<'tree>(node: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    if node.kind() == "assignment"
        && let Some(left) = node.child_by_field_name("left")
        && left.kind() == "call"
        && let Some(method) = left.child_by_field_name("method")
        && &source[method.byte_range()] == "required_ruby_version"
    {
        return node.child_by_field_name("right");
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| required_ruby_version_value(child, source))
}

fn literal_requirements(node: Node<'_>, source: &str) -> Option<Vec<String>> {
    if contains_kind(node, "interpolation") {
        return None;
    }
    let mut strings = Vec::new();
    collect_string_literals(node, source, &mut strings);
    (!strings.is_empty()).then_some(strings)
}

fn collect_string_literals(node: Node<'_>, source: &str, strings: &mut Vec<String>) {
    if node.kind() == "string" {
        let mut cursor = node.walk();
        let contents = node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "string_content")
            .map(|child| &source[child.byte_range()])
            .collect::<String>();
        strings.push(contents);
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_string_literals(child, source, strings);
    }
}

fn contains_kind(node: Node<'_>, kind: &str) -> bool {
    if node.kind() == kind {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| contains_kind(child, kind))
}

#[derive(Clone, Copy, Debug)]
enum RequirementOperator {
    Equal,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    Pessimistic,
}

#[derive(Clone, Debug)]
struct Requirement {
    operator: RequirementOperator,
    version: Vec<u16>,
}

static REQUIREMENT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?-u:\s)*(~>|>=|<=|>|<|=)?(?-u:\s)*((?-u:\d)+(?:\.(?-u:\d)+){0,2})(?-u:\s)*$")
        .expect("the requirement pattern is a valid constant regex")
});

impl Requirement {
    fn parse(value: &str) -> Option<Self> {
        let captures = REQUIREMENT_PATTERN.captures(value)?;
        let operator = match captures.get(1).map(|capture| capture.as_str()) {
            Some("~>") => RequirementOperator::Pessimistic,
            Some(">=") => RequirementOperator::GreaterOrEqual,
            Some("<=") => RequirementOperator::LessOrEqual,
            Some(">") => RequirementOperator::Greater,
            Some("<") => RequirementOperator::Less,
            Some("=") | None => RequirementOperator::Equal,
            Some(_) => return None,
        };
        let version = captures[2]
            .split('.')
            .map(str::parse)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        Some(Self { operator, version })
    }

    /// The comparison runs in `u32` rather than the `u16` the segments are parsed as, because
    /// `~>` builds its upper bound by adding one to a segment and a gemspec is free to name a
    /// segment at the very top of `u16`: `required_ruby_version = '~> 65535'` made `required[0] +
    /// 1` overflow. A debug build panicked outright ("attempt to add with overflow"), and a release
    /// build wrapped to an upper bound of `[0, 0, 0]` -- below every candidate, so the requirement
    /// matched nothing, `target_from_gemspec` came up empty and `TargetRubyVersion` silently fell
    /// back to the default instead of reporting a version the gemspec could not be satisfied by.
    fn matches(&self, candidate: RubyVersion) -> bool {
        let candidate: [u32; 3] = [candidate.major.into(), candidate.minor.into(), 99];
        let required: [u32; 3] = [
            self.version[0].into(),
            self.version.get(1).copied().unwrap_or(0).into(),
            self.version.get(2).copied().unwrap_or(0).into(),
        ];
        match self.operator {
            RequirementOperator::Equal => candidate == required,
            RequirementOperator::Greater => candidate > required,
            RequirementOperator::GreaterOrEqual => candidate >= required,
            RequirementOperator::Less => candidate < required,
            RequirementOperator::LessOrEqual => candidate <= required,
            RequirementOperator::Pessimistic => {
                let upper = if self.version.len() <= 2 {
                    [required[0] + 1, 0, 0]
                } else {
                    [required[0], required[1] + 1, 0]
                };
                candidate >= required && candidate < upper
            }
        }
    }
}

/// `Option` rather than `Result` because there is nothing left here that can fail: an unreadable
/// candidate is answered with "no version in this file", the same as a readable one that names no
/// version. A `Result` that is always `Ok` would only invite a caller to handle an error that
/// cannot arrive.
fn version_file_value(path: &Path, prefix: Option<&str>) -> Option<RubyVersion> {
    let source = readable_source(path)?;
    let value = match prefix {
        None => source.lines().next().map(str::trim),
        Some(prefix) => source.lines().find_map(|line| {
            let line = line.trim();
            line.strip_prefix(prefix).map(|value| {
                value
                    .trim()
                    .trim_matches(|character| matches!(character, '\'' | '"'))
            })
        }),
    };
    value.and_then(RubyVersion::parse)
}

/// `Option` for the same reason as `version_file_value`: an unreadable lockfile is a lockfile with
/// no `RUBY VERSION` section as far as this can tell.
fn target_from_lockfile(path: &Path) -> Option<RubyVersion> {
    let source = readable_source(path)?;
    let mut in_ruby_version = false;
    for line in source.lines() {
        if line.trim() == "RUBY VERSION" {
            in_ruby_version = true;
            continue;
        }
        if in_ruby_version {
            let value = line.trim().strip_prefix("ruby ");
            return value.and_then(RubyVersion::parse);
        }
    }
    None
}

pub(crate) fn validate_supported(version: RubyVersion) -> Result<()> {
    if KNOWN_RUBIES.contains(&version) {
        Ok(())
    } else {
        bail!("unsupported TargetRubyVersion: {version}")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{Requirement, RubyVersion, TargetRubySource, resolve_target_ruby};

    #[test]
    fn resolves_minimum_known_version_from_gemspec_requirements() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("example.gemspec"),
            "Gem::Specification.new do |spec|\n  spec.required_ruby_version = Gem::Requirement.new(['>= 2.6.0', '< 4.0'])\nend\n",
        )
        .unwrap();

        let resolved = resolve_target_ruby(None, directory.path()).unwrap();

        assert_eq!(resolved.version, RubyVersion::new(2, 6));
        assert!(matches!(resolved.source, TargetRubySource::Gemspec(_)));
    }

    #[test]
    fn explicit_configuration_precedes_project_files() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join(".ruby-version"), "2.6.10\n").unwrap();

        let resolved = resolve_target_ruby(Some(RubyVersion::new(3, 3)), directory.path()).unwrap();

        assert_eq!(resolved.version, RubyVersion::new(3, 3));
        assert_eq!(resolved.source, TargetRubySource::Configuration);
    }

    /// `~>` bumps a segment to build its upper bound, and the segment comes from whatever the
    /// gemspec says. At the top of the parsed integer type the bump used to overflow -- a panic in
    /// a debug build, a wrap to an upper bound of zero in a release one. Both spellings of the
    /// failure are the same bug, so the requirement is asked directly as well as through a run.
    #[test]
    fn a_pessimistic_requirement_at_the_top_of_the_range_does_not_overflow() {
        let extreme = Requirement::parse("~> 65535").unwrap();
        assert!(!extreme.matches(RubyVersion::new(3, 1)));
        assert!(!extreme.matches(RubyVersion::new(4, 1)));
        // The same shape one segment down, where the bump has always been in range.
        let extreme = Requirement::parse("~> 3.65535").unwrap();
        assert!(!extreme.matches(RubyVersion::new(3, 1)));

        // An ordinary requirement keeps its meaning: `~> 3.1` is `>= 3.1, < 4.0`, and `~> 3.1.2`
        // is `>= 3.1.2, < 3.2.0`.
        let ordinary = Requirement::parse("~> 3.1").unwrap();
        assert!(!ordinary.matches(RubyVersion::new(3, 0)));
        assert!(ordinary.matches(RubyVersion::new(3, 1)));
        assert!(ordinary.matches(RubyVersion::new(3, 4)));
        assert!(!ordinary.matches(RubyVersion::new(4, 0)));
        let ordinary = Requirement::parse("~> 3.1.2").unwrap();
        assert!(ordinary.matches(RubyVersion::new(3, 1)));
        assert!(!ordinary.matches(RubyVersion::new(3, 2)));
    }

    /// The same requirement through a whole run: the gemspec satisfies no known Ruby, so the
    /// resolution has to walk past it rather than die on it.
    #[test]
    fn an_unsatisfiable_gemspec_requirement_falls_through_to_the_next_source() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("example.gemspec"),
            "Gem::Specification.new do |spec|\n  spec.required_ruby_version = '~> 65535'\nend\n",
        )
        .unwrap();
        fs::write(directory.path().join(".ruby-version"), "3.1.4\n").unwrap();

        let resolved = resolve_target_ruby(None, directory.path()).unwrap();

        assert_eq!(resolved.version, RubyVersion::new(3, 1));
        assert!(matches!(
            resolved.source,
            TargetRubySource::RubyVersionFile(_)
        ));
    }

    /// `find_single_gemspec` climbs to the filesystem root, so it turns up files belonging to
    /// nobody in particular. One of them being unreadable used to abort the run with "failed to
    /// read ... as UTF-8", which meant a stranger's Latin-1 gemspec could stop an unrelated lint.
    #[test]
    fn an_unreadable_gemspec_above_the_project_does_not_abort_resolution() {
        let outer = tempdir().unwrap();
        // Latin-1: a lone 0xe9 is not valid UTF-8.
        fs::write(
            outer.path().join("outer.gemspec"),
            b"Gem::Specification.new do |spec|\n  spec.author = \"Andr\xe9\"\nend\n",
        )
        .unwrap();
        let project = outer.path().join("proj");
        fs::create_dir(&project).unwrap();
        fs::write(project.join(".ruby-version"), "3.1.4\n").unwrap();

        let resolved = resolve_target_ruby(None, &project).unwrap();

        assert_eq!(resolved.version, RubyVersion::new(3, 1));
        assert!(matches!(
            resolved.source,
            TargetRubySource::RubyVersionFile(_)
        ));
    }

    /// The other side of that boundary: a gemspec above the project that *can* be read still
    /// decides the version, so giving up on the unreadable one did not give up on all of them.
    #[test]
    fn a_readable_gemspec_above_the_project_still_decides_the_version() {
        let outer = tempdir().unwrap();
        fs::write(
            outer.path().join("outer.gemspec"),
            "Gem::Specification.new do |spec|\n  spec.required_ruby_version = '>= 3.3'\nend\n",
        )
        .unwrap();
        let project = outer.path().join("proj");
        fs::create_dir(&project).unwrap();
        fs::write(project.join(".ruby-version"), "3.1.4\n").unwrap();

        let resolved = resolve_target_ruby(None, &project).unwrap();

        assert_eq!(resolved.version, RubyVersion::new(3, 3));
        assert!(matches!(resolved.source, TargetRubySource::Gemspec(_)));
    }

    /// `.ruby-version` and `Gemfile.lock` are found by the same upward walk and were read the same
    /// unforgiving way, so they get the same treatment: an unreadable one is a file with no version
    /// in it, and the next source still gets its turn.
    #[test]
    fn an_unreadable_version_file_does_not_abort_resolution() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join(".ruby-version"), b"3.1.\xe9\n").unwrap();
        fs::write(
            directory.path().join("Gemfile.lock"),
            "RUBY VERSION\n   ruby 3.2.2p53\n",
        )
        .unwrap();

        let resolved = resolve_target_ruby(None, directory.path()).unwrap();

        assert_eq!(resolved.version, RubyVersion::new(3, 2));
        assert!(matches!(resolved.source, TargetRubySource::BundlerLock(_)));
    }
}
