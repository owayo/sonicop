use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{Argument, arguments, is_plain_send, send_range};

/// Which of the five patterns a call matched, since each names a different range and a different
/// replacement.
enum Deprecated {
    /// `ENV.clone`, `ENV.dup`, `ENV.freeze`, `File.exists?`, `Dir.exists?`: the receiver is one of
    /// the three constants whose name the replacement repeats.
    Constant,
    /// `Socket.gethostbyaddr`, `Socket.gethostbyname`: named the same way, but never corrected --
    /// the replacement is a different class with a different call shape.
    Socket,
    /// `attr :name, true`.
    Attr,
    /// `iterator?`.
    Receiverless,
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    for node in context.nodes_of("call") {
        let Some(method) = node.child_by_field_name("method") else {
            continue;
        };
        if !is_plain_send(node, context) {
            continue;
        }
        let name = context.source.node_text(method);
        let arguments = arguments(node);
        let Some(kind) = matched(node, name, &arguments, context) else {
            continue;
        };
        let range = offense_range(node, method, &kind, context);
        let prefer = preferred(node, name, &kind, &arguments, context);
        let message = format!(
            "`{}` is deprecated in favor of `{prefer}`.",
            context.source.slice(range.clone()),
        );
        let offense = context.offense(message, range.clone());
        offenses.push(match kind {
            Deprecated::Socket => offense,
            // `ENV.freeze` is the one call whose replacement is not a call at all, so it replaces
            // the whole send rather than the part up to the selector.
            Deprecated::Constant if name == "freeze" => {
                let whole = send_range(node, context);
                offense.corrected_by(Edit {
                    start: whole.start,
                    end: whole.end,
                    replacement: "ENV".to_owned(),
                    safe: true,
                })
            }
            _ => offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: prefer,
                safe: true,
            }),
        });
    }
}

fn matched(
    node: Node<'_>,
    name: &str,
    arguments: &[Argument<'_>],
    context: &RuleContext<'_>,
) -> Option<Deprecated> {
    let Some(receiver) = node.child_by_field_name("receiver") else {
        return match name {
            // `(send nil? :attr _ boolean)`.
            "attr" if arguments.len() == 2 && boolean(arguments[1].first(), context) => {
                Some(Deprecated::Attr)
            }
            "iterator?" if arguments.is_empty() => Some(Deprecated::Receiverless),
            _ => None,
        };
    };
    let constant = short_name(receiver, context)?;
    match (constant, name) {
        ("ENV", "clone" | "dup" | "freeze") if arguments.is_empty() => Some(Deprecated::Constant),
        ("File" | "Dir", "exists?") if arguments.len() == 1 => Some(Deprecated::Constant),
        ("Socket", "gethostbyaddr" | "gethostbyname") => Some(Deprecated::Socket),
        _ => None,
    }
}

/// The name of a constant reached from the top level: `(const {cbase nil?} :Name)`.
fn short_name<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    match node.kind() {
        "constant" => Some(context.source.node_text(node)),
        "scope_resolution" if node.child_by_field_name("scope").is_none() => node
            .child_by_field_name("name")
            .map(|name| context.source.node_text(name)),
        _ => None,
    }
}

fn boolean(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    matches!(context.source.node_text(node), "true" | "false")
        && matches!(node.kind(), "true" | "false")
}

fn offense_range(
    node: Node<'_>,
    method: Node<'_>,
    kind: &Deprecated,
    context: &RuleContext<'_>,
) -> Range<usize> {
    match kind {
        // `node.source_range.begin.join(node.loc.selector.end)`.
        Deprecated::Constant | Deprecated::Socket => node.start_byte()..method.end_byte(),
        Deprecated::Attr => send_range(node, context),
        Deprecated::Receiverless => method.byte_range(),
    }
}

fn preferred(
    node: Node<'_>,
    name: &str,
    kind: &Deprecated,
    arguments: &[Argument<'_>],
    context: &RuleContext<'_>,
) -> String {
    match kind {
        Deprecated::Attr => {
            let accessor = if context.source.slice(arguments[1].range()) == "true" {
                "attr_accessor"
            } else {
                "attr_reader"
            };
            format!("{accessor} {}", context.source.slice(arguments[0].range()))
        }
        Deprecated::Constant => {
            let receiver = node
                .child_by_field_name("receiver")
                .map_or("", |receiver| context.source.node_text(receiver));
            match name {
                "clone" | "dup" => format!("{receiver}.to_h"),
                "exists?" => format!("{receiver}.exist?"),
                // `PREFERRED_METHODS` has no entry for `freeze`, and the fallback is the constant
                // on its own.
                _ => "ENV".to_owned(),
            }
        }
        Deprecated::Socket if name == "gethostbyaddr" => "Addrinfo#getnameinfo".to_owned(),
        Deprecated::Socket => "Addrinfo.getaddrinfo".to_owned(),
        Deprecated::Receiverless => "block_given?".to_owned(),
    }
}
