//! Prints the tree the grammar builds for a snippet, one node per line.
//!
//! Reading a difference against upstream means knowing which node the grammar made where upstream
//! made another -- `variable + if` is a `send` there and a `binary` here, `_1` turns a `block` into
//! a `numblock`, an argument list appears between a call and its arguments. Guessing at that costs
//! more than printing it.
//!
//! ```text
//! cargo run --release --example dump_ast -- 'do_something(**{foo: bar, **{baz: qux}})'
//! cargo run --release --example dump_ast --file path/to/source.rb
//! ```

use std::io::Read;

use tree_sitter::{Node, Parser};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let source = match arguments.next().as_deref() {
        Some("--file") => {
            let path = arguments.next().expect("--file needs a path");
            std::fs::read_to_string(&path).expect("cannot read the file")
        }
        Some(text) => text.to_owned(),
        None => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .expect("cannot read stdin");
            text
        }
    };

    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .expect("the grammar must load");
    let tree = parser
        .parse(&source, None)
        .expect("the parse must produce a tree");
    print(tree.root_node(), &source, 0);
}

fn print(node: Node<'_>, source: &str, depth: usize) {
    let field = node
        .parent()
        .and_then(|parent| {
            let mut cursor = parent.walk();
            cursor.goto_first_child();
            loop {
                if cursor.node().id() == node.id() {
                    return cursor.field_name();
                }
                if !cursor.goto_next_sibling() {
                    return None;
                }
            }
        })
        .map(|name| format!(" [{name}]"))
        .unwrap_or_default();
    let text = &source[node.byte_range()];
    let shown: String = text.chars().take(40).collect();
    let named = if node.is_named() { "" } else { " (anon)" };
    println!(
        "{:indent$}{}{field}{named}  {:?}",
        "",
        node.kind(),
        shown,
        indent = depth * 2
    );
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        print(child, source, depth + 1);
    }
}
