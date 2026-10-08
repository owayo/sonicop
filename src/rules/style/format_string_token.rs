use std::ops::Range;

use tree_sitter::Node;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

use super::format_sequences::{Sequence, SequenceStyle, sequences};
use crate::rules::node_ext::NodeExt;

/// `aggressive` でも書式文字列として判定する、先頭引数が書式になるメソッド。
const FORMAT_METHODS: &[&str] = &["format", "sprintf", "printf"];

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    // 診断対象はすべて `%` で始まるため、それがないファイルでは文字列の収集を省く。
    if !context.source.text().contains('%') {
        return;
    }
    let style = parse_style(
        context
            .setting::<String>("EnforcedStyle")
            .as_deref()
            .unwrap_or("annotated"),
    );
    let max_unannotated: usize = context
        .setting("MaxUnannotatedPlaceholdersAllowed")
        .unwrap_or(1);
    // 本家は String と Symbol の両方に `to_sym` を呼ぶ。引用した ":conservative" は別名になる。
    let conservative =
        context
            .setting::<serde_yaml_ng::Value>("Mode")
            .is_some_and(|mode| match mode {
                serde_yaml_ng::Value::String(name) => name == "conservative",
                serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "ruby/symbol" => {
                    tagged.value.as_str() == Some("conservative")
                }
                serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "binary" => tagged
                    .value
                    .as_str()
                    .and_then(|encoded| super::redundant_argument::decode_binary(encoded).ok())
                    .is_some_and(|bytes| bytes == b"conservative"),
                // Psych は未知の scalar タグを無視し、値を通常の String として解決する。
                serde_yaml_ng::Value::Tagged(tagged)
                    if tagged.tag != "binary" && tagged.tag != "ruby/regexp" =>
                {
                    tagged.value.as_str() == Some("conservative")
                }
                _ => false,
            });
    // 本家はメソッド名の String と直接比較する。Symbol を String にデシリアライズすると
    // 設定が許可していない呼び出しまで診断対象から外れてしまう。
    let allowed_methods: Vec<String> = context
        .setting::<Vec<serde_yaml_ng::Value>>("AllowedMethods")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| match value {
            serde_yaml_ng::Value::String(name) => Some(name),
            serde_yaml_ng::Value::Tagged(tagged) if tagged.tag == "binary" => tagged
                .value
                .as_str()
                .and_then(|encoded| super::redundant_argument::decode_binary(encoded).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok()),
            serde_yaml_ng::Value::Tagged(tagged)
                if !tagged.tag.to_string().starts_with("!ruby/") =>
            {
                tagged.value.as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect();
    // `allowed_method?` と `matches_allowed_pattern?` は文字列ではなく、それを囲む呼び出し名を見る。
    let allowed_patterns =
        crate::rules::naming::support::forbidden_patterns_named(context, "AllowedPatterns");

    for literal in literals(context) {
        if surrounded_by_command_or_regexp(literal.anchor, context)
            || allowed_method(context, literal.anchor, &allowed_methods)
            || allowed_call_pattern(context, literal.anchor, &allowed_patterns)
        {
            continue;
        }
        // 本家の `format_string_in_typical_context?` は `on_str` が受け取るノードを調べる。
        // 補間で分割された文字列は、呼び出しの引数ではない `dstr` の子になる。
        // anchor は本家が調べるノードなので、分割数が一つであることを要求してはいけない。
        // 以前の条件では `format("c#{b}%{template}")` を報告しても修正できなかった。
        let typical = typical_context(context, literal.anchor);
        // 補間で分割されると、各文字列部分と呼び出しの間に `dstr` が入る。
        let split =
            literal.parts.len() > 1 || crate::rules::send_node::has_interpolation(literal.anchor);
        let correctable = typical || enclosing_typical_context(context, literal.anchor);

        for part in &literal.parts {
            let text = &context.source.text()[part.clone()];
            if !text.contains('%') {
                continue;
            }
            let detected: Vec<Sequence> = sequences(text)
                .into_iter()
                .filter(|sequence| sequence.style != SequenceStyle::Percent)
                .filter(|sequence| {
                    // 本家の `allowed_string?` は書式呼び出しの外にある無注釈 token を許し、
                    // conservative ではすべての token を許す。`on_str` が受け取るのは
                    // 文字列全体ではなく部分なので、補間された部分の親は呼び出しではない。
                    let allowed = sequence.style == SequenceStyle::Unannotated || conservative;
                    !(allowed && !(typical && !split))
                })
                .collect();
            if detected.is_empty() || allowed_unannotated(&detected, style, max_unannotated) {
                continue;
            }
            for sequence in &detected {
                report(context, part.start, sequence, style, correctable, offenses);
            }
        }
    }
}

/// One string literal, as the parts upstream's parser cuts it into.
struct Literal<'tree> {
    /// The node the context checks are made against: the heredoc's marker, or the literal itself.
    anchor: Node<'tree>,
    /// The byte ranges `str_contents` yields for each part.
    parts: Vec<Range<usize>>,
}

