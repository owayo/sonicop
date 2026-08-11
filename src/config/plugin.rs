use std::collections::HashSet;

use serde_yaml_ng::Value;

use crate::cop_name;

pub(super) fn configured_plugin_departments(config: &Value) -> HashSet<String> {
    let Some(mapping) = config.as_mapping() else {
        return HashSet::new();
    };
    ["plugins", "require"]
        .into_iter()
        .filter_map(|key| mapping.get(key))
        .flat_map(configured_plugin_names)
        .flat_map(|plugin| plugin_departments(&plugin))
        .collect()
}

/// A plugin owns every department nested below the one it declares, because
/// `rubocop-i18n` ships `I18n/GetText/*` and pre-3.0 `rubocop-rspec` shipped
/// `RSpec/Rails/*`.
pub(super) fn belongs_to_plugin(name: &str, departments: &HashSet<String>) -> bool {
    cop_name::department_ancestors(name).any(|candidate| departments.contains(candidate))
}

fn configured_plugin_names(value: &Value) -> Vec<String> {
    match value {
        Value::String(plugin) => vec![plugin.clone()],
        Value::Sequence(plugins) => plugins.iter().flat_map(configured_plugin_names).collect(),
        Value::Mapping(plugins) => plugins
            .keys()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Gem name to the departments it ships, for plugins RuboCop documents as
/// official or widely used.
///
/// A table is required because capitalizing each `-`/`_` segment guesses
/// `Rspec`/`Graphql`/`Github` instead of the real `RSpec`/`GraphQL`/`GitHub`, and
/// because `cookstyle` carries no `rubocop-` prefix to strip at all. Departments
/// are a flat namespace upstream, so one gem may ship several *sibling*
/// departments — `rubocop-sketchup` ships five that share no common parent — which
/// is why each entry holds a slice. Entries naming a parent namespace such as
/// `Chef` or `I18n` reach their nested departments through `belongs_to_plugin`.
const PLUGIN_DEPARTMENTS: &[(&str, &[&str])] = &[
    ("cookstyle", &["Chef"]),
    ("rubocop-capybara", &["Capybara"]),
    ("rubocop-factory_bot", &["FactoryBot"]),
    ("rubocop-github", &["GitHub"]),
    ("rubocop-graphql", &["GraphQL"]),
    ("rubocop-i18n", &["I18n"]),
    ("rubocop-minitest", &["Minitest"]),
    ("rubocop-packaging", &["Packaging"]),
    ("rubocop-performance", &["Performance"]),
    ("rubocop-rails", &["Rails"]),
    ("rubocop-rake", &["Rake"]),
    ("rubocop-rspec", &["RSpec"]),
    ("rubocop-rspec_rails", &["RSpecRails"]),
    ("rubocop-sequel", &["Sequel"]),
    (
        "rubocop-sketchup",
        &[
            "SketchupBugs",
            "SketchupDeprecations",
            "SketchupPerformance",
            "SketchupRequirements",
            "SketchupSuggestions",
        ],
    ),
    ("rubocop-sorbet", &["Sorbet"]),
    ("rubocop-thread_safety", &["ThreadSafety"]),
];

fn plugin_departments(plugin: &str) -> Vec<String> {
    // Each fallback must apply to the previous step's result: chaining
    // `unwrap_or(plugin)` restores the whole string and loses the directory strip.
    let stem = plugin.rsplit('/').next().unwrap_or(plugin);
    let stem = stem.strip_suffix(".rb").unwrap_or(stem);
    if let Some((_, departments)) = PLUGIN_DEPARTMENTS.iter().find(|(gem, _)| *gem == stem) {
        return departments.iter().map(|name| (*name).to_owned()).collect();
    }
    let Some(extension) = stem.strip_prefix("rubocop-") else {
        return Vec::new();
    };
    let department = extension
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect::<String>();
    if department.is_empty() {
        return Vec::new();
    }
    vec![department]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{belongs_to_plugin, plugin_departments};
    use crate::config::Config;

    fn departments(plugin: &str) -> Vec<String> {
        plugin_departments(plugin)
    }

    #[test]
    fn resolves_departments_for_known_plugins() {
        let cases: &[(&str, &[&str])] = &[
            ("rubocop-rspec", &["RSpec"]),
            ("rubocop-rspec_rails", &["RSpecRails"]),
            ("rubocop-graphql", &["GraphQL"]),
            ("rubocop-github", &["GitHub"]),
            ("rubocop-i18n", &["I18n"]),
            ("rubocop-factory_bot", &["FactoryBot"]),
            ("rubocop-thread_safety", &["ThreadSafety"]),
            ("rubocop-performance", &["Performance"]),
            ("rubocop-rails", &["Rails"]),
            // One gem can ship several sibling departments sharing no parent.
            (
                "rubocop-sketchup",
                &[
                    "SketchupBugs",
                    "SketchupDeprecations",
                    "SketchupPerformance",
                    "SketchupRequirements",
                    "SketchupSuggestions",
                ],
            ),
            // No `rubocop-` prefix exists to strip, so only the table can resolve it.
            ("cookstyle", &["Chef"]),
            // Uncatalogued plugins keep falling back to the capitalization guess.
            ("rubocop-my_house_style", &["MyHouseStyle"]),
            ("../my/custom/file.rb", &[]),
            ("rubocop", &[]),
        ];
        for (plugin, expected) in cases {
            let resolved = departments(plugin);
            let resolved: Vec<&str> = resolved.iter().map(String::as_str).collect();
            assert_eq!(resolved, *expected, "plugin: {plugin}");
        }
    }

    #[test]
    fn resolves_departments_for_plugins_written_as_paths() {
        let cases: &[(&str, &[&str])] = &[
            ("gems/rubocop-rspec", &["RSpec"]),
            ("vendor/bundle/rubocop-performance", &["Performance"]),
            ("./rubocop-rails", &["Rails"]),
            ("rubocop-rspec.rb", &["RSpec"]),
            ("./gems/rubocop-graphql.rb", &["GraphQL"]),
            ("/abs/path/rubocop-minitest.rb", &["Minitest"]),
            ("vendor/cookstyle.rb", &["Chef"]),
        ];
        for (plugin, expected) in cases {
            let resolved = departments(plugin);
            let resolved: Vec<&str> = resolved.iter().map(String::as_str).collect();
            assert_eq!(resolved, *expected, "plugin: {plugin}");
        }
    }

    #[test]
    fn plugin_ownership_covers_nested_departments() {
        let departments = ["RSpec".to_owned(), "I18n".to_owned()]
            .into_iter()
            .collect();
        assert!(belongs_to_plugin("RSpec/ExampleLength", &departments));
        assert!(belongs_to_plugin("RSpec/Rails/HttpStatus", &departments));
        assert!(belongs_to_plugin(
            "I18n/GetText/DecorateString",
            &departments
        ));
        assert!(!belongs_to_plugin("Style/HashSyntax", &departments));
        // A sibling department is not owned by a prefix that merely looks similar.
        assert!(!belongs_to_plugin("RSpecRails/HttpStatus", &departments));
    }

    #[test]
    fn recognizes_cops_from_multi_department_and_namespaced_plugins() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "require:\n  - rubocop-sketchup\n  - cookstyle\nSketchupRequirements/GlobalMethods:\n  Enabled: true\nSketchupBugs/UniformScaleReference:\n  Enabled: true\nChef/Correctness/ServiceResource:\n  Enabled: false\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        assert!(
            config.unrecognized_cop_names().is_empty(),
            "unexpected: {:?}",
            config.unrecognized_cop_names()
        );
    }

    #[test]
    fn recognizes_rspec_cops_declared_through_the_rspec_plugin() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "plugins:\n  - rubocop-rspec\n  - rubocop-graphql\nRSpec/ExampleLength:\n  Max: 20\nGraphQL/ObjectDescription:\n  Enabled: false\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        assert!(
            config.unrecognized_cop_names().is_empty(),
            "unexpected: {:?}",
            config.unrecognized_cop_names()
        );
    }

    #[test]
    fn recognizes_configured_cops_from_declared_plugins() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("Gemfile"), "").unwrap();
        fs::write(
            directory.path().join(".rubocop.yml"),
            "plugins:\n  - rubocop-performance\nPerformance/MapCompact:\n  Enabled: true\n",
        )
        .unwrap();

        let config = Config::load(None, directory.path()).unwrap();

        assert!(config.unrecognized_cop_names().is_empty());
        assert!(
            config
                .known_cop_names()
                .any(|name| name == "Performance/MapCompact")
        );
        assert!(config.rule_enabled("Performance/MapCompact"));
    }
}
