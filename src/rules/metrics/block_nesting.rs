use std::ops::Range;

use tree_sitter::Node;

use super::locals::named_children;
use crate::diagnostic::Offense;
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let max: usize = context.setting("Max").unwrap_or(3);
    let mut nesting = Nesting {
        max,
        count_blocks: context.setting("CountBlocks").unwrap_or(false),
        count_modifier_forms: context.setting("CountModifierForms").unwrap_or(false),
        ignored: Vec::new(),
        offenses: Vec::new(),
    };
    nesting.check(context.root_node(), 0);
    for range in nesting.offenses {
        offenses.push(context.offense(
            format!("Avoid more than {max} levels of block nesting."),
            range,
        ));
    }
}

struct Nesting {
    max: usize,
    count_blocks: bool,
    count_modifier_forms: bool,
    /// The ranges already reported. Everything inside one of them is part of the same run of
    /// nesting and is left alone, which is what `ignore_node` buys upstream.
    ignored: Vec<Range<usize>>,
    offenses: Vec<Range<usize>>,
}

impl Nesting {
    fn check(&mut self, node: Node<'_>, mut level: usize) {
        // `x rescue y` is a `rescue` holding a `resbody`, and only the handler is inside that
        // clause: the expression on the left of the keyword is guarded by it, not nested in it.
        if node.kind_str() == "rescue_modifier" {
            if let Some(body) = node.field("body") {
                self.check(body, level);
            }
            if let Some(handler) = node.field("handler") {
                let inner = level + 1;
                self.report(modifier_clause(node, handler), inner);
                self.check(handler, inner);
            }
            return;
        }
        if self.considered(node) {
            if self.counts(node) {
                level += 1;
            }
            self.report(clause_range(node), level);
        }
        for child in named_children(node) {
            self.check(child, level);
        }
    }

    fn report(&mut self, range: Range<usize>, level: usize) {
        if level <= self.max || self.part_of_ignored(&range) {
            return;
        }
        self.ignored.push(range.clone());
        self.offenses.push(range);
    }

    /// `BlockNesting::NESTING_BLOCKS`, plus every kind of block when `CountBlocks` asks for them.
    fn considered(&self, node: Node<'_>) -> bool {
        match node.kind_str() {
            "case" | "case_match" | "if" | "elsif" | "unless" | "if_modifier"
            | "unless_modifier" | "conditional" | "while" | "while_modifier" | "until"
            | "until_modifier" | "for" | "rescue" => true,
            // `->() {}` is one block upstream, so its braces must not count a second time.
            "block" => self.count_blocks && !is_lambda_body(node),
            "do_block" | "lambda" => self.count_blocks,
            _ => false,
        }
    }

    /// `count_if_block?`: an `elsif` continues the `if` above it rather than nesting inside it,
    /// and a modifier form only counts when asked for.
    fn counts(&self, node: Node<'_>) -> bool {
        match node.kind_str() {
            "elsif" => false,
            "if_modifier" | "unless_modifier" => self.count_modifier_forms,
            _ => true,
        }
    }

    fn part_of_ignored(&self, range: &Range<usize>) -> bool {
        self.ignored
            .iter()
            .any(|ignored| ignored.start <= range.start && ignored.end >= range.end)
    }
}

/// The range the offense covers, which is the node's own range upstream.
///
/// Two node kinds start or end elsewhere here: a block belongs to the call that takes it, which is
/// where its range begins, and a `rescue` clause stops at the last statement it guards rather than
/// running on over the comments and separators that follow.
fn clause_range(node: Node<'_>) -> Range<usize> {
    if matches!(node.kind_str(), "block" | "do_block") {
        return super::support::block_location(node).byte_range();
    }
    if node.kind_str() != "rescue" {
        return node.byte_range();
    }
    // A clause that guards nothing -- one whose body holds only a comment -- ends where its
    // exception list does, or at the keyword when it lists none.
    let listed = ["variable", "exceptions"]
        .iter()
        .find_map(|field| node.field(field))
        .map_or(node.start_byte() + "rescue".len(), |part| part.end_byte());
    let guarded = node
        .field("body")
        .and_then(last_statement)
        .map_or(listed, |statement| statement.end_byte());
    node.start_byte()..guarded.max(listed)
}

/// The `resbody` of `x rescue y`, which starts at the keyword and ends with the handler.
fn modifier_clause(node: Node<'_>, handler: Node<'_>) -> Range<usize> {
    let mut cursor = node.walk();
    let keyword = node.children(&mut cursor)
        .find(|child| !child.is_named() && child.kind_str() == "rescue")
        .map_or(node.start_byte(), |child| child.start_byte());
    keyword..handler.end_byte()
}

fn last_statement<'tree>(body: Node<'tree>) -> Option<Node<'tree>> {
    named_children(body)
        .into_iter()
        .rfind(|child| !matches!(child.kind_str(), "comment" | "empty_statement" | "heredoc_body"))
}

fn is_lambda_body(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind_str() == "lambda")
}
