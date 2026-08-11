use std::collections::HashSet;
use std::ops::Range;

use tree_sitter::Node;

use super::support::valid_name;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;

/// `MethodIdentifierPredicates::OPERATOR_METHODS`, which `operator_method?` consults. None of
/// these can be spelled in the enforced style, so a `def` for one is never an offense.
const OPERATOR_METHODS: &[&str] = &[
    "|", "^", "&", "<=>", "==", "===", "=~", ">", ">=", "<", "<=", "<<", ">>", "+", "-", "*", "/",
    "%", "**", "~", "+@", "-@", "!@", "~@", "[]", "[]=", "!", "!=", "!~", "`",
];

const ATTRIBUTE_ACCESSORS: &[&str] = &["attr_reader", "attr_writer", "attr_accessor", "attr"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut check = Check {
        context,
        style: context
            .setting("EnforcedStyle")
            .unwrap_or_else(|| "snake_case".to_owned()),
        forbidden: context.setting("ForbiddenIdentifiers").unwrap_or_default(),
        reported: HashSet::new(),
        offenses,
    };
    for node in context.nodes_of_any(&["method", "singleton_method", "alias", "call"]) {
        match node.kind() {
            "method" | "singleton_method" => check.on_def(node),
            "alias" => check.on_alias(node),
            _ => check.on_send(node),
        }
    }
}

struct Check<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    style: String,
    forbidden: Vec<String>,
    /// `Base#add_offense` drops a second offense at a range it already reported, and
    /// `attr_accessor :aB, :cD` reports both names over the same range.
    reported: HashSet<Range<usize>>,
    offenses: &'a mut Vec<Offense>,
}

impl Check<'_, '_> {
    fn on_def(&mut self, node: Node<'_>) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        // The `setter` node of `def foo=` spans `foo=`, which is exactly the name the parser
        // reports, so no node kind here needs its text rebuilt.
        let name = self.context.source.node_text(name_node).to_owned();
        if OPERATOR_METHODS.contains(&name.as_str()) {
            return;
        }
        if self.is_forbidden(&name) {
            self.forbidden_offense(&name, name_node.byte_range());
        } else if !valid_name(&name, &self.style) && !self.class_emitter_method(node, &name) {
            self.style_offense(name_node.byte_range());
        }
    }

    fn on_alias(&mut self, node: Node<'_>) {
        let Some(new_identifier) = node.child_by_field_name("name") else {
            return;
        };
        // `alias foo bar` and `alias :foo :bar` both reach RuboCop as `sym` nodes. An alias
        // between global variables does not, and `on_alias` leaves it alone.
        let Some(name) = self.method_name(new_identifier) else {
            return;
        };
        self.handle_method_name(new_identifier, &name, new_identifier.byte_range());
    }

    fn on_send(&mut self, node: Node<'_>) {
        let Some(method) = node.child_by_field_name("method") else {
            return;
        };
        let method = self.context.source.node_text(method);
        if method == "define_method" || method == "define_singleton_method" {
            self.handle_define_method(node);
        } else if method == "new" && self.receiver_is(node, "Struct") {
            self.handle_new_struct(node);
        } else if method == "define" && self.receiver_is(node, "Data") {
            self.handle_members(node, false);
        } else if method == "alias_method" {
            self.handle_alias_method(node);
        } else if ATTRIBUTE_ACCESSORS.contains(&method) {
            self.handle_attr_accessor(node);
        }
    }

    fn handle_define_method(&mut self, node: Node<'_>) {
        let Some(first) = arguments(node).next() else {
            return;
        };
        let Some(name) = self.literal_name(first) else {
            return;
        };
        // The offense is reported against the call, whose own range stops before any block.
        self.handle_method_name(node, &name, range_position(node));
    }

    fn handle_new_struct(&mut self, node: Node<'_>) {
        // `Struct.new("Name", :a)` names the struct with its first argument, so that one is not
        // a member and is skipped.
        let named = arguments(node)
            .next()
            .is_some_and(|first| matches!(first.kind(), "string" | "bare_string"));
        self.handle_members(node, named);
    }

    fn handle_members(&mut self, node: Node<'_>, skip_first: bool) {
        for member in arguments(node).skip(usize::from(skip_first)) {
            if let Some(name) = self.literal_name(member) {
                self.handle_method_name(member, &name, member.byte_range());
            }
        }
    }

    fn handle_alias_method(&mut self, node: Node<'_>) {
        let arguments: Vec<Node<'_>> = arguments(node).collect();
        if arguments.len() != 2 {
            return;
        }
        let Some(name) = self.literal_name(arguments[0]) else {
            return;
        };
        self.handle_method_name(arguments[0], &name, arguments[0].byte_range());
    }

