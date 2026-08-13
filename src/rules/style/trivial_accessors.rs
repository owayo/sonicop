use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

/// `AllowedMethods` plus `initialize`, which are never accessors however they are written.
const DEFAULT_ALLOWED: &[&str] = &[
    "to_ary",
    "to_a",
    "to_c",
    "to_enum",
    "to_h",
    "to_hash",
    "to_i",
    "to_int",
    "to_io",
    "to_open",
    "to_path",
    "to_proc",
    "to_r",
    "to_regexp",
    "to_str",
    "to_s",
    "to_sym",
];

/// Which accessor a definition is a hand-written version of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Reader,
    Writer,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
        }
    }
}

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let exact_name_match: bool = context.setting("ExactNameMatch").unwrap_or(true);
    let allow_predicates: bool = context.setting("AllowPredicates").unwrap_or(true);
    let allow_dsl_writers: bool = context.setting("AllowDSLWriters").unwrap_or(true);
    let ignore_class_methods: bool = context.setting("IgnoreClassMethods").unwrap_or(false);
    let allowed: Vec<String> = context.setting("AllowedMethods").unwrap_or_else(|| {
        DEFAULT_ALLOWED
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    });
    for node in context.nodes_of_any(&["method", "singleton_method"]) {
        // `top_level_node?`: a definition with no parent at all is left alone.
        if node
            .parent()
            .is_none_or(|parent| parent.kind() == "program")
            && node.parent().is_some_and(|parent| {
                super::nodes::children(parent).len() == 1 && parent.kind() != "program"
            })
        {
            continue;
        }
        if is_top_level(node) || in_module_or_instance_eval(context, node) {
            continue;
        }
        if ignore_class_methods && node.kind() == "singleton_method" {
            continue;
        }
        let Some(name) = node.child_by_field_name("name") else {
            continue;
        };
        let method = context.source.node_text(name);
        let Some(kind) = accessor_kind(
            context,
            node,
            method,
            &allowed,
            exact_name_match,
            allow_predicates,
            allow_dsl_writers,
        ) else {
            continue;
        };
        let Some(keyword) = node.child(0).filter(|child| child.kind() == "def") else {
            continue;
        };
        let mut offense = context.offense(
            format!(
                "Use `attr_{}` to define trivial {} methods.",
                kind.as_str(),
                kind.as_str()
            ),
            keyword.byte_range(),
        );
        if let Some(edit) = rewrite(
            context,
            node,
            method,
            kind,
            allow_dsl_writers,
            &allowed,
            exact_name_match,
            allow_predicates,
        ) {
            offense = offense.corrected_by(edit);
        }
        offenses.push(offense);
    }
}

/// `top_level_node?`: `node.parent.nil?`, which is only true for the sole expression of a file.
fn is_top_level(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    parent.kind() == "program" && super::nodes::children(parent).len() == 1
}

/// `in_module_or_instance_eval?`: an accessor is only worth suggesting where one would be defined.
fn in_module_or_instance_eval(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "class" | "singleton_class" => return false,
            "module" => return true,
            "block" | "do_block" => {
                let method = parent
                    .parent()
                    .and_then(|call| call.child_by_field_name("method"))
                    .map(|selector| context.source.node_text(selector));
                if method == Some("instance_eval") {
                    return true;
                }
            }
            _ => {}
        }
        current = parent.parent();
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn accessor_kind(
    context: &RuleContext<'_>,
    node: Node<'_>,
    method: &str,
    allowed: &[String],
    exact_name_match: bool,
    allow_predicates: bool,
    allow_dsl_writers: bool,
) -> Option<Kind> {
    if trivial_reader(
        context,
        node,
        method,
        allowed,
        exact_name_match,
        allow_predicates,
    ) {
        return Some(Kind::Reader);
    }
    trivial_writer(
        context,
        node,
        method,
        allowed,
        exact_name_match,
        allow_dsl_writers,
    )
    .then_some(Kind::Writer)
}

fn trivial_reader(
    context: &RuleContext<'_>,
    node: Node<'_>,
    method: &str,
    allowed: &[String],
    exact_name_match: bool,
    allow_predicates: bool,
) -> bool {
    let Some(body) = single_body(node) else {
        return false;
    };
    if parameters(node).is_some() || body.kind() != "instance_variable" {
        return false;
    }
    if allowed_method_name(context, node, method, allowed, exact_name_match) {
        return false;
    }
    !(allow_predicates && method.ends_with('?'))
}

