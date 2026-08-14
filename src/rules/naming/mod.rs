department_rules! {
    "Naming";
    accessor_method_name => ("AccessorMethodName", Convention),
    ascii_identifiers => ("AsciiIdentifiers", Convention),
    binary_operator_parameter_name => ("BinaryOperatorParameterName", Convention),
    block_forwarding => ("BlockForwarding", Convention),
    block_parameter_name => ("BlockParameterName", Convention),
    class_and_module_camel_case => ("ClassAndModuleCamelCase", Convention),
    constant_name => ("ConstantName", Convention),
    file_name => ("FileName", Convention),
    heredoc_delimiter_case => ("HeredocDelimiterCase", Convention),
    heredoc_delimiter_naming => ("HeredocDelimiterNaming", Convention),
    method_name => ("MethodName", Convention),
    memoized_instance_variable_name => ("MemoizedInstanceVariableName", Convention),
    method_parameter_name => ("MethodParameterName", Convention),
    predicate_method => ("PredicateMethod", Convention),
    predicate_prefix => ("PredicatePrefix", Convention),
    rescued_exceptions_variable_name => ("RescuedExceptionsVariableName", Convention),
    variable_name => ("VariableName", Convention),
    variable_number => ("VariableNumber", Convention),
}

/// Also read by `Lint/UnreachableLoop`, whose `AllowedPatterns` default is a `!ruby/regexp`.
pub(in crate::rules) mod support;
mod uncommunicative;
