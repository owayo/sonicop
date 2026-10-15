//! ProjectIndexHelp の AST 側の読み替え。名前解決そのものは rubydex に任せる。

use crate::project_index::{DeclarationId, Index};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node::{has_interpolation, named_children_of};
use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

pub(crate) fn constant_name(mut node: Node<'_>, context: &RuleContext<'_>) -> Option<String> {
    let mut parts = Vec::new();
    loop {
        match node.kind_str() {
            "constant" => {
                parts.push(context.source.node_text(node));
                break;
            }
            "scope_resolution" => {
                parts.push(context.source.node_text(node.field("name")?));
                match node.field("scope") {
                    Some(scope) => node = scope,
                    None => break,
                }
            }
            _ => return None,
        }
    }
    parts.reverse();
    Some(parts.join("::"))
}

fn absolute_constant(mut node: Node<'_>) -> bool {
    while node.kind_str() == "scope_resolution" {
        let Some(scope) = node.field("scope") else {
            return true;
        };
        node = scope;
    }
    false
}

pub(crate) fn nesting(node: Node<'_>, context: &RuleContext<'_>) -> Vec<String> {
    let mut scopes = Vec::new();
    let mut child = node;
    while let Some(parent) = child.parent_of(context) {
        if matches!(parent.kind_str(), "class" | "module")
            && parent
                .field("body")
                .is_some_and(|body| body.id() == child.id())
            && let Some(name) = parent
                .field("name")
                .and_then(|name| constant_name(name, context))
        {
            scopes.push(name);
        }
        child = parent;
    }
    scopes.reverse();
    scopes
}

pub(crate) fn resolve_constant(
    index: &mut Index<'_>,
    node: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<DeclarationId> {
    let name = constant_name(node, context)?;
    let scopes = if absolute_constant(node) {
        Vec::new()
    } else {
        nesting(node, context)
    };
    let mut segments = name.trim_start_matches("::").split("::");
    let mut declaration = index.resolve_constant(segments.next()?, &scopes)?;
    for segment in segments {
        declaration = index.find_member(declaration, segment)?;
    }
    Some(declaration)
}

pub(super) fn constant_reference(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return true;
    };
    if parent.kind_str() == "superclass" {
        return false;
    }
    if parent.kind_str() == "scope_resolution"
        && parent.parent_of(context).is_some_and(|assignment| {
            assignment.kind_str() == "assignment"
                && assignment
                    .field("left")
                    .is_some_and(|left| left.id() == parent.id())
                && assignment.field("right").is_some_and(|right| {
                    right.kind_str() == "call"
                        && right.field("receiver").is_some_and(|receiver| {
                            crate::rules::send_node::top_level_constant(receiver, "Class", context)
                                || crate::rules::send_node::top_level_constant(
                                    receiver, "Module", context,
                                )
                        })
                        && right
                            .field("method")
                            .is_some_and(|method| context.source.node_text(method) == "new")
                })
        })
    {
        return false;
    }
    let defining_field = match parent.kind_str() {
        "scope_resolution" | "method" | "singleton_method" | "class" | "module" => "name",
        "assignment" | "operator_assignment" => "left",
        "call" => "method",
        "for" => "pattern",
        "exception_variable"
        | "left_assignment_list"
        | "rest_assignment"
        | "destructured_left_assignment" => return false,
        _ => return true,
    };
    !parent
        .field(defining_field)
        .is_some_and(|name| name.id() == node.id())
}

pub(super) struct MethodCall<'tree> {
    pub name: String,
    pub selector: std::ops::Range<usize>,
    pub receiver: Option<Node<'tree>>,
}

pub(super) fn setter_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut child = node;
    while let Some(parent) = child.parent_of(context) {
        match parent.kind_str() {
            "left_assignment_list" | "destructured_left_assignment" | "rest_assignment" => {
                child = parent
            }
            "assignment" => {
                return parent
                    .field("left")
                    .is_some_and(|left| left.id() == child.id());
            }
            _ => return false,
        }
    }
    false
}

