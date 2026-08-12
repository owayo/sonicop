mod comments;
mod format_string;
mod literal;
mod nodes;
mod percent;
mod percent_array;
mod trailing_comma;

department_rules! {
    "Style";
    alias => ("Alias", Convention),
    case_equality => ("CaseEquality", Convention),
    class_and_module_children => ("ClassAndModuleChildren", Convention),
    documentation => ("Documentation", Convention),
    format_string_token => ("FormatStringToken", Convention),
    frozen_string_literal_comment => ("FrozenStringLiteralComment", Convention),
    hash_syntax => ("HashSyntax", Convention),
    numeric_literals => ("NumericLiterals", Convention),
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
