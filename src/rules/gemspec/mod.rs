department_rules! {
    "Gemspec";
    // `Gemspec/DeprecatedAttributeAssignment`, `DuplicatedAssignment`, `RequireMFA`,
    // `RequiredRubyVersion` and `RubyVersionGlobalsUsage` all carry `Severity: warning` in
    // `config/default.yml`; the rest have no override and so inherit `Base#default_severity`, which
    // is `:convention`.
    add_runtime_dependency => ("AddRuntimeDependency", Convention),
    attribute_assignment => ("AttributeAssignment", Convention),
    deprecated_attribute_assignment => ("DeprecatedAttributeAssignment", Warning),
    development_dependencies => ("DevelopmentDependencies", Convention),
    duplicated_assignment => ("DuplicatedAssignment", Warning),
    ordered_dependencies => ("OrderedDependencies", Convention),
    required_ruby_version => ("RequiredRubyVersion", Warning),
    ruby_version_globals_usage => ("RubyVersionGlobalsUsage", Warning),
}

mod support;
