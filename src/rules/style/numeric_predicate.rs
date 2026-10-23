use regex::Regex;
use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;
use crate::rules::send_node;

/// The version `positive?` and `negative?` were added in.
const PREDICATES_SINCE: crate::ruby_version::RubyVersion =
    crate::ruby_version::RubyVersion::new(2, 3);

/// 式展開を含む正規表現リテラルを、構文解析の時点で組み立てなくなる版。
const DYNAMIC_INTERPOLATION_SINCE: crate::ruby_version::RubyVersion =
    crate::ruby_version::RubyVersion::new(3, 3);

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let predicate_style = context
        .setting::<String>("EnforcedStyle")
        .is_none_or(|style| style == "predicate");
    let allowed = Allowed::new(context);

    for node in context.nodes_of_any(&["binary", "call"]) {
        let found = match predicate_style {
            true => comparison(context, node),
            false => predicate(context, node),
        };
        let Some((numeric, operator)) = found else {
            continue;
        };
        let selector = selector_name(context, node);
        if allowed.matches(selector.as_deref())
            || (allowed.is_set() && ancestor_is_allowed(context, node, &allowed))
        {
            continue;
        }
        // `replacement_supported?`: `positive?` and `negative?` arrived in 2.3, so a comparison is
        // left alone below that -- `zero?` has always been there.
        if matches!(operator, ">" | "<") && context.target_ruby_version() < PREDICATES_SINCE {
            continue;
        }

        let replacement = match predicate_style {
            true => format!(
                "{}.{}",
                parenthesized_source(context, numeric),
                predicate_name(operator)
            ),
            false => match negated(context, node) {
                true => format!("({} {} 0)", context.source.node_text(numeric), operator),
                false => format!("{} {} 0", context.source.node_text(numeric), operator),
            },
        };
        offenses.push(
            context
                .offense(
                    format!(
                        "Use `{replacement}` instead of `{}`.",
                        context.source.node_text(node)
                    ),
                    node.byte_range(),
                )
                .corrected_by(Edit {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement,
                    safe: false,
                }),
        );
    }
}

/// `comparison` and `inverted_comparison`: the operand that is not the literal zero, and the
/// operator as it reads from that operand's side.
fn comparison<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str)> {
    let (left, right, operator) = operands(context, node)?;
    let operator = ["==", ">", "<"]
        .into_iter()
        .find(|candidate| *candidate == operator)?;
    if is_zero(context, right) && left.kind_str() != "global_variable" {
        return Some((left, operator));
    }
    if is_zero(context, left) && right.kind_str() != "global_variable" {
        // `invert`: read from the other side, `>` and `<` swap.
        let inverted = match operator {
            ">" => "<",
            "<" => ">",
            other => other,
        };
        return Some((right, inverted));
    }
    None
}

/// `predicate`: `zero?`, `positive?` or `negative?` called on something.
fn predicate<'tree>(
    context: &RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, &'static str)> {
    // Upstream dispatches `on_send` but does not alias it to `on_csend`: a predicate reached
    // through `&.` is deliberately outside this cop even though the grammar calls both forms a
    // `call`.
    if node.kind_str() != "call"
        || !send_node::is_plain_send(node, context)
        || node.field("arguments").is_some()
    {
        return None;
    }
    let receiver = node.field("receiver")?;
    let method = node.field("method")?;
    let operator = match context.source.node_text(method) {
        "zero?" => "==",
        "positive?" => ">",
        "negative?" => "<",
        _ => return None,
    };
    Some((receiver, operator))
}

fn predicate_name(operator: &str) -> &'static str {
    match operator {
        ">" => "positive?",
        "<" => "negative?",
        _ => "zero?",
    }
}

/// The two operands and the selector of a binary send, whichever way it was spelled.
fn operands<'a, 'tree>(
    context: &'a RuleContext<'_>,
    node: Node<'tree>,
) -> Option<(Node<'tree>, Node<'tree>, &'a str)> {
    match node.kind_str() {
        "binary" => {
            let left = node.field("left")?;
            if super::nodes::is_bare_jump(left) {
                return None;
            }
            let operator = node.field("operator")?;
            Some((
                left,
                node.field("right")?,
                context.source.node_text(operator),
            ))
        }
        _ => {
            // `comparison` is a `(send …)` pattern, and `&.` is a `csend`: the grammar spells both
            // a `call`, so the operator reached through a safe navigation has to be excluded here.
            if !send_node::is_plain_send(node, context) {
                return None;
            }
            let method = node.field("method")?;
            let arguments = super::nodes::children(node.field("arguments")?);
            match arguments.as_slice() {
                [only] => Some((
                    node.field("receiver")?,
                    *only,
                    context.source.node_text(method),
                )),
                _ => None,
            }
        }
    }
}

