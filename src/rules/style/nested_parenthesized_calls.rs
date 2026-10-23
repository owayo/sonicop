use crate::diagnostic::{Edit, InsertionAnchor, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// `AllowedMethods`' default: the RSpec matchers whose argument reads better without parentheses.
const DEFAULT_ALLOWED: &[&str] = &[
    "be",
    "be_a",
    "be_an",
    "be_between",
    "be_falsey",
    "be_kind_of",
    "be_instance_of",
    "be_truthy",
    "be_within",
    "eq",
    "eql",
    "end_with",
    "include",
    "match",
    "raise_error",
    "respond_to",
    "start_with",
];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allowed: Vec<String> = context
        .setting::<Vec<String>>("AllowedMethods")
        .unwrap_or_else(|| {
            DEFAULT_ALLOWED
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        });

    for node in context.nodes_of("call") {
        // `node.parenthesized?`.
        let Some(arguments) = node.field("arguments") else {
            continue;
        };
        if !arguments
            .child(0)
            .is_some_and(|open| open.kind_str() == "(")
        {
            continue;
        }
        let outer_arguments = super::nodes::children_in(arguments, context);
        // `each_child_node(:call)` walks the receiver too, since it is a child of the send.
        let children = node
            .field("receiver")
            .into_iter()
            .chain(outer_arguments.iter().copied());
        for nested in children {
            // ブロック付きの呼び出しは本家では `send` ではなく `block` ノードで、
            // `each_child_node(:send)` に入らない (`update_git "bar" do … end.ref_for("HEAD")`)。
            // `&blk` は引数側の block_argument なので、この `block` フィールドには現れない。
            if nested.kind_str() != "call" || nested.field("block").is_some() {
                continue;
            }
            let Some(nested_arguments) = nested.field("arguments") else {
                continue;
            };
            let inner = super::nodes::children_in(nested_arguments, context);
            if inner.is_empty()
                || nested_arguments
                    .child(0)
                    .is_some_and(|open| open.kind_str() == "(")
                || reads_as_binary_and(context, nested_arguments, &inner)
            {
                continue;
            }
            let Some(method) = nested.field("method") else {
                continue;
            };
            let name = context.source.node_text(method);
            // `setter_method?` and `operator_method?`: neither can take parentheses at all.
            if name.ends_with('=') || super::nodes::is_operator_method(name) {
                continue;
            }
            if outer_arguments.len() == 1
                && inner.len() == 1
                && allowed.iter().any(|allowed| allowed == name)
            {
                continue;
            }
            let Some(first) = inner.first() else {
                continue;
            };
            // `insert_after(last_arg, ')')`: 閉じ括弧は最後の引数に掛ける。キーワード引数の並びは
            // 本家では 1 つの hash なので、その全体が最後の引数になる。入れ子の呼び出し全体に
            // 掛けると、最後の引数を覆う別 cop の編集との親子が本家と逆になる。
            let Some(last_argument) = send_node::arguments(nested)
                .last()
                .map(send_node::Argument::range)
            else {
                continue;
            };
            // 本家の SourceBuffer は CRLF を LF にしてから node.source を返す。
            // 表示する断片だけをそろえ、原本の範囲と補正のバイト列は維持する。
            let message = format!(
                "Add parentheses to nested method call `{}`.",
                context.source.node_text(nested).replace("\r\n", "\n")
            );
            offenses.push(
                context
                    .offense(message, nested.byte_range())
                    .corrected_by_all_anchored([
                        // `range_with_surrounding_space(first_arg.begin, side: :left,
                        // whitespace: true, continuations: true)` becomes the opening parenthesis.
                        (
                            Edit {
                                start: leading_space(context, first.start_byte()),
                                end: first.start_byte(),
                                replacement: "(".to_owned(),
                                safe: true,
                            },
                            None,
                        ),
                        InsertionAnchor::After(last_argument).insert(")"),
                    ]),
            );
        }
    }
}

/// Whether the grammar read an `&` that upstream's lexer takes for the binary operator.
///
/// `x.attr&0x8` is `(send (send _ :attr) :& (int 8))` upstream, because an `&` with no space before
/// it cannot open a block argument; the grammar spells it as a block argument all the same, which
/// would make the call look like one taking an argument without parentheses.
fn reads_as_binary_and(
    context: &RuleContext<'_>,
    arguments: tree_sitter::Node<'_>,
    inner: &[tree_sitter::Node<'_>],
) -> bool {
    let [only] = inner else {
        return false;
    };
    only.kind_str() == "block_argument"
        && only.start_byte() == arguments.start_byte()
        && !context.source.text()[..only.start_byte()].ends_with([' ', '\t', '\n'])
}

/// 本家の `range_with_surrounding_space(first_arg.begin, side: :left, whitespace: true,
/// continuations: true)`。行継続はバックスラッシュと LF/CRLF を 1 組として取り込む。
/// 改行だけを空白として消すとバックスラッシュが残り、補正の `(` がその後ろへ落ちる。
/// 字下げを挟む複数の継続は本家も構文を壊すため、共通 walk の段階順は変えず、
/// `continuation_writeback_divergences` と CLI テストで既存の原本保護を監視する。
fn leading_space(context: &RuleContext<'_>, start: usize) -> usize {
    crate::rules::support::final_pos(context.source.text(), start, false, true, true, true)
}
