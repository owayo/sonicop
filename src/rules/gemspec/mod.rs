department_rules! {
    "Gemspec";
    // `Gemspec/DuplicatedAssignment`, `RequiredRubyVersion` and `RubyVersionGlobalsUsage` all carry
    // `Severity: warning` in `config/default.yml`; `OrderedDependencies` has no override and so
    // inherits `Base#default_severity`, which is `:convention`.
    duplicated_assignment => ("DuplicatedAssignment", Warning),
    ordered_dependencies => ("OrderedDependencies", Convention),
    required_ruby_version => ("RequiredRubyVersion", Warning),
    ruby_version_globals_usage => ("RubyVersionGlobalsUsage", Warning),
}

mod support;
