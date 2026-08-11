use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::diagnostic::Offense;
use crate::rules::RuleContext;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let mut tracker = Tracker {
        context,
        // Only the line of the earlier definition is ever read back, so the node itself does not
        // have to be kept alive alongside the key.
        definitions: HashMap::new(),
        scopes: HashMap::new(),
        self_aliased: HashSet::new(),
    };
    for node in context.nodes_of_any(&["method", "singleton_method", "alias", "call"]) {
        match node.kind() {
            "method" => tracker.on_def(node, offenses),
            "singleton_method" => tracker.on_defs(node, offenses),
            "alias" => tracker.on_alias(node, offenses),
            _ => tracker.on_send(node, offenses),
        }
    }
}

struct Tracker<'a> {
    context: &'a RuleContext<'a>,
    definitions: HashMap<String, usize>,
    /// Keys already redefined once inside a `rescue` or `ensure`. A body with such a clause is
    /// allowed one redefinition -- that is how a conditional fallback definition is written -- but
    /// no more.
    scopes: HashMap<&'static str, HashSet<String>>,
    /// Names marked by the `alias foo foo` trick, which declares a redefinition intentional.
    self_aliased: HashSet<String>,
}

impl<'a> Tracker<'a> {
    fn on_def(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        // A definition under an `if` is very likely a platform-specific alternative, so both
        // branches are left alone.
        if has_if_ancestor(node) {
            return;
        }
        let Some(name) = self.name_text(node) else {
            return;
        };
        self.found_instance_method(node, &name, offenses);
    }

    fn on_defs(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        if has_if_ancestor(node) {
            return;
        }
        let (Some(name), Some(receiver)) =
            (self.name_text(node), node.child_by_field_name("object"))
        else {
            return;
        };
        match receiver.kind() {
            "constant" | "scope_resolution" => {
                let receiver_name = self.text(receiver);
                if let Some(qualified) = self.lookup_constant(node, receiver_name) {
                    self.found_method(node, &format!("{qualified}.{name}"), None, offenses);
                }
            }
            "self" => self.check_self_receiver(node, &name, offenses),
            _ => {}
        }
    }

    fn on_alias(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        let (Some(name), Some(original)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("alias"),
        ) else {
            return;
        };
        let (Some(name), Some(original)) = (
            symbol_name(self.text(name)),
            symbol_name(self.text(original)),
        ) else {
            return;
        };
        if name == original {
            self.track_self_alias(node, name);
            return;
        }
        if has_if_ancestor(node) {
            return;
        }
        self.found_instance_method(node, name, offenses);
    }

    fn on_send(&mut self, node: Node<'_>, offenses: &mut Vec<Offense>) {
        // A macro that defines methods never has a receiver, and `alias_method` aside, all of them
        // take plain symbol or string arguments.
        if node.child_by_field_name("receiver").is_some() {
            return;
        }
        let Some(method) = node.child_by_field_name("method") else {
            return;
        };
        let arguments = literal_arguments(self.context, node);
        match self.text(method) {
            "alias_method" => {
                let [name, original] = arguments.as_slice() else {
                    return;
                };
                if name == original {
                    self.track_self_alias(node, name);
                    return;
                }
                if !has_if_ancestor(node) {
                    let name = name.clone();
                    self.found_instance_method(node, &name, offenses);
                }
            }
            accessor @ ("attr" | "attr_reader" | "attr_writer" | "attr_accessor") => {
                if arguments.is_empty() || !in_macro_scope(node) {
                    return;
                }
                self.on_attr(node, accessor, &arguments, offenses);
            }
            // `def_delegator :target, :method` or `def_delegator :target, :method, :alias`;
            // the defined name is the last argument either way.
            "def_delegator" | "def_instance_delegator"
                if arguments.len() >= 2 && !has_if_ancestor(node) =>
            {
                let name = arguments[arguments.len() - 1].clone();
                self.found_instance_method(node, &name, offenses);
            }
            // `def_delegators :target, :one, :two` defines every name after the target.
            "def_delegators" | "def_instance_delegators"
                if arguments.len() >= 2 && !has_if_ancestor(node) =>
            {
                for name in arguments.iter().skip(1) {
                    self.found_instance_method(node, name, offenses);
                }
            }
            _ => {}
        }
    }

