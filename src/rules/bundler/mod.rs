department_rules! {
    "Bundler";
    // `config/default.yml` gives `DuplicatedGem`, `DuplicatedGroup` and `InsecureProtocolSource`
    // `Severity: warning`; `GemFilename` and `OrderedGems` have no override and so inherit
    // `Base#default_severity`, which is `:convention`.
    duplicated_gem => ("DuplicatedGem", Warning),
    duplicated_group => ("DuplicatedGroup", Warning),
    gem_filename => ("GemFilename", Convention),
    gem_version => ("GemVersion", Convention),
    insecure_protocol_source => ("InsecureProtocolSource", Warning),
    ordered_gems => ("OrderedGems", Convention),
}

mod support;
