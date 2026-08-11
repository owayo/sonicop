department_rules! {
    "Migration";
    // `Migration/DepartmentName` inherits `Base#default_severity`, which is `:convention` for every
    // department except `Lint`, and `config/default.yml` gives it no `Severity:` override.
    department_name => ("DepartmentName", Convention),
}