    fn on_attr(
        &mut self,
        node: Node<'_>,
        accessor: &str,
        arguments: &[String],
        offenses: &mut Vec<Offense>,
    ) {
        let (readable, writable) = match accessor {
            // `attr :foo, true` is the historical writer form.
            "attr" => (true, false),
            "attr_reader" => (true, false),
            "attr_writer" => (false, true),
            _ => (true, true),
        };
        let names: Vec<String> = if accessor == "attr" {
            arguments[..1].to_vec()
        } else {
            arguments.to_vec()
        };
        for name in names {
            if readable {
                self.found_instance_method(node, &name, offenses);
            }
            if writable {
                self.found_instance_method(node, &format!("{name}="), offenses);
            }
        }
    }

    fn found_instance_method(&mut self, node: Node<'_>, name: &str, offenses: &mut Vec<Offense>) {
        if let Some(scope) = parent_module_name(self.context, node) {
            let method = format!("{}{name}", humanize_scope(&scope));
            self.found_method(node, &method, None, offenses);
        } else if let Some(anon_block) = anonymous_class_block(self.context, node) {
            let base =
                qualified_object_scope(parent_module_name(self.context, anon_block).as_deref());
            let scope = if singleton_class_ancestor(node).is_some() {
                format!("#<Class:{base}>")
            } else {
                base
            };
            let method = format!("{}{name}", humanize_scope(&scope));
            let scope_id = anon_block_scope_id(self.context, anon_block);
            self.found_method(node, &method, scope_id, offenses);
        } else {
            self.found_sclass_method(node, name, offenses);
        }
    }

    fn check_self_receiver(&mut self, node: Node<'_>, name: &str, offenses: &mut Vec<Offense>) {
        if let Some(enclosing) = parent_module_name(self.context, node) {
            self.found_method(node, &format!("{enclosing}.{name}"), None, offenses);
        } else if let Some(anon_block) = anonymous_class_block(self.context, node) {
            let scope =
                qualified_object_scope(parent_module_name(self.context, anon_block).as_deref());
            let scope_id = anon_block_scope_id(self.context, anon_block);
            self.found_method(node, &format!("{scope}.{name}"), scope_id, offenses);
        }
    }

    /// `class << foo` at a spot with no lexical namespace still names a receiver, which is the
    /// only handle the cop has on where the method lands.
    fn found_sclass_method(&mut self, node: Node<'_>, name: &str, offenses: &mut Vec<Offense>) {
        let Some(sclass) = singleton_class_ancestor(node) else {
            return;
        };
        let Some(receiver) = sclass.child_by_field_name("value") else {
            return;
        };
        if receiver.kind() != "call" {
            return;
        }
        let Some(method) = receiver.child_by_field_name("method") else {
            return;
        };
        let method = self.text(method).to_owned();
        self.found_method(node, &format!("{method}.{name}"), None, offenses);
    }

    fn found_method(
        &mut self,
        node: Node<'_>,
        method_name: &str,
        scope_id: Option<String>,
        offenses: &mut Vec<Offense>,
    ) {
        // A definition nested inside another method only takes effect when that method runs, so it
        // is tracked under the enclosing method's name.
        let mut key = match enclosing_def(node) {
            Some(enclosing) => match self.name_text(enclosing) {
                Some(name) => format!("{name}.{method_name}"),
                None => method_name.to_owned(),
            },
            None => method_name.to_owned(),
        };
        if let Some(scope_id) = scope_id {
            key.push('@');
            key.push_str(&scope_id);
        }
        let line = self.context.source.line_column(node.start_byte()).0;
        let Some(&first) = self.definitions.get(&key) else {
            self.definitions.insert(key, line);
            return;
        };
        if let Some(scope) = rescue_scope(node)
            && self.scopes.entry(scope).or_default().insert(key.clone())
        {
            self.definitions.insert(key, line);
            return;
        }
        let path = self.context.display_path();
        offenses.push(self.context.offense(
            format!("Method `{method_name}` is defined at both {path}:{first} and {path}:{line}."),
            offense_range(node),
        ));
    }

    fn track_self_alias(&mut self, node: Node<'_>, name: &str) {
        if let Some(scope) = parent_module_name(self.context, node) {
            self.self_aliased
                .insert(format!("{}{name}", humanize_scope(&scope)));
        }
    }

