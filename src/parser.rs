//! One Ruby parser per worker thread.
//!
//! `Parser::new` allocates the parse stack, the lexer buffers and the reusable subtree pool, and
//! `set_language` resets all of them. That is work the file being read has nothing to do with, and
//! several places used to repeat it for every file, every recovered fragment and every syntax
//! probe. A sampling profile put it among the costs of a run.
//!
//! The tree a parse returns owns its own storage, so nothing here is shared with the caller.

use std::cell::RefCell;

use tree_sitter::{Parser, Tree};

thread_local! {
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Parses `text` with this thread's parser, building one on first use.
///
/// `None` means the parser produced no tree at all, which is not the same as a tree holding
/// errors: a source the grammar cannot read still comes back as a tree with `ERROR` nodes.
pub(crate) fn parse(text: &str) -> Option<Tree> {
    PARSER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let parser = match slot.as_mut() {
            Some(parser) => parser,
            None => {
                let mut parser = Parser::new();
                parser
                    .set_language(&tree_sitter_ruby::LANGUAGE.into())
                    .expect("the grammar the whole run is parsed with");
                slot.insert(parser)
            }
        };
        parser.parse(text, None)
    })
}
