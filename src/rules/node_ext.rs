//! The two node accessors cops reach for most, without the per-call string work the C API does.
//!
//! `Node::kind` calls `strlen` and validates UTF-8 on every call, and `Node::child_by_field_name`
//! resolves the field name by walking the grammar's field table with `strncmp` before it can even
//! start looking at children. Cops ask both questions millions of times a run -- together they were
//! 15% of a run over RuboCop's own tree -- and both answers are fixed by the grammar, so they are
//! resolved once here and read out of a table afterwards.
//!
//! Neither method changes what a cop sees: [`NodeExt::kind_str`] returns the same `&'static str`
//! the C API points at, and [`NodeExt::field`] falls back to the C lookup for any name the table
//! does not carry.

use std::num::NonZeroU16;
use std::sync::LazyLock;

use tree_sitter::{Language, Node};

use crate::rules::RuleContext;

pub(in crate::rules) fn language() -> Language {
    tree_sitter_ruby::LANGUAGE.into()
}

/// The field ids the codebase asks for, resolved from the grammar once.
///
/// A `match` on the name rather than a hash lookup, because the name is a literal at every call
/// site: with the method inlined the whole lookup folds into a single load.
macro_rules! field_ids {
    ($($field:ident => $name:literal),+ $(,)?) => {
        #[allow(non_snake_case)]
        struct FieldIds { $($field: u16,)+ }

        static FIELD_IDS: LazyLock<FieldIds> = LazyLock::new(|| {
            let language = language();
            FieldIds {
                $($field: language
                    .field_id_for_name($name)
                    .map_or(0, NonZeroU16::get),)+
            }
        });

        #[inline]
        fn field_id(name: &str) -> Option<u16> {
            match name {
                $($name => Some(FIELD_IDS.$field),)+
                _ => None,
            }
        }
    };
}

field_ids! {
    alternative => "alternative",
    arguments => "arguments",
    begin => "begin",
    block => "block",
    body => "body",
    condition => "condition",
    consequence => "consequence",
    end => "end",
    exceptions => "exceptions",
    handler => "handler",
    key => "key",
    left => "left",
    method => "method",
    name => "name",
    object => "object",
    operand => "operand",
    operator => "operator",
    parameters => "parameters",
    pattern => "pattern",
    receiver => "receiver",
    right => "right",
    scope => "scope",
    superclass => "superclass",
    value => "value",
    variable => "variable",
}

/// Every node kind the grammar can produce, indexed by the id `Node::kind_id` answers with.
static KIND_NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let language = language();
    (0..language.node_kind_count())
        .map(|id| {
            language
                .node_kind_for_id(id as u16)
                .expect("every id below the count names a kind")
        })
        .collect()
});

pub(crate) trait NodeExt<'tree> {
    /// `Node::kind`, read out of the grammar's own table.
    fn kind_str(&self) -> &'static str;

    /// `Node::child_by_field_name`, with the name resolved to its id ahead of time.
    fn field(&self, name: &str) -> Option<Node<'tree>>;

    /// `Node::parent`, answered from the file's parent index rather than by walking down from the
    /// root. See `AstIndex::parent`.
    fn parent_of(&self, context: &'tree RuleContext<'_>) -> Option<Node<'tree>>;
}

impl<'tree> NodeExt<'tree> for Node<'tree> {
    #[inline]
    fn kind_str(&self) -> &'static str {
        match KIND_NAMES.get(self.kind_id() as usize) {
            Some(kind) => kind,
            // A kind the table does not carry cannot happen for a tree this grammar produced, but
            // answering from the C API rather than panicking keeps a grammar upgrade from taking
            // the run down.
            None => self.kind(),
        }
    }

    #[inline]
    fn field(&self, name: &str) -> Option<Node<'tree>> {
        match field_id(name) {
            Some(0) => None,
            Some(id) => self.child_by_field_id(id),
            None => self.child_by_field_name(name),
        }
    }

    #[inline]
    fn parent_of(&self, context: &'tree RuleContext<'_>) -> Option<Node<'tree>> {
        context.parent(*self)
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeExt, language};
    use tree_sitter::Parser;

    fn tree(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser.set_language(&language()).unwrap();
        parser.parse(source, None).unwrap()
    }

    /// The table and the C API have to agree for every node of a real file, or a cop matching on a
    /// kind would silently stop matching.
    #[test]
    fn the_table_answers_what_the_parser_answers() {
        let tree = tree(
            "class Foo\n  def bar(a = 1, &block)\n    @x ||= a.map { |v| v + 1 }\n\
             rescue StandardError => e\n    raise e\n  end\nend\n",
        );
        let mut stack = vec![tree.root_node()];
        let mut seen = 0;
        while let Some(node) = stack.pop() {
            assert_eq!(node.kind_str(), node.kind(), "kind of {node:?}");
            for name in [
                "method",
                "receiver",
                "left",
                "right",
                "body",
                "name",
                "operator",
                "arguments",
                "parameters",
                "value",
                "condition",
                "consequence",
                "alternative",
            ] {
                assert_eq!(
                    node.field(name).map(|child| child.byte_range()),
                    node.child_by_field_name(name)
                        .map(|child| child.byte_range()),
                    "field {name} of {node:?}"
                );
            }
            seen += 1;
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
        assert!(seen > 30, "the sample has to reach a variety of nodes");
    }

    /// A name the grammar has no field for must answer the way the C API does rather than pick a
    /// neighbouring field.
    #[test]
    fn an_unknown_field_answers_none() {
        let tree = tree("a.b(1)\n");
        assert!(tree.root_node().field("nonexistent").is_none());
    }
}
