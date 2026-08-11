department_rules! {
    "Metrics";
    abc_size => ("AbcSize", Convention),
    block_length => ("BlockLength", Convention),
    block_nesting => ("BlockNesting", Convention),
    class_length => ("ClassLength", Convention),
    cyclomatic_complexity => ("CyclomaticComplexity", Convention),
    method_length => ("MethodLength", Convention),
    module_length => ("ModuleLength", Convention),
    parameter_lists => ("ParameterLists", Convention),
    perceived_complexity => ("PerceivedComplexity", Convention),
}

mod complexity;
mod fragments;
mod locals;
mod support;