    fn name_text(&self, node: Node<'_>) -> Option<String> {
        Some(self.text(node.child_by_field_name("name")?).to_owned())
    }

    fn text(&self, node: Node<'_>) -> &'a str {
        self.context.source.node_text(node)
    }

    fn lookup_constant(&self, node: Node<'_>, const_name: &str) -> Option<String> {
        // Deliberately imperfect, exactly as upstream: resolving a constant properly would need an
        // index of the whole project, so only the enclosing definitions are consulted.
        let mut current = node;
        while let Some(parent) = current.parent() {
            if let Some(defined) = defined_module_name(self.context, parent) {
                let bare = defined.rsplit("::").next().unwrap_or(&defined);
                if bare == const_name.trim_start_matches("::") {
                    let enclosing = parent_module_name(self.context, parent);
                    return Some(match enclosing.as_deref() {
                        Some("Object") | None => defined,
                        Some(name) => format!("{name}::{defined}"),
                    });
                }
            }
            current = parent;
        }
        None
    }
}

/// Where the offense is drawn. A definition is pointed at from `def` through its name; everything
/// else is reported over its whole expression.
fn offense_range(node: Node<'_>) -> std::ops::Range<usize> {
    if matches!(node.kind(), "method" | "singleton_method")
        && let Some(name) = node.child_by_field_name("name")
    {
        return node.start_byte()..name.end_byte();
    }
    node.byte_range()
}

fn has_if_ancestor(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if matches!(
            parent.kind(),
            "if" | "unless" | "if_modifier" | "unless_modifier" | "conditional" | "elsif"
        ) {
            return true;
        }
        node = parent;
    }
    false
}

fn enclosing_def(mut node: Node<'_>) -> Option<Node<'_>> {
    while let Some(parent) = node.parent() {
        if matches!(parent.kind(), "method" | "singleton_method") {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn singleton_class_ancestor(mut node: Node<'_>) -> Option<Node<'_>> {
    while let Some(parent) = node.parent() {
        if parent.kind() == "singleton_class" {
            return Some(parent);
        }
        node = parent;
    }
    None
}

/// RuboCop wraps a body holding `rescue`/`ensure` clauses in nodes of those names, so everything in
/// the body counts as being inside them. tree-sitter keeps the clauses as siblings of the
/// statements instead, so the wrapper has to be reconstructed by looking at the body's children.
fn rescue_scope(node: Node<'_>) -> Option<&'static str> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if matches!(parent.kind(), "body_statement" | "begin" | "block_body") {
            let mut cursor = parent.walk();
            let mut has_rescue = false;
            let mut has_ensure = false;
            for child in parent.named_children(&mut cursor) {
                has_rescue |= child.kind() == "rescue";
                has_ensure |= child.kind() == "ensure";
            }
            if current.kind() == "ensure" {
                return Some("ensure");
            }
            if has_rescue {
                return Some("rescue");
            }
            if has_ensure {
                return Some("ensure");
            }
        }
        current = parent;
    }
    None
}

/// The lexical namespace enclosing `node`, or `None` when a block breaks the chain -- a block body
/// is ordinary code, so what a `def` inside one attaches to cannot be read off the source.
fn parent_module_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "class" | "module" | "assignment" => {
                if let Some(name) = defined_module_name(context, parent) {
                    parts.push(name);
                }
            }
            "singleton_class" => parts.push(singleton_class_name(context, parent)?),
            "block" | "do_block" => match block_module_name(context, parent) {
                BlockScope::Transparent => {}
                BlockScope::Named(name) => parts.push(name),
                BlockScope::Opaque => return None,
            },
            _ => {}
        }
        current = parent;
    }
    parts.reverse();
    Some(if parts.is_empty() {
        "Object".to_owned()
    } else {
        parts.join("::")
    })
}

/// The constant a `class`, `module` or `CONST = Class.new` declaration defines. A constant
/// assignment of anything else names no module, and simply contributes nothing.
fn defined_module_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    match node.kind() {
        "class" | "module" => Some(constant_name(context, node.child_by_field_name("name")?)),
        "assignment" => {
            let left = node.child_by_field_name("left")?;
            if !matches!(left.kind(), "constant" | "scope_resolution") {
                return None;
            }
            let right = node.child_by_field_name("right")?;
            is_class_or_module_new(context, right, true).then(|| constant_name(context, left))
        }
        _ => None,
    }
}

