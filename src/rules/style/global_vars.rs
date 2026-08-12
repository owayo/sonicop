//! `Style/GlobalVars`: a name of one's own does not belong in the global namespace.

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

const MSG: &str = "Do not introduce global variables.";

/// `BUILT_IN_VARS`: the interpreter's own globals and their English aliases.
const BUILT_IN_VARS: &[&str] = &[
    "$:", "$LOAD_PATH", "$\"", "$LOADED_FEATURES", "$0", "$PROGRAM_NAME", "$!", "$ERROR_INFO",
    "$@", "$ERROR_POSITION", "$;", "$FS", "$FIELD_SEPARATOR", "$,", "$OFS",
    "$OUTPUT_FIELD_SEPARATOR", "$/", "$RS", "$INPUT_RECORD_SEPARATOR", "$\\", "$ORS",
    "$OUTPUT_RECORD_SEPARATOR", "$.", "$NR", "$INPUT_LINE_NUMBER", "$_", "$LAST_READ_LINE", "$>",
    "$DEFAULT_OUTPUT", "$<", "$DEFAULT_INPUT", "$$", "$PID", "$PROCESS_ID", "$?", "$CHILD_STATUS",
    "$~", "$LAST_MATCH_INFO", "$=", "$IGNORECASE", "$*", "$ARGV", "$&", "$MATCH", "$`",
    "$PREMATCH", "$'", "$POSTMATCH", "$+", "$LAST_PAREN_MATCH", "$stdin", "$stdout", "$stderr",
    "$DEBUG", "$FILENAME", "$VERBOSE", "$SAFE", "$-0", "$-a", "$-d", "$-F", "$-i", "$-I", "$-l",
    "$-p", "$-v", "$-w", "$CLASSPATH", "$JRUBY_VERSION", "$JRUBY_REVISION", "$ENV_JAVA",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context.setting("AllowedVariables").unwrap_or_default();
    for node in context.nodes_of("global_variable") {
        let name = context.source.node_text(node);
        // `$1` and its siblings are `nth_ref` nodes upstream, which this cop never sees.
        if name[1..].starts_with(|character: char| character.is_ascii_digit()) && name != "$0" {
            continue;
        }
        if BUILT_IN_VARS.contains(&name) || allowed.iter().any(|variable| variable == name) {
            continue;
        }
        offenses.push(context.offense(MSG, node.byte_range()));
    }
}
