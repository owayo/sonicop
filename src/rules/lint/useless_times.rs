use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::send_node::{arguments, is_plain_send, named_children, symbol_name};

use super::blocks::{BLOCK_KINDS, BlockArgs};
use super::locals::LocalVariables;
use super::ranges::whole_lines;
use super::statements::statements;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let locals = LocalVariables::new(context);
    for call in context.nodes_of("call") {
        if !is_plain_send(call, context) {
            continue;
        }
        if call
            .field("method")
            .is_none_or(|method| context.source.node_text(method) != "times")
        {
            continue;
        }
        // `(send (int $_) :times (block-pass (sym $_))?)`: a literal count, and at most a symbol
        // handed over as the block.
        // `(int $_)`: a sign written in front of a literal is folded into it by the parser, so
        // `-2.times` is a call on the integer `-2` rather than on a negation of `2`.
        let Some(receiver) = call.field("receiver").and_then(signed_integer) else {
            continue;
        };
        let Ok(count) = context
            .source
            .node_text(receiver)
            .replace([' ', '_'], "")
            .parse::<i64>()
        else {
            continue;
        };
        if count > 1 {
            continue;
        }
        let call_arguments = arguments(call);
        let proc_name = match call_arguments.as_slice() {
            [] => None,
            [only] => match block_pass_symbol(only.first(), context) {
                Some(name) => Some(name),
                None => continue,
            },
            _ => continue,
        };
        let block = call.field("block");
        let node = block.map_or(call, |_| call);
        let range = node.byte_range();
        let mut offense =
            context.offense(format!("Useless call to `{count}.times` detected."), range);
        if let Some(edit) = correction(call, block, count, proc_name, context, &locals) {
            offense = offense.corrected_by(edit);
        }
        offenses.push(offense);
    }
}

/// `(block-pass (sym $_))`: `1.times(&:foo)` names the block rather than writing one.
fn block_pass_symbol<'a>(node: Node<'_>, context: &'a RuleContext<'_>) -> Option<&'a str> {
    if node.kind_str() != "block_argument" {
        return None;
    }
    symbol_name(named_children(node).first().copied()?, context)
}

fn correction(
    call: Node<'_>,
    block: Option<Node<'_>>,
    count: i64,
    proc_name: Option<&str>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Edit> {
    let node = call;
    // Nothing is rewritten unless the call stands alone: anything else on the line, or a call it
    // is the receiver of, would be left dangling.
    if !own_line(node, context) || node.parent_of(context).is_some_and(is_upstream_send) {
        return None;
    }
    let body = block
        .filter(|block| BLOCK_KINDS.contains(&block.kind_str()))
        .and_then(|block| block.field("body"))
        .filter(|body| !statements(*body).is_empty());
    // `never_process?`: a count below one, or a block with nothing in it, never runs at all.
    if count < 1 || (block.is_some() && body.is_none()) {
        let range = whole_lines(node.byte_range(), context);
        return Some(Edit {
            start: range.start,
            end: range.end,
            replacement: String::new(),
            safe: true,
        });
    }
    if let Some(name) = proc_name {
        return Some(Edit {
            start: node.start_byte(),
            end: node.end_byte(),
            replacement: name.to_owned(),
            safe: true,
        });
    }
    let block = block.filter(|block| BLOCK_KINDS.contains(&block.kind_str()))?;
    let body = body?;
    reduce_to_body(node, block, body, context, locals)
}

/// `autocorrect_block`: the block runs once, so it can be replaced by its body -- with the block
/// parameter, which would be `0` on that single pass, substituted for.
fn reduce_to_body(
    node: Node<'_>,
    block: Node<'_>,
    body: Node<'_>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
) -> Option<Edit> {
    // `|;a|` declares a block-local variable, which is a `shadowarg` upstream: it counts as an
    // argument the body may read but is no name the count could be substituted for.
    if block
        .field("parameters")
        .and_then(|parameters| parameters.field("locals"))
        .is_some()
    {
        return None;
    }
    let args = BlockArgs::of(block, context, locals);
    let parameter = match &args {
        BlockArgs::Written(params) if params.is_empty() => None,
        BlockArgs::Written(params) if params.len() == 1 && params[0].kind_str() == "identifier" => {
            Some(context.source.node_text(params[0]))
        }
        // More than one parameter, or one that cannot be substituted for, leaves the body
        // referring to something the rewrite would not define.
        _ => return None,
    };
    if orphans_loop_control_keyword(body) {
        return None;
    }
    if let Some(name) = parameter
        && reassigns(body, name, context)
    {
        return None;
    }
    let statements = statements(body);
    let first = statements.first()?;
    let last = statements.last()?;
    let source = context.source.slice(first.start_byte()..last.end_byte());
    let source = match parameter {
        Some(name) => substitute(source, name),
        None => source.to_owned(),
    };
    let column = context.source.line_column(node.start_byte()).1 - 1;
    let body_column = context.source.line_column(first.start_byte()).1 - 1;
    Some(Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement: fix_indentation(&source, column, body_column),
        safe: true,
    })
}

