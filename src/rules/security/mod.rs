department_rules! {
    "Security";
    // `Security/Eval` inherits `Base#default_severity`, which is `:convention` for every
    // department except `Lint`, and `config/default.yml` gives it no `Severity:` override.
    compound_hash => ("CompoundHash", Convention),
    eval => ("Eval", Convention),
    io_methods => ("IoMethods", Convention),
    json_load => ("JSONLoad", Convention),
    marshal_load => ("MarshalLoad", Convention),
    open => ("Open", Convention),
    yaml_load => ("YAMLLoad", Convention),
}
