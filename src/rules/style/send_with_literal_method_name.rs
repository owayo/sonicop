use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

const SENDERS: [&str; 3] = ["public_send", "send", "__send__"];

/// `RESERVED_WORDS`, which upstream holds as symbols -- so a **string** argument is never checked
/// against them, and `foo.public_send('class')` is reported while `foo.public_send(:class)` is not.
const RESERVED_WORDS: &[&str] = &[
    "BEGIN", "END", "alias", "and", "begin", "break", "case", "class", "def", "defined?", "do",
    "else", "elsif", "end", "ensure", "false", "for", "if", "in", "module", "next", "nil", "not",
    "or", "redo", "rescue", "retry", "return", "self", "super", "then", "true", "undef", "unless",
    "until", "when", "while", "yield",
];

/// `foo.public_send(:bar)`, which says the method's name in a literal instead of calling it.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_send = context.setting::<bool>("AllowSend").unwrap_or(true);
    for node in context.nodes_of("call") {
        let Some(selector) = node.field("method") else {
            continue;
        };
        let sender = context.source.node_text(selector);
        if !SENDERS.contains(&sender) {
            continue;
        }
        // `allow_send?`: with it on, only `public_send` is a problem.
        if allow_send && sender != "public_send" {
            continue;
        }
        let arguments = node
            .field("arguments")
            .map(super::nodes::children)
            .unwrap_or_default();
        let Some(first) = arguments.first() else {
            continue;
        };
        // `first_argument.type?(:sym, :str)` and then its value.
        let (name, from_symbol) = match send_node::symbol_name(*first, context) {
            Some(name) => (name, true),
            None if send_node::is_string(*first, context) => {
                (send_node::string_text(*first, context), false)
            }
            None => continue,
        };
        if !is_method_name(name) {
            continue;
        }
        if from_symbol && RESERVED_WORDS.contains(&name) {
            continue;
        }
        // `node.loc.selector.join(node.source_range.end)`.
        let range = selector.start_byte()..send_node::send_range(node, context).end;
        let offense = context.offense(
            format!("Use `{name}` method call directly instead."),
            range.clone(),
        );
        offenses.push(match arguments.as_slice() {
            [_] => offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement: name.to_owned(),
                safe: true,
            }),
            [_, second, ..] => offense.corrected_by_all([
                Edit {
                    start: selector.start_byte(),
                    end: selector.end_byte(),
                    replacement: name.to_owned(),
                    safe: true,
                },
                // `first_argument.source_range.begin.join(second_argument.source_range.begin)`.
                Edit {
                    start: first.start_byte(),
                    end: second.start_byte(),
                    replacement: String::new(),
                    safe: true,
                },
            ]),
            [] => offense,
        });
    }
}

/// `/\A[a-zA-Z_][a-zA-Z0-9_]*[!?]?\z/`.
fn is_method_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && *first != b'_' {
        return false;
    }
    let rest = match rest.split_last() {
        Some((b'!' | b'?', head)) => head,
        _ => rest,
    };
    rest.iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}
