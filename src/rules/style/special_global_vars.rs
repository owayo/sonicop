//! `Style/SpecialGlobalVars`: `$ERROR_INFO` from the `English` library rather than Perl's `$!`.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node;
use crate::rules::node_ext::NodeExt;

/// `ENGLISH_VARS`: the Perl-style globals and the names the `English` library gives them.
const ENGLISH_VARS: &[(&str, &[&str])] = &[
    ("$:", &["$LOAD_PATH"]),
    ("$\"", &["$LOADED_FEATURES"]),
    ("$0", &["$PROGRAM_NAME"]),
    ("$!", &["$ERROR_INFO"]),
    ("$@", &["$ERROR_POSITION"]),
    ("$;", &["$FIELD_SEPARATOR", "$FS"]),
    ("$,", &["$OUTPUT_FIELD_SEPARATOR", "$OFS"]),
    ("$/", &["$INPUT_RECORD_SEPARATOR", "$RS"]),
    ("$\\", &["$OUTPUT_RECORD_SEPARATOR", "$ORS"]),
    ("$.", &["$INPUT_LINE_NUMBER", "$NR"]),
    ("$_", &["$LAST_READ_LINE"]),
    ("$>", &["$DEFAULT_OUTPUT"]),
    ("$<", &["$DEFAULT_INPUT"]),
    ("$$", &["$PROCESS_ID", "$PID"]),
    ("$?", &["$CHILD_STATUS"]),
    ("$~", &["$LAST_MATCH_INFO"]),
    ("$=", &["$IGNORECASE"]),
    ("$*", &["$ARGV", "ARGV"]),
];

/// `NON_ENGLISH_VARS`: the readable names the interpreter provides on its own, which need no
/// `require`.
const NON_ENGLISH_VARS: &[&str] = &["$LOAD_PATH", "$LOADED_FEATURES", "$PROGRAM_NAME", "ARGV"];

const LIBRARY_NAME: &str = "English";

/// Node kinds tree-sitter writes a literal with interpolation as, which upstream's parser reads as
/// a `dstr` -- the one place a replacement has to keep the `#{}` around it.
const STRING_KINDS: &[&str] = &["string", "heredoc_body", "chained_string", "bare_string"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let style = context
        .setting::<String>("EnforcedStyle")
        .unwrap_or_else(|| "use_english_names".to_owned());
    let require_english: bool = context.setting("RequireEnglish").unwrap_or(true);
    let file = TopLevel::new(context);
    // `@required_english`: the library is required once per file, by the first offense that needs
    // it.
    let mut required_english = false;

    for node in context.nodes_of("global_variable") {
        let name = context.source.node_text(node);
        // `on_gvar` never sees an assignment target: `$; = ','` is a `gvasgn` upstream.
        if is_assignment_target(node) {
            continue;
        }
        let Some(preferred) = preferred_names(name, &style) else {
            continue;
        };
        // The name is already the one this style asks for.
        if preferred.contains(&name) {
            continue;
        }
        let corrected = climb(context, node);
        let mut edits = Vec::new();
        let mut anchor = None;
        // `should_require_english?`.
        if style == "use_english_names"
            && require_english
            && !required_english
            && !NON_ENGLISH_VARS.contains(&preferred[0])
        {
            if let Some(require) = file.ensure_required(context, corrected) {
                edits.extend(require.edits);
                anchor = Some(require.anchor);
            }
            required_english = true;
        }
        edits.push(Edit {
            start: corrected.start_byte(),
            end: corrected.end_byte(),
            replacement: replacement(context, corrected, preferred[0], &style),
            safe: true,
        });
        let mut offense = context
            .offense(message(name, &preferred, &style), node.byte_range())
            .corrected_by_all(edits);
        // The `require` is inserted before the file's first statement, which is the range upstream
        // hands its corrector rather than the one this offense reports.
        if let Some(anchor) = anchor {
            offense = offense.corrections_anchored_at(anchor);
        }
        offenses.push(offense);
    }
}

