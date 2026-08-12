/// Shared analyses that belong to no single cop: how RuboCop's `SendNode` reads an access
/// modifier, what a `rescue` clause covers, whether a bare name reads a local variable, and
/// RuboCop's local variable tracking.
mod access_modifier;
mod locals;
mod node_equality;
mod rescue_clause;
mod variable_force;

department_rules! {
    "Lint";
    ambiguous_block_association => ("AmbiguousBlockAssociation", Warning),
    assignment_in_condition => ("AssignmentInCondition", Warning),
    binary_operator_with_identical_operands => ("BinaryOperatorWithIdenticalOperands", Warning),
    boolean_symbol => ("BooleanSymbol", Warning),
    constant_definition_in_block => ("ConstantDefinitionInBlock", Warning),
    duplicate_methods => ("DuplicateMethods", Warning),
    empty_file => ("EmptyFile", Warning),
    empty_interpolation => ("EmptyInterpolation", Warning),
    empty_when => ("EmptyWhen", Warning),
    float_comparison => ("FloatComparison", Warning),
    hash_compare_by_identity => ("HashCompareByIdentity", Warning),
    ineffective_access_modifier => ("IneffectiveAccessModifier", Warning),
    inherit_exception => ("InheritException", Warning),
    interpolation_check => ("InterpolationCheck", Warning),
    literal_in_interpolation => ("LiteralInInterpolation", Warning),
    r#loop => ("Loop", Warning),
    missing_super => ("MissingSuper", Warning),
    non_local_exit_from_iterator => ("NonLocalExitFromIterator", Warning),
    raise_exception => ("RaiseException", Warning),
    rescue_exception => ("RescueException", Warning),
    self_assignment => ("SelfAssignment", Warning),
    struct_new_override => ("StructNewOverride", Warning),
    suppressed_exception => ("SuppressedException", Warning),
    syntax => ("Syntax", Fatal),
    underscore_prefixed_variable_name => ("UnderscorePrefixedVariableName", Warning),
    unused_block_argument => ("UnusedBlockArgument", Warning),
    unused_method_argument => ("UnusedMethodArgument", Warning),
    useless_access_modifier => ("UselessAccessModifier", Warning),
    useless_assignment => ("UselessAssignment", Warning),
    useless_method_definition => ("UselessMethodDefinition", Warning),
}
