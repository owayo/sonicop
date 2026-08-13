//! Whether a `private` governs a group of methods or is written against each definition.
//!
//! Only definitions are at stake: `private :foo`, `private attr_reader :bar` and
//! `private alias_method :a, :b` name what they apply to rather than opening a scope, and the
//! configuration lets each of the three stand.

use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::lint::access_modifier::{in_macro_scope, send_name, statements};
use crate::rules::lint::locals::LocalVariables;
use crate::rules::send_node;

const GROUP_STYLE_MESSAGE: &str = "should not be inlined in method definitions.";
const INLINE_STYLE_MESSAGE: &str = "should be inlined in method definitions.";

/// `RESTRICT_ON_SEND`, which is also the set `bare_access_modifier_declaration?` matches.
const MODIFIERS: [&str; 4] = ["private", "protected", "public", "module_function"];

/// `{:attr :attr_reader :attr_writer :attr_accessor}`.
const ATTR_MACROS: [&str; 4] = ["attr", "attr_reader", "attr_writer", "attr_accessor"];

/// The statement lists tree-sitter wraps a body in. Upstream reads a list of two or more as a
/// `begin` and a list of one as the statement itself, which is what decides who a node's siblings
/// are. A block is not one of them: it is a node of its own upstream, and a body of one statement
/// hangs off it directly.
const STATEMENT_LISTS: [&str; 6] = [
    "program",
    "body_statement",
    "block_body",
    "then",
    "else",
    "parenthesized_statements",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let group = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "group");
    let cop = Cop {
        context,
        locals: LocalVariables::new(context),
        group,
        allow_symbols: context.setting("AllowModifiersOnSymbols").unwrap_or(true),
        allow_attrs: context.setting("AllowModifiersOnAttrs").unwrap_or(true),
        allow_alias_method: context
            .setting("AllowModifiersOnAliasMethod")
            .unwrap_or(true),
    };
    for node in context.nodes_of_any(&["call", "identifier"]) {
        cop.on_send(node, offenses);
    }
}

struct Cop<'a, 'tree> {
    context: &'a RuleContext<'tree>,
    locals: LocalVariables<'a>,
    group: bool,
    allow_symbols: bool,
    allow_attrs: bool,
    allow_alias_method: bool,
}

