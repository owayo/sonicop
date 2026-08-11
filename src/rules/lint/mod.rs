/// Shared by the two cops that need RuboCop's local variable analysis rather than a cop of its own.
mod variable_force;

department_rules! {
    "Lint";
    duplicate_methods => ("DuplicateMethods", Warning),
    syntax => ("Syntax", Fatal),
    unused_block_argument => ("UnusedBlockArgument", Warning),
    useless_assignment => ("UselessAssignment", Warning),
}