    fn handle_attr_accessor(&mut self, node: Node<'_>) {
        if node.child_by_field_name("receiver").is_some() {
            return;
        }
        let arguments: Vec<Node<'_>> = arguments(node).collect();
        let Some(&last) = arguments.last() else {
            return;
        };
        for argument in &arguments {
            let Some(name) = self.literal_name(*argument) else {
                continue;
            };
            if self.is_forbidden(&name) {
                // The quirk is upstream's: a forbidden attribute is reported against the *last*
                // argument whichever of them was misnamed.
                self.forbidden_offense(&name, last.byte_range());
            } else if !valid_name(&name, &self.style) {
                self.style_offense(range_position(node));
            }
        }
    }

    fn handle_method_name(&mut self, node: Node<'_>, name: &str, range: Range<usize>) {
        if self.is_forbidden(name) {
            let forbidden_range = if node.kind() == "call" {
                arguments(node)
                    .next()
                    .map_or(range, |first| first.byte_range())
            } else {
                node.byte_range()
            };
            self.forbidden_offense(name, forbidden_range);
        } else if !OPERATOR_METHODS.contains(&name) && !valid_name(name, &self.style) {
            self.style_offense(range);
        }
    }

    fn class_emitter_method(&self, node: Node<'_>, name: &str) -> bool {
        super::support::class_emitter_method(node, name, self.context.source)
    }

    /// Whether the call's receiver is the bare `Struct`/`Data` constant. `(const {nil? cbase} …)`
    /// deliberately excludes a namespaced `Foo::Struct`, which need not be the core class.
    fn receiver_is(&self, node: Node<'_>, constant: &str) -> bool {
        let Some(receiver) = node.child_by_field_name("receiver") else {
            return false;
        };
        match receiver.kind() {
            "constant" => self.context.source.node_text(receiver) == constant,
            "scope_resolution" => {
                receiver.child_by_field_name("scope").is_none()
                    && receiver
                        .child_by_field_name("name")
                        .is_some_and(|name| self.context.source.node_text(name) == constant)
            }
            _ => false,
        }
    }

    /// The name an argument spells, for the arguments RuboCop accepts as `str` or `sym`. An
    /// interpolated string or symbol is a `dstr`/`dsym` upstream and names nothing.
    fn literal_name(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "simple_symbol" => Some(
                self.context
                    .source
                    .node_text(node)
                    .trim_start_matches(':')
                    .to_owned(),
            ),
            "delimited_symbol" | "string" | "bare_string" => self.quoted_content(node),
            _ => None,
        }
    }

    /// Like [`Self::literal_name`], but also accepting the bare-word form `alias foo bar`, which
    /// the parser turns into a `sym` all the same.
    fn method_name(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "identifier" | "constant" | "operator" | "setter" => {
                Some(self.context.source.node_text(node).to_owned())
            }
            _ => self.literal_name(node),
        }
    }

    /// The value RuboCop reads off a `str`/`sym` node: the literal's text with its escapes
    /// resolved. A literal holding an interpolation is a `dstr`/`dsym` upstream, which names
    /// nothing, and is the one shape rejected here.
    fn quoted_content(&self, node: Node<'_>) -> Option<String> {
        super::support::quoted_content(node, self.context.source)
    }

    fn is_forbidden(&self, name: &str) -> bool {
        self.forbidden.iter().any(|forbidden| forbidden == name)
    }

    fn style_offense(&mut self, range: Range<usize>) {
        let message = format!("Use {} for method names.", self.style);
        self.push(message, range);
    }

    fn forbidden_offense(&mut self, name: &str, range: Range<usize>) {
        let message = format!("`{name}` is forbidden, use another method name instead.");
        self.push(message, range);
    }

    fn push(&mut self, message: String, range: Range<usize>) {
        if self.reported.insert(range.clone()) {
            self.offenses.push(self.context.offense(message, range));
        }
    }
}

/// `range_position` for a `send`: one character past the selector, which lands on the first
/// argument whether or not the call was written with parentheses.
fn range_position(node: Node<'_>) -> Range<usize> {
    let Some(method) = node.child_by_field_name("method") else {
        return node.byte_range();
    };
    // The `send` upstream stops before any block, so `foo :bar do … end` must not swallow the
    // block's `end`.
    let end = node
        .child_by_field_name("arguments")
        .map_or(method.end_byte(), |arguments| arguments.end_byte());
    (method.end_byte() + 1).min(end)..end
}

fn arguments<'tree>(node: Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
    node.child_by_field_name("arguments")
        .into_iter()
        .flat_map(|list| {
            let mut cursor = list.walk();
            list.named_children(&mut cursor).collect::<Vec<_>>()
        })
}