impl<'tree> Cop<'_, 'tree> {
    fn on_send(&self, node: Node<'tree>, offenses: &mut Vec<Offense>) {
        let Some(name) = self.modifier_name(node) else {
            return;
        };
        if self.allowed(node) || !self.offense(node, name) {
            return;
        }
        let selector = self.selector(node);
        let message = format!(
            "`{}` {}",
            name,
            match self.group {
                true => GROUP_STYLE_MESSAGE,
                false => INLINE_STYLE_MESSAGE,
            }
        );
        let mut offense = self.context.offense(message, selector);
        if let Some((edits, anchor)) = self.correct(node, name) {
            if let Some(anchor) = anchor {
                offense = offense.corrections_anchored_at(anchor);
            }
            offense = offense.corrected_by_all(edits);
        }
        offenses.push(offense);
    }

    /// `RESTRICT_ON_SEND` together with `macro?`: the name of the modifier a receiverless call in a
    /// class-like scope spells.
    fn modifier_name(&self, node: Node<'tree>) -> Option<&'static str> {
        if node.kind() == "call" && node.child_by_field_name("receiver").is_some() {
            return None;
        }
        let name = send_name(node, self.context)?;
        let name = MODIFIERS.into_iter().find(|modifier| *modifier == name)?;
        in_macro_scope(node, self.context).then_some(name)
    }

    fn allowed(&self, node: Node<'tree>) -> bool {
        // `(pair ...)`: a modifier's name used as a hash key is no modifier, and one standing alone
        // as a block's body -- `Class.new { private def foo; end }` -- is left as it was written.
        if self
            .upstream_parent(node)
            .is_some_and(|parent| matches!(parent.kind(), "pair" | "do_block" | "block"))
        {
            return true;
        }
        (self.allow_symbols && self.modifier_with_symbol(node))
            || (self.allow_attrs && self.modifier_with_attr(node))
            || (self.allow_alias_method && self.modifier_with_alias_method(node))
    }

    /// `access_modifier_with_symbol?`: every argument is a plain symbol, or the one argument is a
    /// splat of a symbol array, a constant or a call.
    fn modifier_with_symbol(&self, node: Node<'tree>) -> bool {
        let arguments = self.arguments(node);
        if arguments.is_empty() {
            return false;
        }
        if arguments
            .iter()
            .all(|argument| send_node::symbol_name(*argument, self.context).is_some())
        {
            return true;
        }
        let [splat] = arguments.as_slice() else {
            return false;
        };
        if splat.kind() != "splat_argument" {
            return false;
        }
        let Some(value) = send_node::named_children(*splat).first().copied() else {
            return false;
        };
        match value.kind() {
            "constant" | "scope_resolution" => true,
            "array" => percent_symbol_array(value, self.context),
            // `send` and nothing else: `private(*names)` splats an `lvar`, which the pattern does
            // not match, and the modifier is an offense after all.
            _ => send_name(value, self.context).is_some() && !self.locals.is_lvar(value),
        }
    }

    /// `access_modifier_with_attr?`: one argument, a receiverless `attr*` call with arguments of
    /// its own.
    fn modifier_with_attr(&self, node: Node<'tree>) -> bool {
        self.wraps_call(node, &ATTR_MACROS, |count| count >= 1)
    }

    /// `access_modifier_with_alias_method?`: one argument, a receiverless `alias_method` call of
    /// exactly two arguments.
    fn modifier_with_alias_method(&self, node: Node<'tree>) -> bool {
        self.wraps_call(node, &["alias_method"], |count| count == 2)
    }

    fn wraps_call(&self, node: Node<'tree>, names: &[&str], arity: impl Fn(usize) -> bool) -> bool {
        let arguments = self.arguments(node);
        let [only] = arguments.as_slice() else {
            return false;
        };
        let only = *only;
        if only.kind() != "call" || only.child_by_field_name("receiver").is_some() {
            return false;
        }
        send_name(only, self.context).is_some_and(|name| names.contains(&name))
            && arity(self.arguments(only).len())
    }

    fn offense(&self, node: Node<'tree>, name: &str) -> bool {
        let inlined = !self.arguments(node).is_empty();
        if self.group {
            // A modifier standing in a branch of a conditional is left alone; without a parent at
            // all it is the whole file, where `allowed?` has already turned down the symbol form.
            if self
                .upstream_parent(node)
                .is_some_and(|parent| is_conditional(parent))
            {
                return false;
            }
            return inlined && !self.right_siblings_same_inline_method(node, name);
        }
        !inlined && !self.grouped_definitions(node).is_empty()
    }

    /// `right_siblings_same_inline_method?`: a later statement spelling the same modifier against
    /// something of its own, which will be corrected in this one's place.
    fn right_siblings_same_inline_method(&self, node: Node<'tree>, name: &str) -> bool {
        self.right_siblings(node).into_iter().any(|sibling| {
            self.modifier_name(sibling) == Some(name)
                && !self.arguments(sibling).is_empty()
                && !self.allowed(sibling)
        })
    }

    /// `select_grouped_def_nodes`: the definitions a bare modifier governs, which run until the
    /// next bare modifier.
    fn grouped_definitions(&self, node: Node<'tree>) -> Vec<Node<'tree>> {
        self.right_siblings(node)
            .into_iter()
            .take_while(|sibling| {
                !(self.modifier_name(*sibling).is_some() && self.arguments(*sibling).is_empty())
            })
            .filter(|sibling| sibling.kind() == "method")
            .collect()
    }

    fn correct(&self, node: Node<'tree>, name: &str) -> Option<(Vec<Edit>, Option<Range<usize>>)> {
        match self.group {
            true => self.correct_group(node, name),
            false => Some(self.correct_inline(node, name)),
        }
    }

    /// `replace_defs`: the definition moves under a modifier of its own, written where a bare one
    /// already stands or at the end of the class body.
    fn correct_group(
        &self,
        node: Node<'tree>,
        name: &str,
    ) -> Option<(Vec<Edit>, Option<Range<usize>>)> {
        // `find_corresponding_def_nodes` answers with the first argument whatever that argument
        // turns out to be; only the symbol form, which `allowed?` has already turned down under
        // the default configuration, looks the definitions up by name.
        let definition = *self.arguments(node).first()?;
        let source = self.definition_source(node, definition);
        let removals = [definition, node].map(|removed| Edit {
            start: self.with_comments_and_lines(removed).start,
            end: self.with_comments_and_lines(removed).end,
            replacement: String::new(),
            safe: false,
        });
        if let Some(bare) = self.argument_less_modifier(node, name) {
            let mut edits = vec![Edit {
                start: bare.end_byte(),
                end: bare.end_byte(),
                replacement: format!("\n\n{source}"),
                safe: false,
            }];
            edits.extend(removals);
            return Some((edits, Some(bare.byte_range())));
        }
        if let Some(end) = self.enclosing_class_end(node) {
            let mut edits = vec![Edit {
                start: end.start_byte(),
                end: end.start_byte(),
                replacement: format!("{name}\n\n{source}\n"),
                safe: false,
            }];
            edits.extend(removals);
            return Some((edits, Some(end.byte_range())));
        }
        Some((
            vec![Edit {
                start: node.start_byte(),
                end: node.end_byte(),
                replacement: format!("{name}\n\n{source}"),
                safe: false,
            }],
            None,
        ))
    }

    /// `autocorrect_inline_style`: the bare modifier goes away and each definition it governed
    /// gains one of its own.
    fn correct_inline(&self, node: Node<'tree>, name: &str) -> (Vec<Edit>, Option<Range<usize>>) {
        let removal = match self.right_siblings(node).first().copied() {
            // `remove_modifier_node_within_begin`: the removal stops at the first comment written
            // above the definition, which would otherwise be dropped along with the modifier.
            Some(next) => Edit {
                start: node.start_byte(),
                end: self
                    .leading_comments(next)
                    .first()
                    .map_or(next.start_byte(), |comment| comment.start),
                replacement: String::new(),
                safe: false,
            },
            None => {
                let lines = self.with_comments_and_lines(node);
                Edit {
                    start: lines.start,
                    end: lines.end,
                    replacement: String::new(),
                    safe: false,
                }
            }
        };
        let mut edits = vec![removal];
        edits.extend(
            self.grouped_definitions(node)
                .into_iter()
                .map(|definition| Edit {
                    start: definition.start_byte(),
                    end: definition.start_byte(),
                    replacement: format!("{name} "),
                    safe: false,
                }),
        );
        (edits, None)
    }

    /// `def_source`: the comments written above the modifier, then the definition itself.
    fn definition_source(&self, node: Node<'tree>, definition: Node<'tree>) -> String {
        let mut parts: Vec<&str> = self
            .leading_comments(node)
            .into_iter()
            .map(|comment| self.context.source.slice(comment))
            .collect();
        parts.push(self.context.source.node_text(definition));
        parts.join("\n")
    }

    /// `find_argument_less_modifier_node`: the first statement of the same scope spelling this
    /// modifier on its own, wherever it stands relative to this one.
    fn argument_less_modifier(&self, node: Node<'tree>, name: &str) -> Option<Node<'tree>> {
        self.siblings(node).into_iter().find(|sibling| {
            self.modifier_name(*sibling) == Some(name) && self.arguments(*sibling).is_empty()
        })
    }

    /// `each_ancestor(:class, :module, :sclass).first`, read as the `end` that closes it.
    fn enclosing_class_end(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        let mut current = node;
        while let Some(parent) = current.parent() {
            if matches!(parent.kind(), "class" | "module" | "singleton_class") {
                let last = u32::try_from(parent.child_count()).ok()?.checked_sub(1)?;
                return parent.child(last).filter(|end| end.kind() == "end");
            }
            current = parent;
        }
        None
    }

    /// `node.loc.selector`: the modifier's own name, without whatever it was given.
    fn selector(&self, node: Node<'tree>) -> Range<usize> {
        match node.child_by_field_name("method") {
            Some(method) => method.byte_range(),
            None => node.byte_range(),
        }
    }

    fn arguments(&self, node: Node<'tree>) -> Vec<Node<'tree>> {
        send_node::arguments(node)
            .into_iter()
            .map(|argument| argument.first())
            .collect()
    }

    /// The statements the scope holds, which is what `node.parent.children` yields when upstream's
    /// parser wrapped them in a `begin`. A scope of one statement has that statement standing where
    /// the `begin` would, and nothing beside it.
    fn siblings(&self, node: Node<'tree>) -> Vec<Node<'tree>> {
        let Some(parent) = node.parent() else {
            return Vec::new();
        };
        if !STATEMENT_LISTS.contains(&parent.kind()) {
            return Vec::new();
        }
        match statements(parent) {
            Some(statements) if statements.len() >= 2 => statements,
            _ => Vec::new(),
        }
    }

    fn right_siblings(&self, node: Node<'tree>) -> Vec<Node<'tree>> {
        let siblings = self.siblings(node);
        match siblings
            .iter()
            .position(|sibling| sibling.id() == node.id())
        {
            Some(index) => siblings[index + 1..].to_vec(),
            None => Vec::new(),
        }
    }

    /// The node upstream's parser would have made the parent: a statement list of two or more is a
    /// `begin`, and one of a single statement is not there at all.
    fn upstream_parent(&self, node: Node<'tree>) -> Option<Node<'tree>> {
        let mut current = node;
        while let Some(parent) = current.parent() {
            if !STATEMENT_LISTS.contains(&parent.kind()) {
                return Some(parent);
            }
            if statements(parent).is_some_and(|statements| statements.len() >= 2) {
                return Some(parent);
            }
            current = parent;
        }
        None
    }

    /// The comments `ast_with_comments` hands the node: the run written on lines of their own
    /// directly above it.
    ///
    /// A comment belongs to the first node that begins after it, so a node written after something
    /// else on its own line has none -- in `private def foo`, whatever stands above the line was
    /// claimed by the `private`. A comment sharing a line with the code before it decorates that
    /// code instead and never travels forward.
    fn leading_comments(&self, node: Node<'tree>) -> Vec<Range<usize>> {
        let source = self.context.source;
        let (line, column) = source.line_column(node.start_byte());
        if !source.line(line)[..column - 1].trim().is_empty() {
            return Vec::new();
        }
        let mut comments = Vec::new();
        for above in (1..line).rev() {
            let text = source.line(above);
            if text.trim().is_empty() {
                continue;
            }
            let start = source.line_start(above) + (text.len() - text.trim_start().len());
            let Some(comment) = self
                .context
                .comment_ranges()
                .iter()
                .find(|comment| comment.start == start)
            else {
                break;
            };
            comments.push(comment.clone());
        }
        comments.reverse();
        comments
    }

    /// `range_with_comments_and_lines`: the whole lines the node and its comments sit on, with the
    /// newline that ends the last of them.
    fn with_comments_and_lines(&self, node: Node<'tree>) -> Range<usize> {
        let source = self.context.source;
        let start = self
            .leading_comments(node)
            .first()
            .map_or(node.start_byte(), |comment| comment.start);
        let (first, _) = source.line_column(start);
        let (last, _) = source.line_column(node.end_byte());
        source.line_start(first)..source.line_range(last).end
    }
}

/// `percent_symbol_array?`: `%i[...]` or `%I[...]`.
fn percent_symbol_array(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    node.child(0)
        .is_some_and(|open| matches!(context.source.node_text(open).get(..2), Some("%i" | "%I")))
}

/// `if_type?`, which covers every conditional upstream's parser folds into an `if` node.
fn is_conditional(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "if" | "unless" | "elsif" | "if_modifier" | "unless_modifier" | "conditional"
    )
}
