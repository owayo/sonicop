//! Whether a bare identifier reads a local variable or calls a method without a receiver.
//!
//! tree-sitter writes both as an `identifier`, while upstream's parser has already decided: it
//! builds an `lvar` for a name it has seen assigned in the enclosing scope and a receiverless
//! `send` for everything else. A cop ported from a node pattern that names one of the two has to
//! draw the same line, or it answers a different question than the pattern it came from.
//!
//! The analysis behind the answer walks the whole file, so it is deferred until a cop actually has
//! a candidate to ask about -- most files hold none, and a cop that ran it eagerly would pay for
//! the walk in every file it inspects.

use std::cell::OnceCell;
use std::ops::Range;

use tree_sitter::Node;

use crate::rules::RuleContext;
use crate::source::SourceFile;

use super::variable_force::Analysis;

pub(in crate::rules) struct LocalVariables<'a> {
    root: Node<'a>,
    source: &'a SourceFile,
    analysis: OnceCell<Analysis<'a>>,
}

impl<'a> LocalVariables<'a> {
    pub(in crate::rules) fn new(context: &'a RuleContext<'_>) -> Self {
        Self {
            root: context.root_node(),
            source: context.source,
            analysis: OnceCell::new(),
        }
    }

    /// Whether upstream's parser would have built an `lvar` here rather than a receiverless call.
    pub(in crate::rules) fn is_lvar(&self, node: Node<'_>) -> bool {
        self.analysis
            .get_or_init(|| Analysis::run(self.root, self.source))
            .is_variable_reference(node)
    }

    /// Where each variable in the file is written and where it is read.
    ///
    /// A cop that joins `VariableForce` through `after_leaving_scope` sees the whole table rather
    /// than one answer about one node: `Style/InfiniteLoop` asks whether a name a loop body
    /// introduces is still read below the loop, which no single node can say.
    pub(in crate::rules) fn variable_spans(&self) -> Vec<VariableSpans> {
        self.analysis
            .get_or_init(|| Analysis::run(self.root, self.source))
            .variables
            .iter()
            .map(|variable| VariableSpans {
                assignments: variable
                    .assignments
                    .iter()
                    .map(|assignment| assignment.node.byte_range())
                    .collect(),
                references: variable
                    .references
                    .iter()
                    .map(|reference| reference.node.byte_range())
                    .collect(),
            })
            .collect()
    }
}

/// One variable's writes and reads, as the spans `Variable#assignments` and `Variable#references`
/// point at.
pub(in crate::rules) struct VariableSpans {
    pub assignments: Vec<Range<usize>>,
    pub references: Vec<Range<usize>>,
}