pub(super) fn method_call<'tree>(
    node: Node<'tree>,
    context: &RuleContext<'_>,
) -> Option<MethodCall<'tree>> {
    let setter = setter_call(node, context);
    let (mut name, selector, receiver) = match node.kind_str() {
        "call" => {
            let method = node.field("method")?;
            if method.kind_str() == "super" {
                return None;
            }
            (
                context.source.node_text(method).to_owned(),
                method.byte_range(),
                node.field("receiver"),
            )
        }
        "identifier" if bare_call(node, context) => (
            context.source.node_text(node).to_owned(),
            node.byte_range(),
            None,
        ),
        "element_reference" => {
            let object = node.field("object")?;
            let children: Vec<_> =
                crate::rules::send_node::all_children_iter(node, context).collect();
            let open = children.iter().find(|child| child.kind_str() == "[")?;
            let close = children
                .iter()
                .rev()
                .find(|child| child.kind_str() == "]")?;
            (
                "[]".to_owned(),
                open.start_byte()..close.end_byte(),
                Some(object),
            )
        }
        "binary" => {
            let operator = node.field("operator")?;
            let name = context.source.node_text(operator);
            if matches!(name, "&&" | "||" | "and" | "or") {
                return None;
            }
            (name.to_owned(), operator.byte_range(), node.field("left"))
        }
        "unary" => {
            let operator = node.field("operator")?;
            let name = match context.source.node_text(operator) {
                "+" => "+@",
                "-" => "-@",
                "not" => "!",
                "defined?" => return None,
                name => name,
            };
            (
                name.to_owned(),
                operator.byte_range(),
                node.field("operand"),
            )
        }
        _ => return None,
    };
    if setter {
        name.push('=');
    }
    Some(MethodCall {
        name,
        selector,
        receiver,
    })
}

pub(crate) fn ancestors<'tree>(
    node: Node<'tree>,
    context: &'tree RuleContext<'_>,
) -> Vec<Node<'tree>> {
    let mut ancestors = Vec::new();
    let mut current = node;
    while let Some(parent) = current.parent_of(context) {
        ancestors.push(parent);
        current = parent;
    }
    ancestors
}

pub(super) fn enclosing_namespace(
    index: &mut Index<'_>,
    node: Node<'_>,
    context: &RuleContext<'_>,
) -> Option<DeclarationId> {
    for ancestor in ancestors(node, context) {
        if matches!(ancestor.kind_str(), "class" | "module") {
            return resolve_constant(index, ancestor.field("name")?, context);
        }
    }
    index.lookup("Object")
}

pub(crate) fn overrides_inherited_method(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let Some(project) = context.project_index() else {
        return false;
    };
    project.with(|index| {
        let Some(namespace) = ancestors(node, context)
            .into_iter()
            .find(|node| matches!(node.kind_str(), "class" | "module"))
        else {
            return false;
        };
        let Some(mut owner) = namespace
            .field("name")
            .and_then(|name| resolve_constant(index, name, context))
        else {
            return false;
        };
        if node.kind_str() == "singleton_method" {
            let Some(singleton) = index.singleton(owner) else {
                return false;
            };
            owner = singleton;
        }
        node.field("name").is_some_and(|name| {
            index.inherited_member(owner, &format!("{}()", context.source.node_text(name)))
        })
    })
}

pub(super) fn singleton_context(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut instance_method = false;
    for ancestor in ancestors(node, context) {
        match ancestor.kind_str() {
            "singleton_method" | "singleton_class" => return true,
            "method" => instance_method = true,
            "class" | "module" => return !instance_method,
            _ => {}
        }
    }
    false
}