/// `source.gsub!(/\b#{block_arg}\b/, '0')`.
fn substitute(source: &str, name: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < source.len() {
        if source[index..].starts_with(name)
            && !is_word_byte(index.checked_sub(1).map(|before| bytes[before]))
            && !is_word_byte(bytes.get(index + name.len()).copied())
        {
            out.push('0');
            index += name.len();
            continue;
        }
        let character = source[index..].chars().next().expect("index is a boundary");
        out.push(character);
        index += character.len_utf8();
    }
    out
}

fn is_word_byte(byte: Option<u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// `fix_indentation`: every line but the first loses the indentation the block body carried.
fn fix_indentation(source: &str, column: usize, body_column: usize) -> String {
    let mut lines = source.split('\n');
    let mut out = lines.next().unwrap_or_default().to_owned();
    for line in lines {
        out.push('\n');
        if line.is_empty() {
            continue;
        }
        // `line[range] = ''` deletes the characters in `column...body_column`, and does nothing
        // when the line is shorter than the range starts.
        let width = body_column.saturating_sub(column);
        let cut: String = line
            .char_indices()
            .filter(|(offset, _)| {
                let index = line[..*offset].chars().count();
                index < column || index >= column + width
            })
            .map(|(_, character)| character)
            .collect();
        out.push_str(&cut);
    }
    out
}

/// `orphans_loop_control_keyword?`: a `next` bound to this block would have nothing to bind to.
fn orphans_loop_control_keyword(node: Node<'_>) -> bool {
    let mut found = false;
    walk_keywords(node, &mut found);
    found
}

fn walk_keywords(node: Node<'_>, found: &mut bool) {
    for child in named_children(node) {
        if matches!(child.kind_str(), "next" | "break" | "redo") {
            *found = true;
            return;
        }
        if BLOCK_KINDS.contains(&child.kind_str())
            || matches!(
                child.kind_str(),
                "lambda" | "while" | "until" | "for" | "while_modifier" | "until_modifier"
            )
        {
            continue;
        }
        walk_keywords(child, found);
        if *found {
            return;
        }
    }
}

/// `block_reassigns_arg?`: an assignment to the parameter makes `0` the wrong substitution.
fn reassigns(node: Node<'_>, name: &str, context: &RuleContext<'_>) -> bool {
    if node.kind_str() == "assignment"
        && node.field("left").is_some_and(|left| {
            left.kind_str() == "identifier" && context.source.node_text(left) == name
        })
    {
        return true;
    }
    named_children(node)
        .into_iter()
        .any(|child| reassigns(child, name, context))
}

/// `own_line?`: nothing but whitespace stands before the call on its line.
fn own_line(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let text = context.source.text();
    let start = text[..node.start_byte()]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    text[start..node.start_byte()]
        .chars()
        .all(char::is_whitespace)
}

/// `node.parent&.send_type?`: the call is the receiver or an argument of another call.
fn is_upstream_send(node: Node<'_>) -> bool {
    matches!(
        node.kind_str(),
        "call" | "binary" | "unary" | "element_reference"
    )
}

/// The integer a receiver is, with a sign the parser folded into it taken along.
fn signed_integer<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    match node.kind_str() {
        "integer" => Some(node),
        "unary" => node
            .field("operand")
            .filter(|operand| operand.kind_str() == "integer")
            .map(|_| node),
        _ => None,
    }
}
