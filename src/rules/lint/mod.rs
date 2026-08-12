/// Shared analyses that belong to no single cop: how RuboCop's `SendNode` reads an access
/// modifier, what a `rescue` clause covers, and RuboCop's local variable tracking.
mod access_modifier;
mod rescue_clause;
mod variable_force;

department_rules! {
    "Lint";
    ambiguous_block_association => ("AmbiguousBlockAssociation", Warning),
    assignment_in_condition => ("AssignmentInCondition", Warning),
    boolean_symbol => ("BooleanSymbol", Warning),
    constant_definition_in_block => ("ConstantDefinitionInBlock", Warning),
    duplicate_methods => ("DuplicateMethods", Warning),
    ineffective_access_modifier => ("IneffectiveAccessModifier", Warning),
    interpolation_check => ("InterpolationCheck", Warning),
    literal_in_interpolation => ("LiteralInInterpolation", Warning),
    missing_super => ("MissingSuper", Warning),
    rescue_exception => ("RescueException", Warning),
    suppressed_exception => ("SuppressedException", Warning),
    syntax => ("Syntax", Fatal),
    underscore_prefixed_variable_name => ("UnderscorePrefixedVariableName", Warning),
    unused_block_argument => ("UnusedBlockArgument", Warning),
    unused_method_argument => ("UnusedMethodArgument", Warning),
    useless_assignment => ("UselessAssignment", Warning),
}
