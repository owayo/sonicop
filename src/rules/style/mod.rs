mod comments;
mod conditional;
mod format_string;
mod line_length_help;
mod literal;
mod nodes;
mod percent;
mod percent_array;
mod trailing_comma;

department_rules! {
    "Style";
    alias => ("Alias", Convention),
    block_delimiters => ("BlockDelimiters", Convention),
    case_equality => ("CaseEquality", Convention),
    class_and_module_children => ("ClassAndModuleChildren", Convention),
    commented_keyword => ("CommentedKeyword", Convention),
    documentation => ("Documentation", Convention),
    format_string_token => ("FormatStringToken", Convention),
    frozen_string_literal_comment => ("FrozenStringLiteralComment", Convention),
    global_vars => ("GlobalVars", Convention),
    guard_clause => ("GuardClause", Convention),
    hash_syntax => ("HashSyntax", Convention),
    if_unless_modifier => ("IfUnlessModifier", Convention),
    next => ("Next", Convention),
    numeric_literals => ("NumericLiterals", Convention),
    optional_arguments => ("OptionalArguments", Convention),
    parallel_assignment => ("ParallelAssignment", Convention),
    percent_literal_delimiters => ("PercentLiteralDelimiters", Convention),
    redundant_return => ("RedundantReturn", Convention),
    regexp_literal => ("RegexpLiteral", Convention),
    semicolon => ("Semicolon", Convention),
    single_line_methods => ("SingleLineMethods", Convention),
    string_literals => ("StringLiterals", Convention),
    string_literals_in_interpolation => ("StringLiteralsInInterpolation", Convention),
    symbol_array => ("SymbolArray", Convention),
    trailing_comma_in_arguments => ("TrailingCommaInArguments", Convention),
    trailing_comma_in_array_literal => ("TrailingCommaInArrayLiteral", Convention),
    trailing_comma_in_hash_literal => ("TrailingCommaInHashLiteral", Convention),
    word_array => ("WordArray", Convention),
}
