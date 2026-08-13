use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, send_range, string_text};

use super::literals::{is_constant, literal_type};
use crate::rules::node_ext::NodeExt;

/// The ciphers whose name is not followed by a key size, so that `OpenSSL::Cipher::BF.new`
/// translates to a name with nothing appended.
const NO_ARG_ALGORITHM: &[&str] = &["BF", "DES", "IDEA", "RC4"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for call in context.nodes_of("call") {
        if !is_plain_send(call, context) {
            continue;
        }
        let Some(selector) = call.field("method") else {
            continue;
        };
        let method = context.source.node_text(selector);
        if !matches!(method, "new" | "digest") {
            continue;
        }
        let Some(receiver) = call.field("receiver") else {
            continue;
        };
        let call_arguments = arguments(call);
        // An argument the cop cannot read the value of makes the replacement unwritable.
        if call_arguments
            .iter()
            .flat_map(|argument| argument.parts())
            .any(|part| is_opaque(*part, context))
        {
            continue;
        }
        let Some((scope, name)) = openssl_algorithm(receiver, context) else {
            continue;
        };
        let openssl_class = context.source.node_text(scope);
        let arguments_source: Vec<&str> = call_arguments
            .iter()
            .map(|argument| context.source.slice(argument.range()))
            .collect();
        let replacement_args = replacement_args(
            context.source.node_text(receiver),
            openssl_class,
            context.source.node_text(name),
            &call_arguments,
            &arguments_source,
            context,
        );
        let range = send_range(call, context);
        let Some(double_colon) = separator(receiver, name) else {
            continue;
        };
        let Some(dot) = call.field("operator") else {
            continue;
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{openssl_class}.{method}({replacement_args})` instead of `{}`.",
                        context.source.slice(range.clone())
                    ),
                    range.clone(),
                )
                .corrected_by_all([
                    Edit {
                        start: double_colon.start_byte(),
                        end: double_colon.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: name.start_byte(),
                        end: name.end_byte(),
                        replacement: String::new(),
                        safe: true,
                    },
                    Edit {
                        start: dot.end_byte(),
                        end: range.end,
                        replacement: format!("{method}({replacement_args})"),
                        safe: true,
                    },
                ]),
        );
    }
}

/// `algorithm_const`: `OpenSSL::Cipher::Name` or `OpenSSL::Digest::Name`, answered as the scope the
/// name hangs off and the name itself. `OpenSSL::Digest` reached directly is `digest_const?`, which
/// the cop steps over rather than rewrites.
fn openssl_algorithm<'tree>(
    receiver: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<(Node<'tree>, Node<'tree>)> {
    if receiver.kind_str() != "scope_resolution" {
        return None;
    }
    let name = receiver.field("name")?;
    if context.source.node_text(name) == "Digest" {
        return None;
    }
    let scope = receiver.field("scope")?;
    if scope.kind_str() != "scope_resolution" {
        return None;
    }
    let library = scope.field("name")?;
    if !matches!(context.source.node_text(library), "Cipher" | "Digest") {
        return None;
    }
    // `{nil? cbase}` in front of `OpenSSL`: the constant has to be reached from the top level.
    let root = scope.field("scope")?;
    let named = match root.kind_str() {
        "constant" => context.source.node_text(root) == "OpenSSL",
        "scope_resolution" => {
            root.field("scope").is_none()
                && root
                    .field("name")
                    .is_some_and(|inner| context.source.node_text(inner) == "OpenSSL")
        }
        _ => false,
    };
    named.then_some((scope, name))
}

/// `arg.variable? || arg.call_type? || arg.const_type?`: an argument whose value the cop cannot
/// fold into the replacement string.
fn is_opaque(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    matches!(
        node.kind_str(),
        "instance_variable" | "global_variable" | "class_variable" | "call" | "identifier"
    ) || is_constant(node, context)
}

fn replacement_args(
    receiver_source: &str,
    openssl_class: &str,
    name: &str,
    call_arguments: &[crate::rules::send_node::Argument<'_>],
    arguments_source: &[&str],
    context: &RuleContext<'_>,
) -> String {
    if receiver_source == "OpenSSL::Cipher::Cipher" {
        return arguments_source.first().unwrap_or(&"").to_string();
    }
    let algorithm = algorithm_name(openssl_class, name);
    if openssl_class != "OpenSSL::Cipher" {
        return std::iter::once(format!("'{algorithm}'"))
            .chain(arguments_source.iter().map(|source| (*source).to_owned()))
            .collect::<Vec<String>>()
            .join(", ");
    }
    build_cipher_arguments(&algorithm, call_arguments, context)
}

/// `algorithm_name`: a cipher's name is cut into three-character groups joined by dashes, which
/// drops whatever is left over when the length is not a multiple of three.
fn algorithm_name(openssl_class: &str, name: &str) -> String {
    if openssl_class != "OpenSSL::Cipher" || NO_ARG_ALGORITHM.contains(&name) {
        return name.to_owned();
    }
    name.as_bytes()
        .chunks_exact(3)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<String>>()
        .join("-")
}

fn build_cipher_arguments(
    algorithm: &str,
    call_arguments: &[crate::rules::send_node::Argument<'_>],
    context: &RuleContext<'_>,
) -> String {
    let parts: Vec<String> = algorithm
        .to_lowercase()
        .split('-')
        .map(str::to_owned)
        .collect();
    let size_and_mode = sanitize_arguments(call_arguments, context);
    if call_arguments.is_empty()
        && parts
            .first()
            .is_some_and(|first| NO_ARG_ALGORITHM.contains(&first.to_uppercase().as_str()))
    {
        return format!("'{}'", parts.first().cloned().unwrap_or_default());
    }
    let mode = size_and_mode.is_empty().then(|| "cbc".to_owned());
    let joined: Vec<String> = parts
        .into_iter()
        .chain(size_and_mode)
        .chain(mode)
        .take(3)
        .collect();
    format!("'{}'", joined.join("-"))
}

/// `sanitize_arguments`: the value of each argument with `:` and `'` dropped, split on dashes.
fn sanitize_arguments(
    call_arguments: &[crate::rules::send_node::Argument<'_>],
    context: &RuleContext<'_>,
) -> Vec<String> {
    call_arguments
        .iter()
        .flat_map(|argument| {
            let node = argument.first();
            let text = match literal_type(node, context) {
                Some("str") => string_text(node, context).to_owned(),
                _ => context.source.slice(argument.range()).to_owned(),
            };
            text.replace([':', '\''], "")
                .split('-')
                .map(|part| part.to_lowercase())
                .collect::<Vec<String>>()
        })
        .collect()
}

/// `loc.double_colon`: the `::` the name was written after.
fn separator<'tree>(receiver: Node<'tree>, name: Node<'_>) -> Option<Node<'tree>> {
    let mut cursor = receiver.walk();
    receiver
        .children(&mut cursor)
        .find(|child| !child.is_named() && child.end_byte() <= name.start_byte())
}