fn literals<'a>(context: &'a RuleContext<'_>) -> Vec<Literal<'a>> {
    let text = context.source.text();
    let mut found = Vec::new();

    for node in context.nodes_of("string") {
        let Some(open) = node.child(0) else { continue };
        let Some(close) = node.child(node.child_count().saturating_sub(1)) else {
            continue;
        };
        // `"%s" %[a]` is a `send` upstream: the second literal is the operator's argument, not a
        // string at all.
        if open.id() == close.id()
            || (super::percent::is_modulo_operand(node, context)
                && context.source.node_text(node).starts_with('%'))
        {
            continue;
        }
        let opener = context.source.node_text(open);
        let body = open.end_byte()..close.start_byte();
        let mut parts = split_parts(text, body, &interpolations(node), escapes_newline(opener));
        // A literal the parser keeps whole is one `str`, whose contents upstream takes as the
        // expression with a single character trimmed from each end rather than as its body.
        if parts.len() == 1 && node.end_byte() > node.start_byte() + 1 {
            parts.clear();
            parts.push(node.start_byte() + 1..node.end_byte() - 1);
        }
        found.push(Literal {
            anchor: node,
            parts,
        });
    }

    // A heredoc's marker and its body are separate nodes here; upstream has one literal whose
    // `loc.expression` is the marker and whose contents are the body.
    let markers: Vec<Node<'_>> = context.nodes_of("heredoc_beginning").collect();
    for (index, node) in context.nodes_of("heredoc_body").enumerate() {
        let Some(anchor) = markers.get(index) else {
            continue;
        };
        let end = node
            .child(node.child_count().saturating_sub(1))
            .filter(|child| child.kind_str() == "heredoc_end")
            .map_or(node.end_byte(), |child| child.start_byte());
        let quoted = context.source.node_text(*anchor).contains('\'');
        // The body node opens on the line break that closes the marker's line, while upstream's
        // `heredoc_body` starts on the line after it.
        let start =
            node.start_byte() + usize::from(text.as_bytes().get(node.start_byte()) == Some(&b'\n'));
        found.push(Literal {
            anchor: *anchor,
            parts: split_parts(text, start..end, &interpolations(node), !quoted),
        });
    }
    found
}

fn interpolations(node: Node<'_>) -> Vec<Range<usize>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind_str() == "interpolation")
        .map(|child| child.byte_range())
        .collect()
}

/// Whether the literal's delimiters let a backslash escape the line break that follows it, which
/// decides where the parser cuts the body into lines.
fn escapes_newline(opener: &str) -> bool {
    opener != "'" && !opener.starts_with("%q")
}

/// The `str` parts the parser builds: one per line, and one per run between interpolations.
fn split_parts(
    text: &str,
    body: Range<usize>,
    interpolations: &[Range<usize>],
    escapes_newline: bool,
) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = body.start;
    let mut offset = body.start;
    while offset < body.end {
        if let Some(span) = interpolations.iter().find(|span| span.start == offset) {
            if offset > start {
                parts.push(start..offset);
            }
            offset = span.end;
            start = offset;
            continue;
        }
        match bytes[offset] {
            b'\\' if escapes_newline => offset += 2,
            b'\n' => {
                parts.push(start..offset + 1);
                offset += 1;
                start = offset;
            }
            _ => offset += 1,
        }
    }
    if start < body.end {
        parts.push(start..body.end);
    }
    parts
}

/// `format_string_in_typical_context?`: the literal is the first argument of `format`, `sprintf` or
/// `printf`, or the receiver of `%`.
fn typical_context(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let Some(parent) = node.parent_of(context) else {
        return false;
    };
    if parent.kind_str() == "binary" {
        return parent
            .field("operator")
            .is_some_and(|operator| context.source.node_text(operator) == "%")
            && parent
                .field("left")
                .is_some_and(|left| left.id() == node.id());
    }
    // The grammar chains `"%s" %[a]` into one literal, where upstream has the `%` operator with the
    // first literal as its receiver.
    if parent.kind_str() == "chained_string" {
        return node.prev_named_sibling().is_none()
            && node
                .next_named_sibling()
                .is_some_and(|sibling| context.source.node_text(sibling).starts_with('%'));
    }
    if parent.kind_str() != "argument_list" {
        return false;
    }
    let Some(call) = parent
        .parent_of(context)
        .filter(|call| call.kind_str() == "call")
    else {
        return false;
    };
    let named = call
        .field("method")
        .is_some_and(|method| FORMAT_METHODS.contains(&context.source.node_text(method)));
    named
        && super::nodes::children_in(parent, context)
            .first()
            .is_some_and(|first| first.id() == node.id())
}

