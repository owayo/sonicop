use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use super::support::{ruby_regex, ruby_regex_to_s};
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

/// `SNAKE_CASE`, whose POSIX class is Unicode-aware in Ruby. A dot is allowed because only the
/// last extension is stripped before the name is judged.
static SNAKE_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[0-9\p{Lowercase}_.?!]+$").unwrap());

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let path = context.source.path();
    if context.config.allowed_camel_case_file(path) {
        return;
    }
    let Some(basename) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let pattern: Option<&Regex> = context
        .setting::<serde_yaml_ng::Value>("Regex")
        .as_ref()
        .and_then(ruby_regex);
    // `for_bad_filename` branches both ways: a name that *is* good is where the class-and-module
    // checks run, and a name that is not is where the naming message comes from. Returning early
    // on a good name skipped `ExpectMatchingDefinition` entirely.
    if filename_good(basename, pattern.unwrap_or(&SNAKE_CASE)) {
        if let Some(message) = class_and_module_naming(context, path, basename) {
            offenses.push(context.offense(message, 0..0));
        }
        return;
    }
    let ignore_scripts: bool = context.setting("IgnoreExecutableScripts").unwrap_or(true);
    if ignore_scripts && context.source.text().starts_with("#!") {
        return;
    }
    let configured = context
        .setting::<serde_yaml_ng::Value>("Regex")
        .as_ref()
        .and_then(ruby_regex_to_s);
    let message = match configured {
        Some(source) => format!("`{basename}` should match `{source}`."),
        None => format!("The name of this source file (`{basename}`) should use snake_case."),
    };
    // `add_global_offense` places the offense nowhere in particular, which the formatters render
    // as the very start of the file.
    offenses.push(context.offense(message, 0..0));
}

/// `perform_class_and_module_naming_checks`: with `ExpectMatchingDefinition` on, a well-named file
/// still has to define the constant its name spells.
fn class_and_module_naming(
    context: &RuleContext<'_>,
    path: &std::path::Path,
    basename: &str,
) -> Option<String> {
    if !context
        .setting::<bool>("ExpectMatchingDefinition")
        .unwrap_or(false)
    {
        return None;
    }
    let hierarchy: bool = context
        .setting("CheckDefinitionPathHierarchy")
        .unwrap_or(true);
    let acronyms: Vec<String> = context.setting("AllowedAcronyms").unwrap_or_default();
    // The two arms differ only in what the namespace is read off: the whole path, or the base name
    // alone. The message quotes whichever was used.
    let (namespace, quoted) = match hierarchy {
        true => (
            to_namespace(context, path),
            path.to_string_lossy().into_owned(),
        ),
        false => (
            to_namespace(context, std::path::Path::new(basename)),
            basename.to_owned(),
        ),
    };
    if finds_definition(context, &namespace, &acronyms) {
        return None;
    }
    let _ = quoted;
    Some(format!(
        "`{basename}` should define a class or module called `{}`.",
        namespace.join("::")
    ))
}

/// `to_namespace`: the path split into constant names, starting at the innermost directory the
/// configuration calls a root. With no root in the path only the file's own name is used.
fn to_namespace(context: &RuleContext<'_>, path: &std::path::Path) -> Vec<String> {
    let roots: Vec<String> = context
        .setting("CheckDefinitionPathHierarchyRoots")
        .unwrap_or_else(|| ["lib", "spec", "test", "src"].map(str::to_owned).to_vec());
    let components: Vec<String> = path
        .iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect();
    let start = components
        .iter()
        .rposition(|part| roots.contains(part))
        .map(|index| index + 1);
    match start {
        Some(index) if index < components.len() => components[index..]
            .iter()
            .map(|part| module_name(part))
            .collect(),
        _ => components
            .last()
            .map(|part| vec![module_name(part)])
            .unwrap_or_default(),
    }
}

/// `to_module_name`: the extension goes, the underscores split words, each word is capitalised.
fn module_name(basename: &str) -> String {
    let stem = basename.split_once('.').map_or(basename, |(head, _)| head);
    stem.split('_')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `find_class_or_module`: whether the file defines the innermost name, nested in the rest.
///
/// The nesting is checked by name rather than by walking upwards the way upstream does -- the
/// namespaces a file writes are what `class A::B` and `module A; module B` both spell out.
fn finds_definition(context: &RuleContext<'_>, namespace: &[String], acronyms: &[String]) -> bool {
    let Some((name, outer)) = namespace.split_last() else {
        return true;
    };
    context
        .nodes_of_any(&["class", "module", "assignment"])
        .any(|node| {
            let Some(defined) = defined_constant(context, node) else {
                return false;
            };
            let Some((last, prefix)) = defined.split_last() else {
                return false;
            };
            if last != name && !matches_acronym(name, last, acronyms) {
                return false;
            }
            outer.is_empty() || enclosing_names(context, node, prefix).ends_with(outer)
        })
}

/// The constant a `class` / `module` / `Struct.new` assignment defines, as its written path.
fn defined_constant(context: &RuleContext<'_>, node: Node<'_>) -> Option<Vec<String>> {
    let named = match node.kind_str() {
        "class" | "module" => node.field("name")?,
        // `Foo = Struct.new(...)` defines `Foo` the same way.
        "assignment" => {
            let left = node.field("left")?;
            let right = node.field("right")?;
            let struct_new = right.kind_str() == "call"
                && right
                    .field("receiver")
                    .is_some_and(|receiver| context.source.node_text(receiver) == "Struct")
                && right
                    .field("method")
                    .is_some_and(|method| context.source.node_text(method) == "new");
            if !struct_new {
                return None;
            }
            left
        }
        _ => return None,
    };
    let text = context.source.node_text(named);
    Some(
        text.trim_start_matches("::")
            .split("::")
            .map(str::to_owned)
            .collect(),
    )
}

/// The names a definition is written inside, innermost last, with what its own path already spells.
fn enclosing_names(context: &RuleContext<'_>, node: Node<'_>, prefix: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if matches!(ancestor.kind_str(), "class" | "module")
            && let Some(defined) = defined_constant(context, ancestor)
        {
            for part in defined.into_iter().rev() {
                names.push(part);
            }
        }
        current = ancestor.parent();
    }
    names.reverse();
    names.extend(prefix.iter().cloned());
    names
}

/// `match_acronym?`: an acronym listed in `AllowedAcronyms` may be written in full caps.
fn matches_acronym(expected: &str, name: &str, acronyms: &[String]) -> bool {
    let folded = acronyms.iter().fold(name.to_owned(), |result, acronym| {
        let mut capitalized = acronym.to_lowercase();
        if let Some(first) = capitalized.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        result.replace(acronym.as_str(), &capitalized)
    });
    expected == folded
}

/// `filename_good?`: the leading dot and the last extension are dropped, the one `+` an Action Pack
/// variant name carries becomes an underscore, and what is left has to match the pattern.
fn filename_good(basename: &str, pattern: &Regex) -> bool {
    let stem = basename.strip_prefix('.').unwrap_or(basename);
    let stem = match stem.rfind('.') {
        Some(dot) => &stem[..dot],
        None => stem,
    };
    let stem = stem.replacen('+', "_", 1);
    pattern.is_match(&stem)
}
