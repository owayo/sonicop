/// Shared by the two cops that need RuboCop's local variable analysis rather than a cop of its own.
mod variable_force;

department_rules! {
    "Lint";
    ambiguous_block_association => ("AmbiguousBlockAssociation", Warning),
    assignment_in_condition => ("AssignmentInCondition", Warning),
    constant_definition_in_block => ("ConstantDefinitionInBlock", Warning),
    duplicate_methods => ("DuplicateMethods", Warning),
    interpolation_check => ("InterpolationCheck", Warning),
    missing_super => ("MissingSuper", Warning),
    suppressed_exception => ("SuppressedException", Warning),
    syntax => ("Syntax", Fatal),
    unused_block_argument => ("UnusedBlockArgument", Warning),
    unused_method_argument => ("UnusedMethodArgument", Warning),
    useless_assignment => ("UselessAssignment", Warning),
}