/// The file's top-level statements, which is the scope `RequireLibrary` reasons about.
struct TopLevel<'t> {
    statements: Vec<Node<'t>>,
    /// Whether upstream's root is a `begin`, which is what makes the statements siblings.
    begin_root: bool,
}

/// What inserting `require 'English'` costs: the edits, and the range they hang off.
struct Require {
    edits: Vec<Edit>,
    anchor: Range<usize>,
}

impl<'t> TopLevel<'t> {
    fn new(context: &RuleContext<'t>) -> Self {
        let statements = super::nodes::children(context.root_node());
        Self {
            begin_root: statements.len() > 1,
            statements,
        }
    }

    /// `ensure_required`: the `require 'English'` the file is missing, or `None` when it already
    /// has one above the code being corrected.
    fn ensure_required(&self, context: &RuleContext<'_>, node: Node<'_>) -> Option<Require> {
        let statement = self.statements.iter().position(|top| {
            top.start_byte() <= node.start_byte() && node.end_byte() <= top.end_byte()
        })?;
        let mut edits = Vec::new();
        if self.begin_root {
            // `@required_libs`: a `require` upstream met before this point in the walk.
            if self.statements[..statement]
                .iter()
                .any(|top| requires_library(*top, context))
            {
                return None;
            }
            // `remove_subsequent_requires`: the one written below moves to the top of the file.
            for top in &self.statements[statement + 1..] {
                if requires_library(*top, context) {
                    let line = context.source.line_column(top.start_byte()).0;
                    let last = context.source.line_column(top.end_byte()).0;
                    edits.push(Edit {
                        start: context.source.line_start(line),
                        end: context.source.line_range(last).end,
                        replacement: String::new(),
                        safe: true,
                    });
                }
            }
        }
        let first = self.statements.first()?;
        edits.push(Edit {
            start: first.start_byte(),
            end: first.start_byte(),
            replacement: format!("require '{LIBRARY_NAME}'\n"),
            safe: true,
        });
        Some(Require {
            edits,
            anchor: first.byte_range(),
        })
    }
}

/// `require_library_name?`: a top-level `require 'English'`, however the receiver was spelled.
fn requires_library(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if node.kind_str() != "call" {
        return false;
    }
    if let Some(receiver) = node.field("receiver") {
        if !send_node::top_level_constant(receiver, "Kernel", context) {
            return false;
        }
    }
    if node
        .field("method")
        .is_none_or(|method| context.source.node_text(method) != "require")
    {
        return false;
    }
    let arguments = send_node::arguments(node);
    let [only] = arguments.as_slice() else {
        return false;
    };
    let argument = only.first();
    send_node::is_string(argument, context)
        && send_node::string_text(argument, context) == LIBRARY_NAME
}

/// `node = node.parent while node.parent&.begin_type? && node.parent.children.one?`: a global
/// standing alone inside `#{...}` or `(...)` is replaced along with what wraps it.
fn climb<'t>(context: &RuleContext<'_>, mut node: Node<'t>) -> Node<'t> {
    while let Some(parent) = node.parent() {
        if !is_lone_begin(context, parent) {
            break;
        }
        node = parent;
    }
    node
}

fn is_lone_begin(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        // `"#$!"` interpolates without a `begin`: upstream hangs the global off the `dstr` itself.
        "interpolation" => {
            context.source.node_text(node).starts_with("#{")
                && super::nodes::children(node).len() == 1
        }
        "parenthesized_statements" => super::nodes::children(node).len() == 1,
        _ => false,
    }
}

/// The node upstream's parser makes the parent, which for the `#$!` short form is the literal
/// rather than the interpolation tree-sitter writes around it.
fn upstream_parent<'t>(context: &RuleContext<'_>, node: Node<'t>) -> Option<Node<'t>> {
    let parent = node.parent()?;
    match parent.kind_str() == "interpolation" && !context.source.node_text(parent).starts_with("#{") {
        true => parent.parent(),
        false => Some(parent),
    }
}