pub(super) fn bare_call(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    if matches!(
        context.source.node_text(node),
        "__FILE__" | "__LINE__" | "__ENCODING__"
    ) {
        return false;
    }
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    let defining_field = match parent.kind_str() {
        "assignment" | "operator_assignment" => "left",
        "call" => "method",
        "method" | "singleton_method" | "optional_parameter" | "keyword_parameter" => "name",
        "for" => "pattern",
        _ => "",
    };
    if !defining_field.is_empty()
        && parent
            .field(defining_field)
            .is_some_and(|name| name.id() == node.id())
    {
        return false;
    }
    if matches!(
        parent.kind_str(),
        "block_parameters"
            | "method_parameters"
            | "lambda_parameters"
            | "splat_parameter"
            | "hash_splat_parameter"
            | "block_parameter"
            | "alias"
            | "undef"
            | "left_assignment_list"
            | "destructured_left_assignment"
            | "rest_assignment"
    ) {
        return false;
    }
    !super::locals::LocalVariables::new(context).is_lvar(node)
}

pub(super) fn within_deprecated_definition(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    ancestors(node, context)
        .into_iter()
        .filter(|node| {
            matches!(
                node.kind_str(),
                "method" | "singleton_method" | "class" | "module"
            )
        })
        .any(|definition| {
            // 本文の後のコメントは定義へ、最後の式と同じ行のコメントはその式へ関連付く。
            let body: Vec<_> = definition
                .field("body")
                .map(|body| {
                    named_children_of(body, context)
                        .into_iter()
                        .filter(|child| child.kind_str() != "comment")
                        .collect()
                })
                .unwrap_or_default();
            let last = body
                .last()
                .copied()
                .or_else(|| definition.field("parameters"))
                .or_else(|| definition.field("name"));
            if let Some(last) = last {
                let last_line = context
                    .source
                    .line_column(last.end_byte().saturating_sub(1))
                    .0;
                let definition_line = context
                    .source
                    .line_column(definition.end_byte().saturating_sub(1))
                    .0;
                if context.comment_ranges().iter().any(|range| {
                    let line = context.source.line_column(range.start).0;
                    ((range.start >= last.end_byte()
                        && range.end <= definition.end_byte()
                        && line > last_line)
                        || (range.start >= definition.end_byte() && line == definition_line))
                        && context.source.slice(range.clone()).contains("@deprecated")
                }) {
                    return true;
                }
            }
            let mut cursor = definition.start_byte();
            for range in context
                .comment_ranges()
                .iter()
                .rev()
                .filter(|range| range.end <= definition.start_byte())
            {
                if !context.source.slice(range.end..cursor).trim().is_empty() {
                    break;
                }
                let line = context.source.line_column(range.start).0;
                if !context
                    .source
                    .slice(context.source.line_start(line)..range.start)
                    .trim()
                    .is_empty()
                {
                    break;
                }
                if context.source.slice(range.clone()).contains("@deprecated") {
                    return true;
                }
                cursor = range.start;
            }
            false
        })
}

