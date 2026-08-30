use std::collections::HashMap;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::locals::LocalVariables;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::all_children_of;

/// One entry of `Methods`, which upstream writes as a list of single-key hashes.
type Methods = Vec<HashMap<String, Vec<String>>>;

/// A one-line block on one of `Methods` whose parameters are not named the way the configuration
/// asks for.
pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let Some(methods) = context.setting::<Methods>("Methods") else {
        return;
    };
    let locals = LocalVariables::new(context);
    for node in context.nodes_of("call") {
        // `eligible_method?`: the receiver has to be written out.
        let (Some(receiver), Some(selector)) = (node.field("receiver"), node.field("method")) else {
            continue;
        };
        let _ = receiver;
        let method = context.source.node_text(selector);
        let Some(expected) = methods
            .iter()
            .find_map(|entry| entry.get(method))
            .filter(|names| !names.is_empty())
        else {
            continue;
        };
        let Some(block) = node.field("block") else {
            continue;
        };
        // `node.single_line?` on a block is about its braces.
        if context.source.line_column(block.start_byte()).0
            != context.source.line_column(block.end_byte()).0
        {
            continue;
        }
        let Some(list) = block.field("parameters") else {
            continue;
        };
        // `eligible_arguments?`: every parameter has to be a plain one. A block-local written
        // after a `;` is a `shadowarg` upstream, which fails `arg_type?` -- the grammar spells it
        // as another identifier in the same list and marks it only by the separator.
        let written = super::nodes::children_in(list, context);
        if written.is_empty() || written.iter().any(|arg| arg.kind_str() != "identifier") {
            continue;
        }
        let _cursor = list.walk();
        if all_children_of(list, context)
            .into_iter()
            .any(|child| !child.is_named() && child.kind_str() == ";")
        {
            continue;
        }
        let names: Vec<&str> = written
            .iter()
            .map(|arg| context.source.node_text(*arg))
            .collect();
        // `args_match?`: the names match once their leading underscores are dropped.
        let stripped: Vec<&str> = names
            .iter()
            .map(|name| name.trim_start_matches('_'))
            .collect();
        if stripped
            .iter()
            .zip(expected.iter())
            .filter(|_| true)
            .count()
            == stripped.len()
            && stripped
                .iter()
                .zip(expected.iter())
                .all(|(actual, wanted)| actual == wanted)
            && stripped.len() <= expected.len()
        {
            continue;
        }
        // `build_preferred_arguments_map`: an argument past the end of the list maps to nothing,
        // which is what leaves the trailing `, ` in the message upstream prints.
        let preferred: Vec<String> = names
            .iter()
            .enumerate()
            .map(|(index, name)| match expected.get(index) {
                Some(wanted) if name.starts_with('_') => format!("_{wanted}"),
                Some(wanted) => wanted.clone(),
                None => String::new(),
            })
            .collect();
        let joined = preferred.join(", ");
        let mut edits = vec![Edit {
            start: list.start_byte(),
            end: list.end_byte(),
            replacement: format!("|{joined}|"),
            safe: true,
        }];
        // `node.each_descendant(:lvar)`: the reads of the renamed parameters follow the names.
        let renames: HashMap<&str, &String> = names
            .iter()
            .zip(preferred.iter())
            .filter(|(_, wanted)| !wanted.is_empty())
            .map(|(name, wanted)| (*name, wanted))
            .collect();
        rename_reads(block, &renames, context, &locals, &mut edits);
        offenses.push(
            context
                .offense(
                    format!("Name `{method}` block params `|{joined}|`."),
                    list.byte_range(),
                )
                .corrected_by_all(edits),
        );
    }
}

/// Replaces every local variable read that names one of the renamed parameters.
fn rename_reads(
    node: Node<'_>,
    renames: &HashMap<&str, &String>,
    context: &RuleContext<'_>,
    locals: &LocalVariables<'_, '_>,
    edits: &mut Vec<Edit>,
) {
    let mut stack: Vec<Node<'_>> = super::nodes::children_in(node, context)
        .into_iter()
        .filter(|child| child.kind_str() != "block_parameters")
        .collect();
    while let Some(current) = stack.pop() {
        stack.extend(super::nodes::children_in(current, context));
        if current.kind_str() != "identifier" || !locals.is_lvar(current) {
            continue;
        }
        if let Some(wanted) = renames.get(context.source.node_text(current)) {
            edits.push(Edit {
                start: current.start_byte(),
                end: current.end_byte(),
                replacement: (*wanted).clone(),
                safe: true,
            });
        }
    }
}