fn trivial_writer(
    context: &RuleContext<'_>,
    node: Node<'_>,
    method: &str,
    allowed: &[String],
    exact_name_match: bool,
    allow_dsl_writers: bool,
) -> bool {
    if !looks_like_trivial_writer(context, node) {
        return false;
    }
    if allowed_method_name(context, node, method, allowed, exact_name_match) {
        return false;
    }
    // `dsl_writer?`: a writer whose name does not end in `=` reads as a DSL setting.
    !(allow_dsl_writers && !method.ends_with('='))
}

/// `{(def _ (args (arg ...)) (ivasgn _ (lvar _))) ...}`.
fn looks_like_trivial_writer(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parameters) = parameters(node) else {
        return false;
    };
    let written = super::nodes::children(parameters);
    let [only] = written.as_slice() else {
        return false;
    };
    if only.kind() != "identifier" {
        return false;
    }
    let Some(body) = single_body(node) else {
        return false;
    };
    body.kind() == "assignment"
        && body
            .child_by_field_name("left")
            .is_some_and(|left| left.kind() == "instance_variable")
        && body
            .child_by_field_name("right")
            .is_some_and(|right| right.kind() == "identifier")
        && !super::nodes::is_match_assignment(body, context.source.text())
}

fn allowed_method_name(
    context: &RuleContext<'_>,
    node: Node<'_>,
    method: &str,
    allowed: &[String],
    exact_name_match: bool,
) -> bool {
    if method == "initialize" || allowed.iter().any(|name| name == method) {
        return true;
    }
    exact_name_match && !names_match(context, node, method)
}

/// `names_match?`: the method and the instance variable name the same thing.
fn names_match(context: &RuleContext<'_>, node: Node<'_>, method: &str) -> bool {
    let Some(body) = single_body(node) else {
        return false;
    };
    let variable = match body.kind() {
        "instance_variable" => body,
        "assignment" => match body.child_by_field_name("left") {
            Some(left) if left.kind() == "instance_variable" => left,
            _ => return false,
        },
        _ => return false,
    };
    let name = context.source.node_text(variable);
    method.trim_end_matches(['=', '?']) == &name[1..]
}

/// The parameters a definition declares, or nothing when it declares none.
fn parameters<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let parameters = node.child_by_field_name("parameters")?;
    (!super::nodes::children(parameters).is_empty()).then_some(parameters)
}

/// The single expression a definition's body holds.
fn single_body<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let body = node.child_by_field_name("body")?;
    if body.kind() != "body_statement" {
        return Some(body);
    }
    match super::nodes::children(body).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite(
    context: &RuleContext<'_>,
    node: Node<'_>,
    method: &str,
    kind: Kind,
    allow_dsl_writers: bool,
    allowed: &[String],
    exact_name_match: bool,
    allow_predicates: bool,
) -> Option<Edit> {
    // A definition used as an argument -- `private def foo` -- is not replaced.
    if node
        .parent()
        .is_some_and(|parent| matches!(parent.kind(), "argument_list") || parent.kind() == "call")
    {
        return None;
    }
    // `trivial_accessor_kind`: a DSL writer is reported but never rewritten.
    let kind = match kind {
        Kind::Writer if !method.ends_with('=') => trivial_reader(
            context,
            node,
            method,
            allowed,
            exact_name_match,
            allow_predicates,
        )
        .then_some(Kind::Reader)?,
        other => other,
    };
    let _ = allow_dsl_writers;
    if !names_match(context, node, method) || method.ends_with('?') {
        return None;
    }
    let accessor = format!("attr_{} :{}", kind.as_str(), method.trim_end_matches('='));
    let replacement = match node.kind() {
        "singleton_method" => {
            if node
                .child_by_field_name("object")
                .is_none_or(|object| object.kind() != "self")
            {
                return None;
            }
            let indent = " ".repeat(context.source.line_column(node.start_byte()).1 - 1);
            format!("class << self\n{indent}  {accessor}\n{indent}end")
        }
        _ => accessor,
    };
    Some(Edit {
        start: node.start_byte(),
        end: node.end_byte(),
        replacement,
        safe: true,
    })
}