fn constant_name(context: &RuleContext<'_>, node: Node<'_>) -> String {
    context
        .source
        .node_text(node)
        .trim_start_matches("::")
        .to_owned()
}

/// `Class.new`/`Module.new`, optionally carrying a block. `global` demands the bare global
/// constant, which is what `CONST = Class.new` requires; the anonymous-class lookup accepts a
/// namespaced one as upstream does.
fn is_class_or_module_new(context: &RuleContext<'_>, node: Node<'_>, global: bool) -> bool {
    let call = if node.kind() == "call" {
        node
    } else {
        return false;
    };
    let (Some(receiver), Some(method)) = (
        call.child_by_field_name("receiver"),
        call.child_by_field_name("method"),
    ) else {
        return false;
    };
    if context.source.node_text(method) != "new" {
        return false;
    }
    let name = constant_name(context, receiver);
    if global && (name.contains("::") || call.child_by_field_name("arguments").is_some()) {
        return false;
    }
    let bare = name.rsplit("::").next().unwrap_or(&name);
    matches!(receiver.kind(), "constant" | "scope_resolution") && matches!(bare, "Class" | "Module")
}

fn singleton_class_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let value = node.child_by_field_name("value")?;
    match value.kind() {
        "constant" | "scope_resolution" => {
            Some(format!("#<Class:{}>", constant_name(context, value)))
        }
        "self" => Some(format!(
            "#<Class:{}>",
            parent_module_name(context, node).unwrap_or_default()
        )),
        _ => None,
    }
}

enum BlockScope {
    /// Contributes no namespace but does not break the chain: `CONST = Class.new do ... end`.
    Transparent,
    /// `Const.class_eval do ... end` reopens a named class.
    Named(String),
    /// An ordinary block, which makes the namespace unknowable.
    Opaque,
}

fn block_module_name(context: &RuleContext<'_>, block: Node<'_>) -> BlockScope {
    let Some(call) = block.parent().filter(|parent| parent.kind() == "call") else {
        return BlockScope::Opaque;
    };
    let Some(method) = call.child_by_field_name("method") else {
        return BlockScope::Opaque;
    };
    if context.source.node_text(method) == "class_eval" {
        return match call.child_by_field_name("receiver") {
            None => BlockScope::Transparent,
            Some(receiver) if matches!(receiver.kind(), "constant" | "scope_resolution") => {
                BlockScope::Named(constant_name(context, receiver))
            }
            Some(_) => BlockScope::Opaque,
        };
    }
    let assigned_to_constant = call
        .parent()
        .filter(|parent| parent.kind() == "assignment")
        .and_then(|parent| parent.child_by_field_name("left"))
        .is_some_and(|left| matches!(left.kind(), "constant" | "scope_resolution"));
    if assigned_to_constant && is_class_or_module_new(context, call, true) {
        BlockScope::Transparent
    } else {
        BlockScope::Opaque
    }
}

/// `qualified_name(enclosing, nil, 'Object')` -- the name upstream builds for a definition inside
/// an anonymous class. A missing enclosing name interpolates to nothing, which is where the
/// leading `::` in `::Object` comes from.
fn qualified_object_scope(enclosing: Option<&str>) -> String {
    match enclosing {
        Some("Object") => "Object".to_owned(),
        Some(name) => format!("{name}::Object"),
        None => "::Object".to_owned(),
    }
}

/// `Foo` becomes `Foo#`, and a singleton class marker becomes the dotted form: `Foo::#<Class:Foo>`
/// and `#<Class:Foo>` both become `Foo.`.
fn humanize_scope(scope: &str) -> String {
    let humanized = if let Some((prefix, marker)) = scope.rsplit_once("::#<Class:") {
        match marker.strip_suffix('>').filter(|name| *name == prefix) {
            Some(name) => format!("{name}."),
            None => scope.to_owned(),
        }
    } else if let Some(rest) = scope.strip_prefix("#<Class:") {
        match rest.rfind('>') {
            Some(index) => {
                let tail = &rest[index + 1..];
                format!(
                    "{}.{}",
                    &rest[..index],
                    tail.strip_prefix("::").unwrap_or(tail)
                )
            }
            None => scope.to_owned(),
        }
    } else {
        scope.to_owned()
    };
    if humanized.ends_with('.') {
        humanized
    } else {
        format!("{humanized}#")
    }
}

