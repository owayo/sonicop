/// Shared analyses that belong to no single cop: how RuboCop's `SendNode` reads an access
/// modifier, what a `rescue` clause covers, whether a bare name reads a local variable, and
/// RuboCop's local variable tracking.
mod access_modifier;
mod flow;
mod literals;
mod locals;
mod node_equality;
mod rescue_clause;
mod statements;
mod variable_force;

department_rules! {
    "Lint";
    ambiguous_block_association => ("AmbiguousBlockAssociation", Warning),
    assignment_in_condition => ("AssignmentInCondition", Warning),
    big_decimal_new => ("BigDecimalNew", Warning),
    binary_operator_with_identical_operands => ("BinaryOperatorWithIdenticalOperands", Warning),
    boolean_symbol => ("BooleanSymbol", Warning),
    constant_definition_in_block => ("ConstantDefinitionInBlock", Warning),
    deprecated_class_methods => ("DeprecatedClassMethods", Warning),
    disjunctive_assignment_in_constructor => ("DisjunctiveAssignmentInConstructor", Warning),
    duplicate_case_condition => ("DuplicateCaseCondition", Warning),
    duplicate_elsif_condition => ("DuplicateElsifCondition", Warning),
    duplicate_hash_key => ("DuplicateHashKey", Warning),
    duplicate_methods => ("DuplicateMethods", Warning),
    duplicate_require => ("DuplicateRequire", Warning),
    duplicate_rescue_exception => ("DuplicateRescueException", Warning),
    each_with_object_argument => ("EachWithObjectArgument", Warning),
    empty_ensure => ("EmptyEnsure", Warning),
    empty_file => ("EmptyFile", Warning),
    empty_interpolation => ("EmptyInterpolation", Warning),
    empty_when => ("EmptyWhen", Warning),
    ensure_return => ("EnsureReturn", Warning),
    float_comparison => ("FloatComparison", Warning),
    hash_compare_by_identity => ("HashCompareByIdentity", Warning),
    identity_comparison => ("IdentityComparison", Warning),
    ineffective_access_modifier => ("IneffectiveAccessModifier", Warning),
    inherit_exception => ("InheritException", Warning),
    interpolation_check => ("InterpolationCheck", Warning),
    literal_in_interpolation => ("LiteralInInterpolation", Warning),
    r#loop => ("Loop", Warning),
    missing_super => ("MissingSuper", Warning),
    next_without_accumulator => ("NextWithoutAccumulator", Warning),
    non_local_exit_from_iterator => ("NonLocalExitFromIterator", Warning),
    parentheses_as_grouped_expression => ("ParenthesesAsGroupedExpression", Warning),
    raise_exception => ("RaiseException", Warning),
    rand_one => ("RandOne", Warning),
    rescue_exception => ("RescueException", Warning),
    return_in_void_context => ("ReturnInVoidContext", Warning),
    self_assignment => ("SelfAssignment", Warning),
    struct_new_override => ("StructNewOverride", Warning),
    suppressed_exception => ("SuppressedException", Warning),
    syntax => ("Syntax", Fatal),
    to_json => ("ToJSON", Warning),
    top_level_return_with_argument => ("TopLevelReturnWithArgument", Warning),
    trailing_comma_in_attribute_declaration => ("TrailingCommaInAttributeDeclaration", Warning),
    underscore_prefixed_variable_name => ("UnderscorePrefixedVariableName", Warning),
    unified_integer => ("UnifiedInteger", Warning),
    unreachable_code => ("UnreachableCode", Warning),
    unreachable_loop => ("UnreachableLoop", Warning),
    unused_block_argument => ("UnusedBlockArgument", Warning),
    unused_method_argument => ("UnusedMethodArgument", Warning),
    uri_escape_unescape => ("UriEscapeUnescape", Warning),
    uri_regexp => ("UriRegexp", Warning),
    useless_access_modifier => ("UselessAccessModifier", Warning),
    useless_assignment => ("UselessAssignment", Warning),
    useless_method_definition => ("UselessMethodDefinition", Warning),
}
