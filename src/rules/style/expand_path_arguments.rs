use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::is_plain_send;
use crate::rules::node_ext::NodeExt;

const PATHNAME_MSG: &str =
    "Use `Pathname(__dir__).expand_path` instead of `Pathname(__FILE__).parent.expand_path`.";
const PATHNAME_NEW_MSG: &str = "Use `Pathname.new(__dir__).expand_path` instead of `Pathname.new(__FILE__).parent.expand_path`.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        // Upstream's `on_send` is never called for a `csend` node, and this cop does not alias
        // `on_csend`, so `x&.foo` is not its business. The grammar has one kind for both.
        if !is_plain_send(node, context) {
            continue;
        }
        if node
            .field("method")
            .is_none_or(|method| context.source.node_text(method) != "expand_path")
        {
            continue;
        }
        if let Some((path, default_dir)) = file_expand_path(context, node) {
            report_file(context, offenses, node, path, default_dir);
        } else if let Some((parent, argument, qualified)) = pathname_parent(context, node) {
            if context.source.node_text(argument) != "__FILE__" {
                continue;
            }
            let message = match qualified {
                true => PATHNAME_NEW_MSG,
                false => PATHNAME_MSG,
            };
            let (Some(dot), Some(selector)) = (
                parent.field("operator"),
                parent.field("method"),
            ) else {
                continue;
            };
            offenses.push(
                context
                    .offense(message, node.byte_range())
                    .corrected_by_all([
                        Edit {
                            start: argument.start_byte(),
                            end: argument.end_byte(),
                            replacement: "__dir__".to_owned(),
                            safe: true,
                        },
                        Edit {
                            start: dot.start_byte(),
                            end: dot.end_byte(),
                            replacement: String::new(),
                            safe: true,
                        },
                        Edit {
                            start: selector.start_byte(),
                            end: selector.end_byte(),
                            replacement: String::new(),
                            safe: true,
                        },
                    ]),
            );
        }
    }
}

fn report_file(
    context: &RuleContext<'_>,
    offenses: &mut Vec<Offense>,
    node: Node<'_>,
    path: Node<'_>,
    default_dir: Node<'_>,
) {
    // `unrecommended_argument?` and `current_path.str_type?`.
    if context.source.node_text(default_dir) != "__FILE__"
        || path.kind_str() != "string"
        || path.start_position().row != path.end_position().row
        || super::nodes::children(path)
            .iter()
            .any(|child| child.kind_str() == "interpolation")
    {
        return;
    }
    // `strip_surrounded_quotes!` drops the first and last character of the *source*.
    let source = context.source.node_text(path);
    let mut characters = source.chars();
    characters.next();
    characters.next_back();
    let current: String = characters.collect();

    let parent = parent_path(&current);
    let new_path = match parent.is_empty() {
        true => String::new(),
        false => format!("'{parent}', "),
    };
    let depth = depth(&current);
    let new_default_dir = match depth {
        0 => "__FILE__",
        _ => "__dir__",
    };
    let message = format!(
        "Use `expand_path({new_path}{new_default_dir})` instead of `expand_path('{current}', __FILE__)`."
    );
    let Some(selector) = node.field("method") else {
        return;
    };
    let edits = match depth {
        // The whole argument list collapses to the one keyword that says the same thing.
        0 | 1 => vec![Edit {
            start: path.start_byte(),
            end: default_dir.end_byte(),
            replacement: match depth {
                0 => "__FILE__".to_owned(),
                _ => "__dir__".to_owned(),
            },
            safe: true,
        }],
        _ => vec![
            Edit {
                start: path.start_byte(),
                end: path.end_byte(),
                replacement: format!("'{parent}'"),
                safe: true,
            },
            Edit {
                start: default_dir.start_byte(),
                end: default_dir.end_byte(),
                replacement: "__dir__".to_owned(),
                safe: true,
            },
        ],
    };
    offenses.push(
        context
            .offense(message, selector.byte_range())
            .corrected_by_all(edits),
    );
}

/// `(send (const {nil? cbase} :File) :expand_path $_ $_)`.
fn file_expand_path<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    let receiver = node.field("receiver")?;
    if !names_constant(context, receiver, "File") {
        return None;
    }
    let arguments = node.field("arguments")?;
    match super::nodes::children(arguments).as_slice() {
        [path, default_dir] => Some((*path, *default_dir)),
        _ => None,
    }
}

/// `Pathname(x).parent.expand_path` and `Pathname.new(x).parent.expand_path`: the `.parent` call,
/// the argument, and which of the two spellings it was.
fn pathname_parent<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>, bool)> {
    if node.field("arguments").is_some() {
        return None;
    }
    let parent = node.field("receiver")?;
    if parent.kind_str() != "call"
        || parent.field("arguments").is_some()
        || parent
            .field("method")
            .is_none_or(|method| context.source.node_text(method) != "parent")
    {
        return None;
    }
    let call = parent.field("receiver")?;
    if call.kind_str() != "call" {
        return None;
    }
    let arguments = call.field("arguments")?;
    let [argument] = super::nodes::children(arguments)[..] else {
        return None;
    };
    let method = call.field("method")?;
    match call.field("receiver") {
        // `Pathname.new(x)`.
        Some(receiver) => (context.source.node_text(method) == "new"
            && names_constant(context, receiver, "Pathname"))
        .then_some((parent, argument, true)),
        // `Pathname(x)`, which is a receiverless call.
        None => {
            (context.source.node_text(method) == "Pathname").then_some((parent, argument, false))
        }
    }
}

/// `(const {nil? cbase} :Name)`: the constant, qualified by nothing or by the root.
fn names_constant(context: &RuleContext<'_>, node: Node<'_>, name: &str) -> bool {
    let named = match node.kind_str() {
        "constant" => node,
        "scope_resolution" if node.field("scope").is_none() => {
            match node.field("name") {
                Some(inner) => inner,
                None => return false,
            }
        }
        _ => return false,
    };
    context.source.node_text(named) == name
}

/// `String#split(File::SEPARATOR)`, which drops the empty fields a trailing separator leaves.
fn segments(current: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = current.split('/').collect();
    while parts.last() == Some(&"") {
        parts.pop();
    }
    parts
}

/// `depth`: how many segments of the path are not `.`.
fn depth(current: &str) -> usize {
    segments(current).into_iter().filter(|part| *part != ".").count()
}

/// `parent_path`: every `.` goes, and then the first `..` does.
fn parent_path(current: &str) -> String {
    let mut parts: Vec<&str> = segments(current)
        .into_iter()
        .filter(|part| *part != ".")
        .collect();
    if let Some(index) = parts.iter().position(|part| *part == "..") {
        parts.remove(index);
    }
    parts.join("/")
}
