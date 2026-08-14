/// Shared analyses that belong to no single cop: how RuboCop's `SendNode` reads an access
/// modifier, what a `rescue` clause covers, whether a bare name reads a local variable, and
/// RuboCop's local variable tracking.
///
/// Reachable from `style` too: `VisibilityHelp` and `SendNode#access_modifier?` answer the same
/// questions for the Style cops that reason about where a `private` reaches.
pub(crate) mod access_modifier;
mod ambiguity;
mod blocks;
mod conditions;
mod cop_directives;
mod exception_hierarchy;
mod flow;
mod format_string;
/// Reachable from `style` too: what the parser knows the value of, and what it calls the node it
/// parked that value in, are questions the Style cops ported from node patterns share.
pub(crate) mod literals;
// `Layout/MultilineMethodCallIndentation` needs the same lvar/send distinction, so the analysis
// is visible to the other departments rather than duplicated there.
/// Reachable from `style` too: whether a bare identifier reads a local variable is a question
/// several Style cops ported from node patterns have to answer the same way.
pub(crate) mod locals;
/// Reachable from `style` too: `Style/SafeNavigation` measures a chained call against the same
/// `nil.methods` list that the `NilMethods` mixin gives the Lint cops.
pub(crate) mod nil_methods;
/// Reachable from `style` too: `Node#==` is the same question wherever a cop ported from a node
/// pattern compares two subtrees, and answering it by source text instead would call `a.b` and
/// `a. b` different nodes.
pub(crate) mod node_equality;
mod parameters;
mod percent_literal;
mod ranges;
mod regexp;
mod rescue_clause;
mod statements;
/// Reachable from the shared `RuleContext` too: the analysis is the same for every cop that
/// asks about a local variable, so the context caches one run of it per file.
pub(in crate::rules) mod variable_force;