/// The `Class.new`/`Module.new` block a definition sits directly inside, if any. Methods defined
/// there land on one anonymous class, so they can collide with each other even though no constant
/// names the class.
fn anonymous_class_block<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<Node<'tree>> {
    let mut current = node;
    let block = loop {
        let parent = current.parent()?;
        if matches!(parent.kind(), "block" | "do_block") {
            break parent.parent().filter(|call| call.kind() == "call")?;
        }
        current = parent;
    };
    if !is_class_or_module_new(context, block, false) {
        return None;
    }
    // A class kept in a local variable is a value, not a namespace, so its methods are not pooled.
    if block
        .parent()
        .filter(|parent| parent.kind() == "assignment")
        .and_then(|parent| parent.child_by_field_name("left"))
        .is_some_and(|left| left.kind() == "identifier")
    {
        return None;
    }
    // `class << other` inside the block moves the definition somewhere else entirely.
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent == block {
            break;
        }
        if parent.kind() == "singleton_class"
            && parent
                .child_by_field_name("value")
                .is_none_or(|value| value.kind() != "self")
        {
            return None;
        }
        current = parent;
    }
    Some(block)
}

/// What upstream sees above an anonymous-class block, once tree-sitter's statement lists have been
/// folded away. The distinction matters because the scope id is derived from it, and a `None` id
/// pools every such block in the file into one scope.
enum AnonParent<'tree> {
    /// Upstream's implicit `begin`. It only names a scope when it is a block's body.
    Begin { under_block: bool },
    /// A block, whose scope id comes from the receiver of the call it belongs to.
    Block { call: Option<Node<'tree>> },
    /// A plain call the block was passed to, or whose receiver it is.
    Send { call: Node<'tree> },
    /// A constant assignment or a method definition: accepted, but naming no receiver.
    Plain,
    /// Something upstream does not accept, which leaves the block without a scope id.
    Rejected,
}

/// What distinguishes one anonymous class from another. `None` pools every such block in the file
/// together, which is upstream's behaviour for a block whose surroundings say nothing about where
/// the class ends up.
fn anon_block_scope_id(context: &RuleContext<'_>, block: Node<'_>) -> Option<String> {
    match anon_parent(block) {
        AnonParent::Rejected => None,
        AnonParent::Begin { under_block } => {
            under_block.then(|| anon_block_identity(context, block))
        }
        AnonParent::Plain => Some(anon_block_identity(context, block)),
        // A block parent hands over its call's receiver whatever the block constructs.
        AnonParent::Block { call } => {
            Some(match call.and_then(|call| named_receiver(context, call)) {
                Some((receiver, method)) => format!("{receiver}.{method}"),
                None => anon_block_identity(context, block),
            })
        }
        // A `Class.new` passed to a named method is excluded: the receiver is the same at every
        // call site, which would merge classes that have nothing to do with each other.
        AnonParent::Send { call } => {
            let receiver = if is_class_new_block(context, block) {
                None
            } else {
                named_receiver(context, call)
            };
            Some(match receiver {
                Some((receiver, method)) => format!("{receiver}.{method}"),
                None => anon_block_identity(context, block),
            })
        }
    }
}

/// Folds tree-sitter's statement lists away to reach the node upstream would call the block's
/// parent. A list of several statements is upstream's `begin`; a single statement attaches
/// straight to the construct owning the body; and a body with a `rescue`/`ensure` clause is
/// wrapped in a node of that name, which upstream does not accept as a scope.
fn anon_parent(block: Node<'_>) -> AnonParent<'_> {
    let Some(parent) = block.parent() else {
        return AnonParent::Rejected;
    };
    match parent.kind() {
        "body_statement" | "block_body" => {
            let (statements, clause) = statement_shape(parent);
            if statements > 1 {
                return AnonParent::Begin {
                    under_block: !clause
                        && parent
                            .parent()
                            .is_some_and(|owner| matches!(owner.kind(), "block" | "do_block")),
                };
            }
            if clause {
                return AnonParent::Rejected;
            }
            match parent.parent() {
                Some(owner) => owner_parent(owner),
                None => AnonParent::Rejected,
            }
        }
        // `begin ... end` is upstream's `kwbegin`, which is not among the accepted kinds.
        "begin" => AnonParent::Rejected,
        _ => owner_parent(parent),
    }
}

fn owner_parent(owner: Node<'_>) -> AnonParent<'_> {
    match owner.kind() {
        "block" | "do_block" => AnonParent::Block {
            call: owner.parent().filter(|call| call.kind() == "call"),
        },
        "lambda" => AnonParent::Block { call: None },
        "method" | "singleton_method" => AnonParent::Plain,
        "assignment" => match owner.child_by_field_name("left") {
            Some(left) if matches!(left.kind(), "constant" | "scope_resolution") => {
                AnonParent::Plain
            }
            _ => AnonParent::Rejected,
        },
        "argument_list" => match owner.parent() {
            Some(call) if call.kind() == "call" => AnonParent::Send { call },
            _ => AnonParent::Rejected,
        },
        // `Class.new do ... end.new` -- the block is the receiver of a further call, which is a
        // plain send to upstream.
        "call" => AnonParent::Send { call: owner },
        _ => AnonParent::Rejected,
    }
}

/// How many statements a body holds and whether it carries a `rescue`/`ensure`/`else` clause.
fn statement_shape(body: Node<'_>) -> (usize, bool) {
    let mut cursor = body.walk();
    let mut statements = 0;
    let mut clause = false;
    for child in body.named_children(&mut cursor) {
        if matches!(child.kind(), "rescue" | "ensure" | "else") {
            clause = true;
        } else {
            statements += 1;
        }
    }
    (statements, clause)
}

/// The receiver and method of a call, unless the receiver is itself a `Class.new`/`Module.new`
/// block -- naming one anonymous class after another would say nothing about where it lands.
fn named_receiver<'a>(context: &'a RuleContext<'_>, call: Node<'_>) -> Option<(&'a str, &'a str)> {
    let receiver = call.child_by_field_name("receiver")?;
    let method = call.child_by_field_name("method")?;
    if is_class_or_module_new(context, receiver, false) {
        return None;
    }
    Some((
        context.source.node_text(receiver),
        context.source.node_text(method),
    ))
}

fn is_class_new_block(context: &RuleContext<'_>, block: Node<'_>) -> bool {
    block
        .child_by_field_name("receiver")
        .is_some_and(|receiver| {
            constant_name(context, receiver)
                .rsplit("::")
                .next()
                .is_some_and(|name| name == "Class")
        })
        && is_class_or_module_new(context, block, false)
}

fn anon_block_identity(context: &RuleContext<'_>, block: Node<'_>) -> String {
    let (line, _) = context.source.line_column(block.start_byte());
    format!(
        "{}:{line}:{}",
        context.source.path().display(),
        block.start_byte()
    )
}

/// Whether a bare method call sits where a class-body macro can: at the top level, or in a
/// class-like body, possibly wrapped in `begin`/block/`if` bodies.
fn in_macro_scope(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    match parent.kind() {
        "program" => true,
        "class" | "module" | "singleton_class" => true,
        "body_statement" | "block_body" | "begin" | "then" | "else" => {
            parent.parent().is_none_or(|grandparent| {
                matches!(
                    grandparent.kind(),
                    "class" | "module" | "singleton_class" | "block" | "do_block" | "program"
                ) || in_macro_scope(parent)
            })
        }
        "block" | "do_block" => in_macro_scope(parent),
        _ => false,
    }
}

/// The symbol or string arguments of a macro call. A call with any other kind of argument defines
/// no name the cop can track.
fn literal_arguments(context: &RuleContext<'_>, node: Node<'_>) -> Vec<String> {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut cursor = arguments.walk();
    let mut names = Vec::new();
    for argument in arguments.named_children(&mut cursor) {
        match symbol_name(context.source.node_text(argument)) {
            Some(name) if matches!(argument.kind(), "simple_symbol" | "string") => {
                names.push(name.to_owned());
            }
            _ => return Vec::new(),
        }
    }
    names
}

/// The name a `:foo`, `"foo"` or bare `foo` argument spells.
fn symbol_name(text: &str) -> Option<&str> {
    let text = text.strip_prefix(':').unwrap_or(text);
    let unquoted = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            text.strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(text);
    (!unquoted.is_empty() && !unquoted.contains(['#', '"', '\''])).then_some(unquoted)
}