pub(super) fn literal_names(context: &RuleContext<'_>, setter: bool) -> HashSet<String> {
    let mut names = HashSet::new();
    let heredoc_quotes: HashMap<_, _> = context
        .nodes_of("heredoc_body")
        .zip(context.nodes_of("heredoc_beginning"))
        .map(|(body, opener)| {
            (
                body.id(),
                context
                    .source
                    .node_text(opener)
                    .trim_start_matches("<<")
                    .trim_start_matches(['~', '-'])
                    .starts_with('\''),
            )
        })
        .collect();
    for node in context.nodes_of_any(&[
        "simple_symbol",
        "delimited_symbol",
        "hash_key_symbol",
        "bare_symbol",
        "string",
        "bare_string",
        "character",
        "heredoc_body",
        "regex",
    ]) {
        match node.kind_str() {
            "simple_symbol" => {
                names.insert(
                    context
                        .source
                        .node_text(node)
                        .trim_start_matches(':')
                        .to_owned(),
                );
            }
            "hash_key_symbol" => {
                names.insert(context.source.node_text(node).to_owned());
            }
            "delimited_symbol" if !has_interpolation(node) => {
                names.insert(crate::rules::ruby_literal::string_value(node, context));
            }
            "character" => collect_identifiers(
                &crate::rules::ruby_literal::character_value(context.source.node_text(node)),
                setter,
                &mut names,
            ),
            "string" if !has_interpolation(node) => collect_identifiers(
                &String::from_utf8_lossy(
                    &crate::rules::literal::node_bytes(context, node).unwrap_or_default(),
                ),
                setter,
                &mut names,
            ),
            "string" => {
                let mut fragment = String::new();
                for part in named_children_of(node, context) {
                    if matches!(part.kind_str(), "string_content" | "escape_sequence") {
                        fragment.push_str(context.source.node_text(part));
                    } else {
                        let value =
                            crate::rules::literal::double_quoted_fragment(context, &fragment);
                        collect_identifiers(&String::from_utf8_lossy(&value), setter, &mut names);
                        fragment.clear();
                    }
                }
                let value = crate::rules::literal::double_quoted_fragment(context, &fragment);
                collect_identifiers(&String::from_utf8_lossy(&value), setter, &mut names);
            }
            "bare_string" | "bare_symbol" => {
                let mut fragments = Vec::new();
                let mut fragment = String::new();
                for part in named_children_of(node, context) {
                    if matches!(part.kind_str(), "string_content" | "escape_sequence") {
                        fragment.push_str(context.source.node_text(part));
                    } else {
                        fragments.push(std::mem::take(&mut fragment));
                    }
                }
                fragments.push(fragment);
                for fragment in fragments {
                    let value = crate::rules::literal::word_fragment(context, node, &fragment);
                    let text = String::from_utf8_lossy(&value);
                    if node.kind_str() == "bare_symbol" && !has_interpolation(node) {
                        names.insert(text.into_owned());
                    } else {
                        collect_identifiers(&text, setter, &mut names);
                    }
                }
            }
            "heredoc_body" => {
                let single = heredoc_quotes.get(&node.id()).copied().unwrap_or(false);
                let mut fragment = String::new();
                for part in named_children_of(node, context) {
                    if matches!(part.kind_str(), "heredoc_content" | "escape_sequence") {
                        fragment.push_str(context.source.node_text(part));
                    } else {
                        let value = if single {
                            fragment.as_bytes().to_vec()
                        } else {
                            crate::rules::literal::double_quoted_fragment(context, &fragment)
                        };
                        collect_identifiers(&String::from_utf8_lossy(&value), setter, &mut names);
                        fragment.clear();
                    }
                }
            }
            _ => {
                // 補間の両側を結合すると、Ruby の別々の str ノードにはない識別子を作ってしまう。
                for part in named_children_of(node, context)
                    .into_iter()
                    .filter(|part| matches!(part.kind_str(), "string_content" | "heredoc_body"))
                {
                    collect_identifiers(context.source.node_text(part), setter, &mut names);
                }
            }
        }
    }
    names
}

fn collect_identifiers(text: &str, setter: bool, names: &mut HashSet<String>) {
    let mut characters = text.char_indices().peekable();
    while let Some((start, character)) = characters.next() {
        if !character.is_ascii_alphabetic() && character != '_' {
            continue;
        }
        let mut end = start + character.len_utf8();
        while let Some(&(position, next)) = characters.peek() {
            if next.is_ascii_alphanumeric() || next == '_' {
                end = position + next.len_utf8();
                characters.next();
            } else {
                break;
            }
        }
        if let Some(&(position, next)) = characters.peek()
            && (matches!(next, '?' | '!') || setter && next == '=')
        {
            end = position + next.len_utf8();
            characters.next();
        }
        names.insert(text[start..end].to_owned());
    }
}
