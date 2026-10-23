//! `Layout/LeadingEmptyLines`.

use std::ops::Range;

use crate::diagnostic::{Edit, Offense};
use crate::rules::node_ext::NodeExt;
use crate::rules::{ParserEngine, RuleContext};

const MSG: &str = "Unnecessary blank line at the beginning of the source.";

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // 本家はtokens[0]を使う。空programの範囲や行継続、DATAをtokenとして扱わない。
    let Some(token) = first_token_range(context) else {
        return;
    };
    if context.source.line_column(token.start).0 <= 1 {
        return;
    }
    offenses.push(context.offense(MSG, token.clone()).corrected_by(Edit {
        start: 0,
        end: token.start,
        replacement: String::new(),
        safe: true,
    }));
}

/// 先頭の1tokenだけを索引上で反復降下する。残りの木を再帰走査しない。
fn first_token_range(context: &RuleContext<'_>) -> Option<Range<usize>> {
    let mut node = context.root_node();
    loop {
        match node.kind_str() {
            "uninterpreted" | "heredoc_body" => return None,
            "comment" => {
                // コメントのCRLFと=beginの報告範囲は共通の原文座標への写像を使う。
                return context
                    .comment_ranges()
                    .iter()
                    .find(|comment| comment.start == node.start_byte())
                    .cloned();
            }
            _ => {}
        }
        if let Some(first) = context.children(node).and_then(|mut children| children.next()) {
            node = first;
            continue;
        }
        let range = node.byte_range();
        return (!range.is_empty())
            .then(|| string_token_range(context, range.start).unwrap_or(range));
    }
}

/// whitequarkはNUL入り文字列を分割し、Prismは単一tokenにする。二重引用の行継続だけは両者とも
/// 物理行をまたいだtSTRINGになるため、構文ノードの引用符と先頭tokenの範囲を分ける。
fn string_token_range(context: &RuleContext<'_>, start: usize) -> Option<Range<usize>> {
    let leaf = context
        .root_node()
        .descendant_for_byte_range(start, start + 1)?;
    let literal = if leaf.kind_str() == "string" {
        leaf
    } else {
        leaf.parent_of(context)?
    };
    if literal.kind_str() != "string" || literal.start_byte() != start {
        return None;
    }
    let text = context.source.node_text(literal);
    let bytes = text.as_bytes();
    let quote = *bytes.first()?;
    if quote == b'%' {
        // percent literalの先頭tokenは文字列全体ではなく導入子まで。
        return literal.child(0).map(|opening| opening.byte_range());
    }
    if !matches!(quote, b'\'' | b'"') || bytes.last() != Some(&quote) {
        return None;
    }
    let interpolates = context
        .named_children(literal)?
        .iter()
        .any(|node| node.kind_str() == "interpolation");
    let mut single = !interpolates
        && !(context.parser_engine() == ParserEngine::Whitequark && bytes.contains(&0));
    let mut index = 1;
    while single && index < bytes.len() - 1 {
        match bytes[index] {
            b'\n' => single = false,
            b'\\' if index + 1 < bytes.len() - 1 => {
                let next = bytes[index + 1];
                if next == b'\n' && quote == b'\'' {
                    single = false;
                } else if next == b'\r'
                    && bytes.get(index + 2) == Some(&b'\n')
                    && quote == b'"'
                {
                    index += 1;
                }
                index += 1;
            }
            _ => {}
        }
        index += 1;
    }
    Some(if single {
        literal.byte_range()
    } else {
        start..start + 1
    })
}