department_rules! {
    "Lint";
    ambiguous_assignment => ("AmbiguousAssignment", Warning),
    ambiguous_block_association => ("AmbiguousBlockAssociation", Warning),
    ambiguous_operator => ("AmbiguousOperator", Warning),
    ambiguous_range => ("AmbiguousRange", Warning),
    ambiguous_regexp_literal => ("AmbiguousRegexpLiteral", Warning),
    assignment_in_condition => ("AssignmentInCondition", Warning),
    big_decimal_new => ("BigDecimalNew", Warning),
    binary_operator_with_identical_operands => ("BinaryOperatorWithIdenticalOperands", Warning),
    boolean_symbol => ("BooleanSymbol", Warning),
    circular_argument_reference => ("CircularArgumentReference", Warning),
    constant_definition_in_block => ("ConstantDefinitionInBlock", Warning),
    constant_overwritten_in_rescue => ("ConstantOverwrittenInRescue", Warning),
    debugger => ("Debugger", Warning),
    data_define_override => ("DataDefineOverride", Warning),
    deprecated_class_methods => ("DeprecatedClassMethods", Warning),
    deprecated_constants => ("DeprecatedConstants", Warning),
    deprecated_open_ssl_constant => ("DeprecatedOpenSSLConstant", Warning),
    disjunctive_assignment_in_constructor => ("DisjunctiveAssignmentInConstructor", Warning),
    duplicate_case_condition => ("DuplicateCaseCondition", Warning),
    duplicate_elsif_condition => ("DuplicateElsifCondition", Warning),
    duplicate_hash_key => ("DuplicateHashKey", Warning),
    duplicate_match_pattern => ("DuplicateMatchPattern", Warning),
    duplicate_magic_comment => ("DuplicateMagicComment", Warning),
    duplicate_methods => ("DuplicateMethods", Warning),
    duplicate_require => ("DuplicateRequire", Warning),
    duplicate_rescue_exception => ("DuplicateRescueException", Warning),
    duplicate_set_element => ("DuplicateSetElement", Warning),
    each_with_object_argument => ("EachWithObjectArgument", Warning),
    else_layout => ("ElseLayout", Warning),
    empty_class => ("EmptyClass", Warning),
    empty_block => ("EmptyBlock", Warning),
    empty_conditional_body => ("EmptyConditionalBody", Warning),
    empty_ensure => ("EmptyEnsure", Warning),
    empty_expression => ("EmptyExpression", Warning),
    empty_file => ("EmptyFile", Warning),
    empty_in_pattern => ("EmptyInPattern", Warning),
    empty_interpolation => ("EmptyInterpolation", Warning),
    empty_when => ("EmptyWhen", Warning),
    ensure_return => ("EnsureReturn", Warning),
    erb_new_arguments => ("ErbNewArguments", Warning),
    flip_flop => ("FlipFlop", Warning),
    float_comparison => ("FloatComparison", Warning),
    float_out_of_range => ("FloatOutOfRange", Warning),
    format_parameter_mismatch => ("FormatParameterMismatch", Warning),
    hash_compare_by_identity => ("HashCompareByIdentity", Warning),
    identity_comparison => ("IdentityComparison", Warning),
    hash_new_with_keyword_arguments_as_default => ("HashNewWithKeywordArgumentsAsDefault", Warning),
    implicit_string_concatenation => ("ImplicitStringConcatenation", Warning),
    incompatible_io_select_with_fiber_scheduler => ("IncompatibleIoSelectWithFiberScheduler", Warning),
    ineffective_access_modifier => ("IneffectiveAccessModifier", Warning),
    inherit_exception => ("InheritException", Warning),
    interpolation_check => ("InterpolationCheck", Warning),
    lambda_without_literal_block => ("LambdaWithoutLiteralBlock", Warning),
    literal_as_condition => ("LiteralAsCondition", Warning),
    literal_in_interpolation => ("LiteralInInterpolation", Warning),
    r#loop => ("Loop", Warning),
    missing_cop_enable_directive => ("MissingCopEnableDirective", Warning),
    missing_super => ("MissingSuper", Warning),
    mixed_regexp_capture_types => ("MixedRegexpCaptureTypes", Warning),
    multiple_comparison => ("MultipleComparison", Warning),
    nested_method_definition => ("NestedMethodDefinition", Warning),
    nested_percent_literal => ("NestedPercentLiteral", Warning),
    numbered_parameter_assignment => ("NumberedParameterAssignment", Warning),
    next_without_accumulator => ("NextWithoutAccumulator", Warning),
    no_return_in_begin_end_blocks => ("NoReturnInBeginEndBlocks", Warning),
    non_deterministic_require_order => ("NonDeterministicRequireOrder", Warning),
    non_local_exit_from_iterator => ("NonLocalExitFromIterator", Warning),
    numeric_operation_with_constant_result => ("NumericOperationWithConstantResult", Warning),
    ordered_magic_comments => ("OrderedMagicComments", Warning),
    or_assignment_to_constant => ("OrAssignmentToConstant", Warning),
    out_of_range_regexp_ref => ("OutOfRangeRegexpRef", Warning),
    parentheses_as_grouped_expression => ("ParenthesesAsGroupedExpression", Warning),
    percent_string_array => ("PercentStringArray", Warning),
    percent_symbol_array => ("PercentSymbolArray", Warning),
    raise_exception => ("RaiseException", Warning),
    rand_one => ("RandOne", Warning),
    redundant_dir_glob_sort => ("RedundantDirGlobSort", Warning),
    redundant_cop_disable_directive => ("RedundantCopDisableDirective", Warning),
    redundant_cop_enable_directive => ("RedundantCopEnableDirective", Warning),
    redundant_require_statement => ("RedundantRequireStatement", Warning),
    redundant_safe_navigation => ("RedundantSafeNavigation", Warning),
    redundant_splat_expansion => ("RedundantSplatExpansion", Warning),
    redundant_string_coercion => ("RedundantStringCoercion", Warning),
    redundant_with_index => ("RedundantWithIndex", Warning),
    redundant_with_object => ("RedundantWithObject", Warning),
    regexp_as_condition => ("RegexpAsCondition", Warning),
    require_parentheses => ("RequireParentheses", Warning),
    refinement_import_methods => ("RefinementImportMethods", Warning),
    require_relative_self_path => ("RequireRelativeSelfPath", Warning),
    rescue_exception => ("RescueException", Warning),
    rescue_type => ("RescueType", Warning),
    return_in_void_context => ("ReturnInVoidContext", Warning),
    safe_navigation_chain => ("SafeNavigationChain", Warning),
    safe_navigation_consistency => ("SafeNavigationConsistency", Warning),
    safe_navigation_with_empty => ("SafeNavigationWithEmpty", Warning),
    script_permission => ("ScriptPermission", Warning),
    self_assignment => ("SelfAssignment", Warning),
    send_with_mixin_argument => ("SendWithMixinArgument", Warning),
    shadowed_argument => ("ShadowedArgument", Warning),
    shared_mutable_default => ("SharedMutableDefault", Warning),
    shadowed_exception => ("ShadowedException", Warning),
    struct_new_override => ("StructNewOverride", Warning),
    suppressed_exception => ("SuppressedException", Warning),
    syntax => ("Syntax", Fatal),
    to_json => ("ToJSON", Warning),
    top_level_return_with_argument => ("TopLevelReturnWithArgument", Warning),
    triple_quotes => ("TripleQuotes", Warning),
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
    useless_defined => ("UselessDefined", Warning),
    useless_else_without_rescue => ("UselessElseWithoutRescue", Warning),
    useless_method_definition => ("UselessMethodDefinition", Warning),
    useless_or => ("UselessOr", Warning),
    useless_numeric_operation => ("UselessNumericOperation", Warning),
    useless_setter_call => ("UselessSetterCall", Warning),
    useless_times => ("UselessTimes", Warning),
    void => ("Void", Warning),
}