/// `(int 0)`: the literal zero, however it was written.
fn is_zero(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let node = match node.kind_str() {
        "unary" => match node.field("operand") {
            Some(operand) if operand.kind_str() == "integer" => operand,
            _ => return false,
        },
        _ => node,
    };
    if node.kind_str() != "integer" {
        return false;
    }
    let digits = context.source.node_text(node).replace('_', "");
    let digits = ["0x", "0X", "0b", "0B", "0o", "0O", "0d", "0D"]
        .into_iter()
        .find_map(|prefix| digits.strip_prefix(prefix).map(str::to_owned))
        .unwrap_or(digits);
    !digits.is_empty() && digits.chars().all(|digit| digit == '0')
}

/// `parenthesized_source`: 括弧の無い演算子の呼び出しは、述語を繋ぐ前に括弧で包む。
fn parenthesized_source(context: &RuleContext<'_>, node: Node<'_>) -> String {
    let source = context.source.node_text(node);
    match require_parentheses(context, node) {
        true => format!("({source})"),
        false => source.to_owned(),
    }
}

/// `require_parentheses?`: `node.send_type? && node.binary_operation? && !node.parenthesized?`。
/// 本家の send のうち、演算子の selector が受け手の後ろにあり、呼び出し自身の括弧を持たないもの。
fn require_parentheses(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    match node.kind_str() {
        // `&&` と `||` は本家では send ではないので、二項演算でもない。読む時点で組み立てられる
        // 正規表現リテラルを左に書いた `=~` も `match_with_lvasgn` で、send ではない。
        "binary" => node.field("operator").is_some_and(|operator| {
            let operator = context.source.node_text(operator);
            super::nodes::is_operator_method(operator)
                && !(operator == "=~"
                    && node
                        .field("left")
                        .is_some_and(|left| is_static_regexp(context, left)))
        }),
        // `a[b]` は `:[]` の send で、`loc.begin` は括弧ではなく角括弧。
        "element_reference" => true,
        // `a[b] = c` は `:[]=` の send で、selector は同じ角括弧。属性代入 `a.b = c` の `b=` は
        // 演算子メソッドではなく、自己代入 `a[b] += c` は `op_asgn` で send ではない。
        "assignment" => node
            .field("left")
            .is_some_and(|left| left.kind_str() == "element_reference"),
        // `&.` の呼び出しは csend、ブロック付きの呼び出しは block ノードで、どちらも send ではない。
        "call" => {
            send_node::is_plain_send(node, context)
                && node.field("block").is_none()
                && node.field("receiver").is_some()
                && node.field("method").is_some_and(|method| {
                    super::nodes::is_operator_method(context.source.node_text(method))
                })
                && !has_own_parentheses(context, node)
        }
        _ => false,
    }
}

/// `parenthesized?`: `loc.end` が呼び出し自身の `)` であること。その `(` はメソッド名の直後に
/// 書かれる。空白を挟んだ `a.+ (b)` の `(b)` は引数の begin で、呼び出しは括弧を持たない。
fn has_own_parentheses(context: &RuleContext<'_>, call: Node<'_>) -> bool {
    match (call.field("method"), call.field("arguments")) {
        (Some(method), Some(arguments)) => {
            arguments.start_byte() == method.end_byte()
                && context.source.node_text(arguments).starts_with('(')
        }
        _ => false,
    }
}

/// `static_regexp_node`: 読む時点で組み立てられる正規表現リテラル。そうでないものを左に書いた
/// `=~` は send のまま。
///
/// 3.3 より前の parser は、文字列リテラルだけからなる式展開も組み立てる (`static_string` が
/// begin を辿る)。3.3 からは式展開を含むものを組み立てない。parser の `version >= 33` の分岐と、
/// 既定で使う Prism の翻訳が式展開のある正規表現を `match_op` に渡さないことの両方による。
fn is_static_regexp(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.kind_str() == "regex"
        && super::nodes::children_in(node, context)
            .into_iter()
            .filter(|part| part.kind_str() == "interpolation")
            .all(|interpolation| {
                context.target_ruby_version() < DYNAMIC_INTERPOLATION_SINCE
                    && is_static_string(context, interpolation)
            })
}

