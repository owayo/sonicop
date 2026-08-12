mod block_args;
mod comments;
mod conditional;
mod format_string;
mod line_length_help;
mod literal;
mod nodes;
mod percent;
mod percent_array;
mod ranges;
mod trailing_comma;

department_rules! {
    "Style";
    alias => ("Alias", Convention),
    begin_block => ("BeginBlock", Convention),
    block_comments => ("BlockComments", Convention),
    block_delimiters => ("BlockDelimiters", Convention),
    case_equality => ("CaseEquality", Convention),
    class_and_module_children => ("ClassAndModuleChildren", Convention),
    class_methods => ("ClassMethods", Convention),
    colon_method_call => ("ColonMethodCall", Convention),
    colon_method_definition => ("ColonMethodDefinition", Convention),
    commented_keyword => ("CommentedKeyword", Convention),
    def_with_parentheses => ("DefWithParentheses", Convention),
    documentation => ("Documentation", Convention),
    each_for_simple_loop => ("EachForSimpleLoop", Convention),
    empty_block_parameter => ("EmptyBlockParameter", Convention),
    empty_lambda_parameter => ("EmptyLambdaParameter", Convention),
    empty_method => ("EmptyMethod", Convention),
    end_block => ("EndBlock", Convention),
    format_string_token => ("FormatStringToken", Convention),
    frozen_string_literal_comment => ("FrozenStringLiteralComment", Convention),
    global_std_stream => ("GlobalStdStream", Convention),
    global_vars => ("GlobalVars", Convention),
    guard_clause => ("GuardClause", Convention),
    hash_as_last_array_item => ("HashAsLastArrayItem", Convention),
    hash_syntax => ("HashSyntax", Convention),
    if_unless_modifier => ("IfUnlessModifier", Convention),
    if_unless_modifier_of_if_unless => ("IfUnlessModifierOfIfUnless", Convention),
    lambda => ("Lambda", Convention),
    min_max => ("MinMax", Convention),
    multiline_if_then => ("MultilineIfThen", Convention),
    multiline_memoization => ("MultilineMemoization", Convention),
    multiline_when_then => ("MultilineWhenThen", Convention),
    negated_unless => ("NegatedUnless", Convention),
    negated_while => ("NegatedWhile", Convention),
    next => ("Next", Convention),
    not => ("Not", Convention),
    numeric_literal_prefix => ("NumericLiteralPrefix", Convention),
    numeric_predicate => ("NumericPredicate", Convention),
    numeric_literals => ("NumericLiterals", Convention),
    optional_arguments => ("OptionalArguments", Convention),
    parallel_assignment => ("ParallelAssignment", Convention),
    percent_literal_delimiters => ("PercentLiteralDelimiters", Convention),
    perl_backrefs => ("PerlBackrefs", Convention),
    preferred_hash_methods => ("PreferredHashMethods", Convention),
    redundant_return => ("RedundantReturn", Convention),
    regexp_literal => ("RegexpLiteral", Convention),
    rescue_standard_error => ("RescueStandardError", Convention),
    semicolon => ("Semicolon", Convention),
    single_line_methods => ("SingleLineMethods", Convention),
    string_concatenation => ("StringConcatenation", Convention),
    string_literals => ("StringLiterals", Convention),
    string_literals_in_interpolation => ("StringLiteralsInInterpolation", Convention),
    symbol_array => ("SymbolArray", Convention),
    trailing_comma_in_arguments => ("TrailingCommaInArguments", Convention),
    trailing_comma_in_array_literal => ("TrailingCommaInArrayLiteral", Convention),
    trailing_comma_in_hash_literal => ("TrailingCommaInHashLiteral", Convention),
    unless_else => ("UnlessElse", Convention),
    while_until_do => ("WhileUntilDo", Convention),
    word_array => ("WordArray", Convention),
}