/// `node.each_ancestor(:dstr).any? { format_string_in_typical_context?(_1) }`: a literal handed to
/// `format` through an interpolation or an adjacent-literal chain is still a format string.
fn enclosing_typical_context(context: &RuleContext<'_>, node: Node<'_>) -> bool {
    let mut current = node.parent_of(context);
    while let Some(parent) = current {
        if matches!(
            parent.kind_str(),
            "chained_string" | "string" | "heredoc_body"
        ) && typical_context(context, parent)
        {
            return true;
        }
        current = parent.parent_of(context);
    }
    false
}

/// `format_string_token?`: a literal written inside a command or a regexp is none of this cop's
/// business.
fn surrounded_by_command_or_regexp(node: Node<'_>, context: &RuleContext<'_>) -> bool {
    let mut current = context.parent(node);
    while let Some(parent) = current {
        if matches!(parent.kind_str(), "subshell" | "regex") {
            return true;
        }
        current = context.parent(parent);
    }
    false
}

/// `use_allowed_method?`: the nearest enclosing call may be one the configuration exempts.
fn allowed_method(context: &RuleContext<'_>, node: Node<'_>, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return false;
    }
    let mut current = node.parent_of(context);
    while let Some(parent) = current {
        if parent.kind_str() == "call" {
            return parent.field("method").is_some_and(|method| {
                allowed
                    .iter()
                    .any(|name| name == context.source.node_text(method))
            });
        }
        current = parent.parent_of(context);
    }
    false
}

/// `allowed_unannotated?`: a handful of unannotated tokens is what the configuration tolerates.
fn allowed_unannotated(detected: &[Sequence], style: SequenceStyle, max: usize) -> bool {
    if !detected
        .iter()
        .all(|sequence| sequence.style == SequenceStyle::Unannotated)
    {
        return false;
    }
    detected.len() <= max
        || detected
            .iter()
            .any(|sequence| !correctable_sequence(sequence.kind, style))
}

/// `correctable_sequence?`.
fn correctable_sequence(kind: Option<char>, style: SequenceStyle) -> bool {
    kind == Some('s') || style == SequenceStyle::Annotated || style == SequenceStyle::Unannotated
}

fn report(
    context: &RuleContext<'_>,
    offset: usize,
    sequence: &Sequence,
    style: SequenceStyle,
    correctable: bool,
    offenses: &mut Vec<Offense>,
) {
    if sequence.style == style || !correctable_sequence(sequence.kind, style) {
        return;
    }
    let range = offset + sequence.begin..offset + sequence.end;
    let offense = context.offense(
        format!(
            "Prefer {} over {}.",
            describe(style),
            describe(sequence.style)
        ),
        range.clone(),
    );
    // The corrector rewrites the token only where the string is used as a format string, and only
    // when the token names something to carry over.
    offenses.push(
        match correctable.then(|| rewritten(sequence, style)).flatten() {
            Some(replacement) => offense.corrected_by(Edit {
                start: range.start,
                end: range.end,
                replacement,
                safe: true,
            }),
            None => offense,
        },
    );
}

/// `autocorrect_sequence`.
fn rewritten(sequence: &Sequence, style: SequenceStyle) -> Option<String> {
    if style == SequenceStyle::Unannotated {
        return None;
    }
    let name = sequence.name.as_ref()?;
    let flags = &sequence.flags;
    let width = &sequence.width;
    let precision = match sequence.precision.is_empty() {
        true => String::new(),
        false => format!(".{}", sequence.precision),
    };
    // A template token names no type, so the annotated form it becomes takes the default one.
    let kind = match sequence.style {
        SequenceStyle::Template => 's',
        _ => sequence.kind?,
    };
    Some(match style {
        SequenceStyle::Annotated => format!("%<{name}>{flags}{width}{precision}{kind}"),
        SequenceStyle::Template => format!("%{flags}{width}{precision}{{{name}}}"),
        _ => return None,
    })
}

fn parse_style(value: &str) -> SequenceStyle {
    match value {
        "template" => SequenceStyle::Template,
        "unannotated" => SequenceStyle::Unannotated,
        _ => SequenceStyle::Annotated,
    }
}

fn describe(style: SequenceStyle) -> &'static str {
    match style {
        SequenceStyle::Annotated => "annotated tokens (like `%<foo>s`)",
        SequenceStyle::Template => "template tokens (like `%{foo}`)",
        _ => "unannotated tokens (like `%s`)",
    }
}

/// `matches_allowed_pattern?(send_parent.method_name)`.
fn allowed_call_pattern(
    context: &RuleContext<'_>,
    anchor: Node<'_>,
    patterns: &[&regex::Regex],
) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let mut current = anchor.parent_of(context);
    while let Some(node) = current {
        if node.kind_str() == "call"
            && let Some(method) = node.field("method")
        {
            let name = context.source.node_text(method);
            return patterns.iter().any(|pattern| pattern.is_match(name));
        }
        current = node.parent_of(context);
    }
    false
}