/// `static_string`: `str` と、それだけを並べた begin。
///
/// parser は文字列の本文を 1 行ずつの `str` に切るので、改行をまたぐ文字列リテラルは `dstr` に
/// なる。heredoc も同じ。
fn is_static_string(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind_str() {
            "interpolation" | "parenthesized_statements" => {
                stack.extend(super::nodes::children_in(current, context));
            }
            "heredoc_beginning" if is_str_heredoc(context, current) => {}
            _ if send_node::is_string(current, context) && !is_dstr(context, current) => {}
            _ => return false,
        }
    }
    true
}

/// 改行をまたいで `dstr` に切られる文字列リテラル。本家は CRLF を LF に正規化してから読むので、
/// CRLF のファイルでも `\` + 改行は行継続になる。
fn is_dstr(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let text = context.source.node_text(node);
    match text.contains("\r\n") {
        true => super::string_literals::is_dstr(&text.replace("\r\n", "\n")),
        false => super::string_literals::is_dstr(text),
    }
}

/// heredoc が `str` になるのは、本文がちょうど 1 行で式展開を含まないときだけ。空の本文と
/// 2 行以上の本文は `dstr`、バッククォートの heredoc は `xstr` になる。
fn is_str_heredoc(context: &RuleContext<'_>, beginning: Node<'_>) -> bool {
    let quote = context
        .source
        .node_text(beginning)
        .trim_start_matches("<<")
        .trim_start_matches(['~', '-'])
        .chars()
        .next();
    if quote == Some('`') {
        return false;
    }
    let Some(body) = send_node::heredoc_body(beginning, context) else {
        return false;
    };
    let parts = super::nodes::children_in(body, context);
    if parts.iter().any(|part| part.kind_str() == "interpolation") {
        return false;
    }
    let end = parts
        .iter()
        .find(|part| part.kind_str() == "heredoc_end")
        .map_or(body.end_byte(), |terminator| terminator.start_byte());
    let text = &context.source.text()[body.start_byte()..end];
    // 本文は、開始を書いた行を閉じる改行から始まる。
    let content = text
        .strip_prefix("\r\n")
        .or_else(|| text.strip_prefix('\n'))
        .unwrap_or(text);
    line_breaks(content, quote == Some('\'')) == 1
}

/// 本文の改行のうち、行末の `\` がつながないもの。単一引用符の heredoc の `\` は何もつながない。
fn line_breaks(content: &str, single_quoted: bool) -> usize {
    let bytes = content.as_bytes();
    let mut breaks = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            // CRLF のファイルでは、`\` がつなぐ改行が 2 バイトになる。
            b'\\' if !single_quoted && bytes[index + 1..].starts_with(b"\r\n") => index += 3,
            b'\\' if !single_quoted => index += 2,
            b'\n' => {
                breaks += 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    breaks
}

/// `negated?`: the comparison this cop is about to write sits under a `!`.
fn negated(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    node.parent_of(context).is_some_and(|parent| {
        parent.kind_str() == "unary"
            && parent
                .field("operator")
                .is_some_and(|operator| context.source.node_text(operator) == "!")
    })
}

fn selector_name(context: &RuleContext<'_>, node: Node<'_>) -> Option<String> {
    let selector = match node.kind_str() {
        "binary" => node.field("operator"),
        _ => node.field("method"),
    }?;
    Some(context.source.node_text(selector).to_owned())
}

/// `AllowedMethods` and `AllowedPatterns`, which spare a call and everything written inside it.
struct Allowed {
    methods: Vec<String>,
    patterns: Vec<Regex>,
}

impl Allowed {
    fn new(context: &RuleContext<'_>) -> Self {
        let patterns: Vec<String> = context.setting("AllowedPatterns").unwrap_or_default();
        Self {
            methods: context.setting("AllowedMethods").unwrap_or_default(),
            patterns: patterns
                .iter()
                .filter_map(|pattern| Regex::new(pattern).ok())
                .collect(),
        }
    }

    fn is_set(&self) -> bool {
        !self.methods.is_empty() || !self.patterns.is_empty()
    }

    fn matches(&self, name: Option<&str>) -> bool {
        let Some(name) = name else {
            return false;
        };
        self.methods.iter().any(|allowed| allowed == name)
            || self.patterns.iter().any(|pattern| pattern.is_match(name))
    }
}

/// `node.each_ancestor(:send, :any_block)`: a call or block written around this one.
fn ancestor_is_allowed(context: &RuleContext<'_>, node: Node<'_>, allowed: &Allowed) -> bool {
    let mut current = node.parent_of(context);
    while let Some(parent) = current {
        if matches!(
            parent.kind_str(),
            "binary" | "call" | "unary" | "element_reference"
        ) && allowed.matches(selector_name(context, parent).as_deref())
        {
            return true;
        }
        current = parent.parent_of(context);
    }
    false
}
