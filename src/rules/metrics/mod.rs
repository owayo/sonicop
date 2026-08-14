department_rules! {
    "Metrics";
    abc_size => ("AbcSize", Convention),
    block_length => ("BlockLength", Convention),
    block_nesting => ("BlockNesting", Convention),
    class_length => ("ClassLength", Convention),
    collection_literal_length => ("CollectionLiteralLength", Convention),
    cyclomatic_complexity => ("CyclomaticComplexity", Convention),
    method_length => ("MethodLength", Convention),
    module_length => ("ModuleLength", Convention),
    parameter_lists => ("ParameterLists", Convention),
    perceived_complexity => ("PerceivedComplexity", Convention),
}

mod complexity;
/// Reachable from the shared `RuleContext`: the recovery is the same for every cop that asks,
/// so the context runs it once per file.
pub(in crate::rules) mod fragments;
/// Reachable from the shared `RuleContext` for the same reason as `fragments`.
pub(in crate::rules) mod locals;
mod support;
