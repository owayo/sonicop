mod comments;
mod conditional;
mod format_sequences;
mod line_length_help;
mod literal;
mod nodes;
mod parameters;
mod percent;
mod percent_array;
mod trailing_comma;

department_rules! {
    "Style";
    alias => ("Alias", Convention),
    array_join => ("ArrayJoin", Convention),
    bare_percent_literals => ("BarePercentLiterals", Convention),
    block_delimiters => ("BlockDelimiters", Convention),
    case_equality => ("CaseEquality", Convention),
    class_and_module_children => ("ClassAndModuleChildren", Convention),
    character_literal => ("CharacterLiteral", Convention),
    class_check => ("ClassCheck", Convention),
    class_vars => ("ClassVars", Convention),
    commented_keyword => ("CommentedKeyword", Convention),
    documentation => ("Documentation", Convention),
    empty_method => ("EmptyMethod", Convention),
    format_string => ("FormatString", Convention),
    format_string_token => ("FormatStringToken", Convention),
    frozen_string_literal_comment => ("FrozenStringLiteralComment", Convention),
    global_std_stream => ("GlobalStdStream", Convention),
    global_vars => ("GlobalVars", Convention),
    guard_clause => ("GuardClause", Convention),
    hash_as_last_array_item => ("HashAsLastArrayItem", Convention),
    hash_syntax => ("HashSyntax", Convention),
    if_unless_modifier => ("IfUnlessModifier", Convention),
    lambda => ("Lambda", Convention),
    lambda_call => ("LambdaCall", Convention),
    missing_respond_to_missing => ("MissingRespondToMissing", Convention),
    negated_if => ("NegatedIf", Convention),
    next => ("Next", Convention),
    numeric_literal_prefix => ("NumericLiteralPrefix", Convention),
    numeric_predicate => ("NumericPredicate", Convention),
    numeric_literals => ("NumericLiterals", Convention),
    optional_arguments => ("OptionalArguments", Convention),
    optional_boolean_parameter => ("OptionalBooleanParameter", Convention),
    parallel_assignment => ("ParallelAssignment", Convention),
    percent_literal_delimiters => ("PercentLiteralDelimiters", Convention),
    percent_q_literals => ("PercentQLiterals", Convention),
    perl_backrefs => ("PerlBackrefs", Convention),
    preferred_hash_methods => ("PreferredHashMethods", Convention),
    proc => ("Proc", Convention),
    raise_args => ("RaiseArgs", Convention),
    redundant_capital_w => ("RedundantCapitalW", Convention),
    redundant_exception => ("RedundantException", Convention),
    redundant_percent_q => ("RedundantPercentQ", Convention),
    redundant_return => ("RedundantReturn", Convention),
    regexp_literal => ("RegexpLiteral", Convention),
    rescue_standard_error => ("RescueStandardError", Convention),
    semicolon => ("Semicolon", Convention),
    single_line_methods => ("SingleLineMethods", Convention),
    stabby_lambda_parentheses => ("StabbyLambdaParentheses", Convention),
    stderr_puts => ("StderrPuts", Convention),
    string_concatenation => ("StringConcatenation", Convention),
    string_literals => ("StringLiterals", Convention),
    string_literals_in_interpolation => ("StringLiteralsInInterpolation", Convention),
    symbol_array => ("SymbolArray", Convention),
    symbol_literal => ("SymbolLiteral", Convention),
    trailing_comma_in_arguments => ("TrailingCommaInArguments", Convention),
    trailing_comma_in_array_literal => ("TrailingCommaInArrayLiteral", Convention),
    trailing_comma_in_hash_literal => ("TrailingCommaInHashLiteral", Convention),
    variable_interpolation => ("VariableInterpolation", Convention),
    when_then => ("WhenThen", Convention),
    word_array => ("WordArray", Convention),
    zero_length_predicate => ("ZeroLengthPredicate", Convention),
}