fn replacement(context: &RuleContext<'_>, node: Node<'_>, preferred: &str, style: &str) -> String {
    let parent = upstream_parent(context, node).map(|parent| parent.kind_str());
    let interpolating = parent
        .is_some_and(|kind| STRING_KINDS.contains(&kind) || matches!(kind, "regex" | "subshell"));
    if !interpolating {
        return preferred.to_owned();
    }
    if style != "use_english_names" {
        return format!("#{preferred}");
    }
    // `english_name_replacement`: a name that is not one word cannot be interpolated bare.
    match node.kind_str() == "interpolation" {
        true => format!("#{{{preferred}}}"),
        false => format!("{{{preferred}}}"),
    }
}

/// `preferred_names`: the names this style asks for in place of `global`, or `None` when the
/// global is not one this cop knows.
fn preferred_names(global: &str, style: &str) -> Option<Vec<&'static str>> {
    match style {
        "use_perl_names" => perl_names(global),
        "use_builtin_english_names" => builtin_names(global),
        _ => english_names(global),
    }
}

/// `ENGLISH_VARS` after its identity entries have been merged in.
fn english_names(global: &str) -> Option<Vec<&'static str>> {
    if let Some((_, names)) = ENGLISH_VARS.iter().find(|(perl, _)| *perl == global) {
        return Some(names.to_vec());
    }
    english_name(global).map(|name| vec![name])
}

/// `PERL_VARS`: every English name back to the Perl-style global it stands for, and every Perl
/// global to itself.
fn perl_names(global: &str) -> Option<Vec<&'static str>> {
    if let Some((perl, _)) = ENGLISH_VARS.iter().find(|(perl, _)| *perl == global) {
        return Some(vec![perl]);
    }
    ENGLISH_VARS
        .iter()
        .find(|(_, names)| names.contains(&global))
        .map(|(perl, _)| vec![*perl])
}

/// `BUILTIN_VARS`: `PERL_VARS`, except that the three globals the interpreter already names
/// readably keep those names.
fn builtin_names(global: &str) -> Option<Vec<&'static str>> {
    for builtin in NON_ENGLISH_VARS.iter().filter(|name| name.starts_with('$')) {
        let perl = perl_names(builtin)?;
        if global == *builtin || perl.contains(&global) {
            return Some(vec![builtin]);
        }
    }
    perl_names(global)
}

/// The `English` name a global spells, for the identity entries of the tables.
fn english_name(global: &str) -> Option<&'static str> {
    ENGLISH_VARS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .find(|name| **name == global)
        .copied()
}

fn message(global: &str, preferred: &[&str], style: &str) -> String {
    if style != "use_english_names" {
        return format!("Prefer `{}` over `{global}`.", preferred[0]);
    }
    // `format_english_message` reads the English table whatever the preferred names were.
    let names = english_names(global).unwrap_or_default();
    let (regular, english): (Vec<&str>, Vec<&str>) = names
        .iter()
        .partition(|name| NON_ENGLISH_VARS.contains(name));
    let stdlib = "from the stdlib 'English' module (don't forget to require it)";
    match (regular.is_empty(), english.is_empty()) {
        (true, _) => format!(
            "Prefer `{}` {stdlib} over `{global}`.",
            english.join("` or `")
        ),
        (false, true) => format!("Prefer `{}` over `{global}`.", regular.join("` or `")),
        (false, false) => format!(
            "Prefer `{}` {stdlib} or `{}` over `{global}`.",
            english.join("` or `"),
            regular.join("` or `")
        ),
    }
}

/// Whether the global is being written rather than read, which upstream's parser spells as a
/// `gvasgn` that `on_gvar` never sees.
fn is_assignment_target(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind_str() {
        "assignment" | "operator_assignment" => parent
            .field("left")
            .is_some_and(|left| left.id() == node.id()),
        "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => true,
        _ => false,
    }
}
