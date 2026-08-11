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
        && let Some(version) = version_file_value(&path, None)?
    {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::RubyVersionFile(path),
        });
    }
    if let Some(path) = find_upwards(base_directory, "mise.toml")
        && let Some(version) = version_file_value(&path, Some("ruby ="))?
    {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::MiseToml(path),
        });
    }
    if let Some(path) = find_upwards(base_directory, ".tool-versions")
        && let Some(version) = version_file_value(&path, Some("ruby "))?
    {
        return Ok(ResolvedTargetRuby {
            version,
            source: TargetRubySource::ToolVersions(path),
        });
    }
    for filename in ["Gemfile.lock", "gems.locked"] {
        if let Some(path) = find_upwards(base_directory, filename)
            && let Some(version) = target_from_lockfile(&path)?
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

fn target_from_gemspec(path: &Path) -> Result<Option<RubyVersion>> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read {} as UTF-8", path.display()))?;
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
    Regex::new(r"^\s*(~>|>=|<=|>|<|=)?\s*(\d+(?:\.\d+){0,2})\s*$")
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

    fn matches(&self, candidate: RubyVersion) -> bool {
        let candidate = [candidate.major, candidate.minor, 99];
        let required = [
            self.version[0],
            self.version.get(1).copied().unwrap_or(0),
            self.version.get(2).copied().unwrap_or(0),
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

fn version_file_value(path: &Path, prefix: Option<&str>) -> Result<Option<RubyVersion>> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read {} as UTF-8", path.display()))?;
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
    Ok(value.and_then(RubyVersion::parse))
}

fn target_from_lockfile(path: &Path) -> Result<Option<RubyVersion>> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read {} as UTF-8", path.display()))?;
    let mut in_ruby_version = false;
    for line in source.lines() {
        if line.trim() == "RUBY VERSION" {
            in_ruby_version = true;
            continue;
        }
        if in_ruby_version {
            let value = line.trim().strip_prefix("ruby ");
            return Ok(value.and_then(RubyVersion::parse));
        }
    }
    Ok(None)
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

    use super::{RubyVersion, TargetRubySource, resolve_target_ruby};

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
}
