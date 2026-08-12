department_rules! {
    "Layout";
    empty_line_after_magic_comment => ("EmptyLineAfterMagicComment", Convention),
    argument_alignment => ("ArgumentAlignment", Convention),
    array_alignment => ("ArrayAlignment", Convention),
    empty_line_after_guard_clause => ("EmptyLineAfterGuardClause", Convention),
    empty_lines_around_access_modifier => ("EmptyLinesAroundAccessModifier", Convention),
    end_of_line => ("EndOfLine", Convention),
    first_array_element_indentation => ("FirstArrayElementIndentation", Convention),
    first_hash_element_indentation => ("FirstHashElementIndentation", Convention),
    hash_alignment => ("HashAlignment", Convention),
    indentation_consistency => ("IndentationConsistency", Convention),
    indentation_width => ("IndentationWidth", Convention),
    line_length => ("LineLength", Convention),
    space_after_comma => ("SpaceAfterComma", Convention),
    space_around_operators => ("SpaceAroundOperators", Convention),
    space_inside_array_literal_brackets => ("SpaceInsideArrayLiteralBrackets", Convention),
    space_inside_parens => ("SpaceInsideParens", Convention),
    space_inside_percent_literal_delimiters => ("SpaceInsidePercentLiteralDelimiters", Convention),
    trailing_empty_lines => ("TrailingEmptyLines", Convention),
    trailing_whitespace => ("TrailingWhitespace", Convention),
}

mod support;
