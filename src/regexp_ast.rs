//! 選択した parser の字句規則、`match_with_lvasgn` の判定、静的な正規表現の本文。
//!
//! whitequark の `static_regexp_node` は 3.3 未満では `str` と `begin` だけの補間を
//! 連結する。Ruby の実行時評価と異なり、複文の最後だけでなく全 `str` を連結する。
//! 3.3 以降と Prism は、補間が 1 つでもあれば通常の send のままにする。

use sonicop_onigmo_compat::Encoding;
use tree_sitter::Node;

use crate::config::Config;
use crate::engine::{LiteralEncoding, declared_literal_encoding};
use crate::ruby_version::RubyVersion;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Semantics {
    static_interpolation: bool,
    prism_question_literals: bool,
    physical_binding: bool,
}

impl Default for Semantics {
    fn default() -> Self {
        Self {
            static_interpolation: false,
            prism_question_literals: true,
            physical_binding: false,
        }
    }
}

impl Semantics {
    pub(crate) fn from_config(config: &Config) -> Self {
        let engine = config.cop_value::<String>("AllCops", "ParserEngine");
        let prism_question_literals = match engine.as_deref() {
            Some("parser_prism") => true,
            Some("parser_whitequark") => false,
            _ => config.target_ruby_version() >= RubyVersion::new(3, 3),
        };
        Self {
            physical_binding: false,
            static_interpolation: config.target_ruby_version() < RubyVersion::new(3, 3)
                && engine.as_deref() != Some("parser_prism"),
            prism_question_literals,
        }
    }

    /// `?あい` を三項と読むのは Prism と現 Ruby。whitequark は最初の Unicode を文字として読む。
    pub(crate) fn modern_question_literals(self) -> bool {
        self.prism_question_literals
    }

    /// RuboCop が match_with_lvasgn として dispatch する条件。実行時の束縛とは別に使う。
    pub(crate) fn is_static<'t>(
        self,
        regex: Node<'t>,
        source: &str,
        heredoc: impl Fn(Node<'t>) -> Option<Node<'t>>,
    ) -> bool {
        if regex.kind() != "regex" {
            return false;
        }
        children(regex).into_iter().all(|part| {
            part.kind() != "interpolation"
                || self.static_interpolation && static_string(part, source, &heredoc)
        })
    }

    /// 補正の安全性だけは、Buffer の UTF-8 化前の物理 Ruby の束縛を別解析する。
    pub(crate) fn physical_binding(self) -> Self {
        Self {
            physical_binding: true,
            prism_question_literals: true,
            ..self
        }
    }

    /// 診断は本家の Symbol 同一性へ合わせ、補間では #31 の安全な名前の積集合を保つ。
    pub(crate) fn named_groups<'t>(
        self,
        regex: Node<'t>,
        source: &str,
        heredoc: impl Fn(Node<'t>) -> Option<Node<'t>>,
    ) -> Vec<String> {
        let interpolated = children(regex)
            .iter()
            .any(|part| part.kind() == "interpolation");
        // 非補間は既存の字句 scanner を候補の選別に使い、名前の確定だけ native へ渡す。
        if regex.kind() == "regex"
            && !interpolated
            && crate::rules::literal_named_groups(&source[regex.byte_range()]).is_empty()
        {
            return Vec::new();
        }
        if self.physical_binding {
            return match self.physical_groups(regex, source, heredoc) {
                crate::regexp_validation::LiteralNames::Valid(names) => names,
                crate::regexp_validation::LiteralNames::Invalid
                | crate::regexp_validation::LiteralNames::Unavailable => Vec::new(),
            };
        }
        let encoding = buffer_encoding(source);
        let Some((pattern, flags)) = self.pattern(regex, source, &heredoc, false, encoding) else {
            return Vec::new();
        };
        let mut safe = white_names(pattern, &flags, source).unwrap_or_default();
        if interpolated {
            let Some((physical, _)) = self.pattern(regex, source, &heredoc, true, encoding) else {
                return Vec::new();
            };
            let physical: std::collections::HashSet<_> = white_names(physical, &flags, source)
                .unwrap_or_default()
                .into_iter()
                .collect();
            // 空 begin・先行文を連結する差は、#31 の意味保存契約を先に保つ。
            safe.retain(|name| physical.contains(name));
        }
        if let crate::regexp_validation::LiteralNames::Valid(physical) =
            self.physical_groups(regex, source, heredoc)
        {
            // 非補間でも Unicode escape の捕捉 Symbol と元ソースの識別子が異なり得る。
            // 有効な物理 literal の名前だけを残し、架空の local で method の引数を失わせない。
            safe.retain(|name| physical.contains(name));
        }
        // 無効・未対応の物理入力には意味保存を新たに断定せず、本家診断側の判定を保つ。
        safe
    }

    pub(crate) fn physical_groups<'t>(
        self,
        regex: Node<'t>,
        source: &str,
        heredoc: impl Fn(Node<'t>) -> Option<Node<'t>>,
    ) -> crate::regexp_validation::LiteralNames {
        use crate::regexp_validation::LiteralNames;
        let Some(encoding) = crate::engine::regexp_source_encoding(source) else {
            return LiteralNames::Unavailable;
        };
        if !self.is_static(regex, source, &heredoc) {
            return LiteralNames::Valid(Vec::new());
        }
        let Some((pattern, flags)) = self.pattern(regex, source, heredoc, true, encoding) else {
            return LiteralNames::Unavailable;
        };
        crate::regexp_validation::literal_names(&pattern.bytes, pattern.encoding, &flags, encoding)
    }

    fn pattern<'t>(
        self,
        regex: Node<'t>,
        source: &str,
        heredoc: impl Fn(Node<'t>) -> Option<Node<'t>>,
        physical: bool,
        encoding: Encoding,
    ) -> Option<(Encoded, String)> {
        if !self.is_static(regex, source, &heredoc) {
            return None;
        }
        let (opening, closing) = (regex.child(0)?, regex.child(regex.child_count() - 1)?);
        let close = source[closing.byte_range()].chars().next()?;
        let flags = source[closing.byte_range()][close.len_utf8()..].to_owned();
        let mut pattern = Joined::new();
        let mut position = opening.end_byte();
        for interpolation in children(regex)
            .into_iter()
            .filter(|part| part.kind() == "interpolation")
        {
            if position < interpolation.start_byte() {
                pattern.append(Encoded::raw(
                    &crate::rules::decode_regexp_compile_fragment(
                        &source[position..interpolation.start_byte()],
                        close,
                    ),
                    encoding,
                )?)?;
            }
            pattern.append(string_bytes(
                interpolation,
                source,
                &heredoc,
                physical,
                encoding,
            )?)?;
            position = interpolation.end_byte();
        }
        if position < closing.start_byte() {
            pattern.append(Encoded::raw(
                &crate::rules::decode_regexp_compile_fragment(
                    &source[position..closing.start_byte()],
                    close,
                ),
                encoding,
            )?)?;
        }
        Some((pattern.value, flags))
    }
}

/// 捕捉名を独自の Unicode・keyword 表で分類しない。物理 encoding のまま照会し、
/// 単純な local 名の候補だけを短い代入として解析して予約語を除く。
pub(crate) fn capture_name_is_local(name: &str, source: &str) -> Option<bool> {
    let encoding = crate::engine::regexp_source_encoding(source)?;
    let encoded = Encoded::raw(name, encoding)?;
    let label = match encoding {
        Encoding::Utf8 => "UTF-8",
        Encoding::Binary => "ASCII-8BIT",
        Encoding::UsAscii => "US-ASCII",
        Encoding::EucJp => "EUC-JP",
        Encoding::Windows31J => "Windows-31J",
    };
    if !sonicop_prism_compat::local_variable_name(&encoded.bytes, label)? {
        return Some(false);
    }
    let mut probe = format!("# encoding: {label}\n").into_bytes();
    probe.extend_from_slice(&encoded.bytes);
    probe.extend_from_slice(b" = nil\n");
    let parsed = ruby_prism::parse(&probe);
    let node = parsed.node();
    let program = node.as_program_node()?;
    let locals = program.locals();
    Some(
        parsed.errors().next().is_none()
            && locals.len() == 1
            && locals.first()?.as_slice() == encoded.bytes,
    )
}

fn buffer_encoding(source: &str) -> Encoding {
    match declared_literal_encoding(source) {
        LiteralEncoding::Text | LiteralEncoding::SevenBit => Encoding::Utf8,
        LiteralEncoding::Binary => Encoding::Binary,
    }
}

/// 再解析で符号化宣言を失う断片も、Ruby の String#== と同じ条件で比較する。
pub(crate) fn same_string_encoding(
    content: &str,
    single: bool,
    closing: Option<char>,
    encodings: [LiteralEncoding; 2],
    whitequark: bool,
) -> bool {
    let model = if whitequark {
        StringModel::Whitequark
    } else {
        StringModel::Mri27
    };
    let values = encodings.map(|encoding| {
        let encoding = match encoding {
            LiteralEncoding::Text | LiteralEncoding::SevenBit => Encoding::Utf8,
            LiteralEncoding::Binary => Encoding::Binary,
        };
        let mut value = decode_string(content, single, closing, encoding, model)?;
        // 仕様版の Prism の str は、Binary の原本では Unicode escape も Binary の値を保持する。
        // whitequark は同じ escape を UTF-8 にするため、物理 Ruby 用の復号規則は変更しない。
        if !whitequark && encoding == Encoding::Binary {
            value.encoding = Encoding::Binary;
        }
        Some(value)
    });
    let [Some(original), Some(reparsed)] = values else {
        return false;
    };
    original.bytes == reparsed.bytes
        && (original.encoding == reparsed.encoding || original.bytes.is_ascii())
}

/// 静的 regexp は断片側でもコンパイルする。動的な補間は parser もこの検証を行わない。
pub(crate) fn same_regexp_encoding<'tree>(
    regex: Node<'tree>,
    source: &str,
    semantics: Semantics,
    encodings: [LiteralEncoding; 2],
    whitequark: bool,
    heredoc: impl Fn(Node<'tree>) -> Option<Node<'tree>>,
) -> bool {
    // match_with_lvasgn の dispatch と違い、静的 str だけの補間は Prism もコンパイルする。
    let semantics = Semantics {
        static_interpolation: true,
        ..semantics
    };
    let regexp_encodings = encodings.map(|encoding| match encoding {
        LiteralEncoding::Text | LiteralEncoding::SevenBit => Encoding::Utf8,
        LiteralEncoding::Binary => Encoding::Binary,
    });
    if !semantics.is_static(regex, source, &heredoc) {
        let (Some(opening), Some(closing)) = (regex.child(0), regex.child(regex.child_count() - 1))
        else {
            return false;
        };
        let Some(close) = source[closing.byte_range()].chars().next() else {
            return false;
        };
        // 動的 regexp の \xFF は生の ASCII escape のまま AST に残る。
        // コンパイルせず、補間を挟む str 断片の bytes と符号化だけを照合する。
        let same_fragment = |fragment: &str| {
            let fragment = crate::rules::decode_regexp_compile_fragment(fragment, close);
            let [Some(original), Some(reparsed)] =
                regexp_encodings.map(|encoding| Encoded::raw(&fragment, encoding))
            else {
                return false;
            };
            original.bytes == reparsed.bytes
                && (original.encoding == reparsed.encoding || original.bytes.is_ascii())
        };
        let mut start = opening.end_byte();
        for interpolation in children(regex)
            .into_iter()
            .filter(|child| child.kind() == "interpolation")
        {
            if !same_fragment(&source[start..interpolation.start_byte()]) {
                return false;
            }
            start = interpolation.end_byte();
        }
        return same_fragment(&source[start..closing.start_byte()]);
    }
    let patterns = regexp_encodings
        .map(|encoding| semantics.pattern(regex, source, &heredoc, false, encoding));
    let [Some((original, _)), Some((reparsed, flags))] = patterns else {
        return false;
    };
    if original.bytes != reparsed.bytes
        || (original.encoding != reparsed.encoding && !original.bytes.is_ascii())
    {
        return false;
    }
    if whitequark {
        crate::regexp_validation::validate(&reparsed.bytes, encodings[1], &flags).is_ok()
    } else {
        matches!(
            crate::regexp_validation::literal_names(
                &reparsed.bytes,
                reparsed.encoding,
                &flags,
                reparsed.encoding,
            ),
            crate::regexp_validation::LiteralNames::Valid(_)
        )
    }
}

fn white_names(pattern: Encoded, flags: &str, source: &str) -> Option<Vec<String>> {
    // ASCII escape は regexp の構文記号を作らないので、名前の開始記号が無ければ native は不要。
    if !pattern
        .bytes
        .windows(3)
        .any(|part| part == b"(?<" || part == b"(?'")
    {
        return Some(Vec::new());
    }
    let encoding = match pattern.encoding {
        Encoding::Utf8 => LiteralEncoding::Text,
        Encoding::UsAscii => LiteralEncoding::SevenBit,
        Encoding::Binary => LiteralEncoding::Binary,
        _ => return None,
    };
    crate::regexp_validation::named_groups(
        &pattern.bytes,
        encoding,
        flags,
        if declared_literal_encoding(source) == LiteralEncoding::Binary {
            LiteralEncoding::Binary
        } else {
            LiteralEncoding::Text
        },
    )
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "comment" | "empty_statement" | "heredoc_body"))
        .collect()
}

fn character_node(node: Node<'_>, source: &str) -> bool {
    (node.kind() == "character"
        || node.kind() == "ERROR" && source[node.byte_range()].starts_with('?'))
        && character_end(source, node.start_byte(), StringModel::Whitequark)
            .is_some_and(|end| end >= node.end_byte())
}
/// 回復した character の外にある ERROR は残す。字句が実際に消費する領域だけをまとめる。
fn semantic_children<'t>(node: Node<'t>, source: &str) -> Option<Vec<Node<'t>>> {
    let mut result = Vec::new();
    let mut consumed = node.start_byte();
    let boundary = node.child(node.child_count().checked_sub(1)?)?.start_byte();
    for part in children(node) {
        if part.start_byte() < consumed {
            if part.end_byte() > consumed {
                return None;
            }
            continue;
        }
        if character_node(part, source) {
            let end = character_end(source, part.start_byte(), StringModel::Whitequark)?;
            if end != character_end(source, part.start_byte(), StringModel::Mri27)?
                || end < part.end_byte()
                || end > boundary
            {
                return None;
            }
            consumed = end;
        }
        result.push(part);
    }
    Some(result)
}

fn character_end(source: &str, start: usize, model: StringModel) -> Option<usize> {
    let raw = source.get(start..)?;
    let mut chars = raw.char_indices().peekable();
    if chars.next()?.1 != '?' {
        return None;
    }
    if chars.next()?.1 == '\\' {
        let mut terminal = chars.next()?.1;
        let mut modifiers = 0;
        let mut escaped = true;
        let mut control = false;
        let mut meta = false;
        while matches!(terminal, 'c' | 'C' | 'M') {
            let flag = if terminal == 'M' {
                &mut meta
            } else {
                &mut control
            };
            if *flag {
                return None;
            }
            *flag = true;
            modifiers += 1;
            if terminal != 'c' && chars.next()?.1 != '-' {
                return None;
            }
            terminal = chars.next()?.1;
            if terminal != '\\' {
                escaped = false;
                break;
            }
            terminal = chars.next()?.1;
        }
        if escaped && terminal == 'x' {
            let mut count = 0;
            while count < 2 && chars.peek().is_some_and(|(_, ch)| ch.is_ascii_hexdigit()) {
                chars.next();
                count += 1;
            }
            if count == 0 {
                return None;
            }
        } else if escaped
            && matches!(terminal, '0'..='7')
            && !(modifiers > 0 && matches!(model, StringModel::Whitequark))
        {
            for _ in 0..2 {
                if chars.peek().is_some_and(|(_, ch)| matches!(ch, '0'..='7')) {
                    chars.next();
                } else {
                    break;
                }
            }
        } else if escaped && terminal == 'u' {
            if modifiers > 0 {
                return None;
            }
            if chars.peek().is_some_and(|(_, ch)| *ch == '{') {
                chars.next();
                let mut points = String::new();
                loop {
                    let ch = chars.next()?.1;
                    if ch == '}' {
                        break;
                    }
                    points.push(ch);
                }
                let mut points = points.split_whitespace();
                char::from_u32(u32::from_str_radix(points.next()?, 16).ok()?)?;
                if points.next().is_some() {
                    return None;
                }
            } else {
                for _ in 0..4 {
                    if !chars.next()?.1.is_ascii_hexdigit() {
                        return None;
                    }
                }
            }
        }
    }
    Some(start + chars.peek().map_or(raw.len(), |(offset, _)| *offset))
}

fn character_bytes(raw: &str, encoding: Encoding, model: StringModel) -> Option<Encoded> {
    let end = character_end(raw, 0, model)?;
    if end != raw.len() {
        return None;
    }
    let payload = raw.strip_prefix('?')?;
    let mut value = if payload == "\\\n" || payload == "\\\r\n" {
        Encoded::raw("\n", encoding)?
    } else {
        decode_string(payload, false, None, encoding, model)?
    };
    if payload.starts_with("\\u")
        || value.bytes.is_ascii()
            && (matches!(model, StringModel::Whitequark)
                && matches!(
                    payload,
                    "\\a" | "\\b" | "\\e" | "\\f" | "\\n" | "\\r" | "\\s" | "\\t" | "\\v" | "\\\\"
                ))
    {
        value.encoding = Encoding::Utf8;
    }
    Some(value)
}
fn static_string<'t>(
    node: Node<'t>,
    source: &str,
    heredoc: &impl Fn(Node<'t>) -> Option<Node<'t>>,
) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind() {
            "interpolation" | "parenthesized_statements" => {
                let Some(parts) = semantic_children(current, source) else {
                    return false;
                };
                stack.extend(parts);
            }
            _ if character_node(current, source) => {}
            "string"
                if !children(current)
                    .iter()
                    .any(|part| part.kind() == "interpolation")
                    && !is_dstr(&source[current.byte_range()]) => {}
            "heredoc_beginning" if heredoc_content(current, source, heredoc).is_some() => {}
            _ => return false,
        }
    }
    true
}

/// parser の文字列は、行境界ごとの複数 str をまとめると dstr になる。
fn is_dstr(raw: &str) -> bool {
    let single = raw.starts_with('\'') || raw.starts_with("%q");
    let open = if let Some(rest) = raw.strip_prefix('%') {
        1 + rest.chars().next().map_or(0, |first| {
            first.len_utf8()
                + if first.is_ascii_alphabetic() {
                    rest[first.len_utf8()..]
                        .chars()
                        .next()
                        .map_or(0, char::len_utf8)
                } else {
                    0
                }
        })
    } else {
        1
    };
    let close = raw.chars().next_back().map_or(0, char::len_utf8);
    let normalized = raw[open..raw.len() - close].replace("\r\n", "\n");
    let mut chars = normalized.char_indices();
    let mut parts = 0;
    let mut start = 0;
    while let Some((index, character)) = chars.next() {
        match character {
            '\\' if !single => {
                chars.next();
            }
            '\n' => {
                parts += 1;
                start = index + 1;
            }
            _ => {}
        }
    }
    parts + usize::from(start < normalized.len()) > 1
}

fn heredoc_content<'a, 't>(
    node: Node<'t>,
    source: &'a str,
    heredoc: &impl Fn(Node<'t>) -> Option<Node<'t>>,
) -> Option<(&'a str, bool, bool)> {
    let opening = source[node.byte_range()]
        .trim_start_matches("<<")
        .trim_start_matches(['~', '-']);
    let quote = opening.chars().next()?;
    if quote == '`' {
        return None;
    }
    let body = heredoc(node)?;
    let parts = children(body);
    if parts.iter().any(|part| part.kind() == "interpolation") {
        return None;
    }
    let end = parts
        .iter()
        .find(|part| part.kind() == "heredoc_end")?
        .start_byte();
    let content = source[body.start_byte()..end]
        .strip_prefix("\r\n")
        .or_else(|| source[body.start_byte()..end].strip_prefix('\n'))?;
    let single = quote == '\'';
    // 終端前の 1 改行だけを許す。空や複数行の本文は dstr。
    let normalized = content.replace("\r\n", "\n");
    let mut breaks = 0;
    let mut bytes = normalized.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\' && !single {
            bytes.next();
        } else if byte == b'\n' {
            breaks += 1;
        }
    }
    (breaks == 1).then_some((
        content,
        single,
        source[node.byte_range()].starts_with("<<~"),
    ))
}

/// String#<< は最初の非 ASCII 部品の符号化を採用する。Unicode escape は原本が Binary
/// でも UTF-8 になるので、原本の符号化を結合後にかけ直してはいけない。
struct Encoded {
    bytes: Vec<u8>,
    encoding: Encoding,
}

impl Encoded {
    fn new(encoding: Encoding) -> Self {
        Self {
            bytes: Vec::new(),
            encoding,
        }
    }

    fn raw(text: &str, encoding: Encoding) -> Option<Self> {
        Some(Self {
            bytes: crate::regexp_validation::encode_fragment(text, encoding)?,
            encoding,
        })
    }

    fn append(&mut self, other: Self) -> Option<()> {
        if !other.bytes.is_ascii() {
            if !self.bytes.is_ascii() && self.encoding != other.encoding {
                return None;
            }
            self.encoding = other.encoding;
        }
        self.bytes.extend(other.bytes);
        Some(())
    }
}

// Array#join は全 ASCII でも先頭部品の符号化を保つ。String#<< の append と分ける。
struct Joined {
    value: Encoded,
    first: bool,
}

impl Joined {
    fn new() -> Self {
        Self {
            value: Encoded::new(Encoding::UsAscii),
            first: true,
        }
    }
    fn append(&mut self, other: Encoded) -> Option<()> {
        if self.first {
            self.value.encoding = other.encoding;
            self.first = false;
        }
        self.value.append(other)
    }
}

#[derive(Clone, Copy)]
enum StringModel {
    Whitequark,
    Mri27,
}

fn string_bytes<'t>(
    node: Node<'t>,
    source: &str,
    heredoc: &impl Fn(Node<'t>) -> Option<Node<'t>>,
    physical: bool,
    encoding: Encoding,
) -> Option<Encoded> {
    if physical {
        return physical_string_bytes(node, source, heredoc, encoding);
    }
    let mut out = Joined::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind() {
            "interpolation" | "parenthesized_statements" => {
                let mut parts = semantic_children(current, source)?;
                if parts.is_empty() {
                    // static_string([]) の空 join も、外側 Array#join の先頭部品になる。
                    out.append(Encoded::new(Encoding::UsAscii))?;
                }
                parts.reverse();
                stack.extend(parts);
            }
            _ => out.append(literal_bytes(
                current,
                source,
                heredoc,
                StringModel::Whitequark,
                encoding,
            )?)?,
        }
    }
    Some(out.value)
}

fn literal_bytes<'t>(
    node: Node<'t>,
    source: &str,
    heredoc: &impl Fn(Node<'t>) -> Option<Node<'t>>,
    model: StringModel,
    encoding: Encoding,
) -> Option<Encoded> {
    match node.kind() {
        _ if character_node(node, source) => {
            let end = character_end(source, node.start_byte(), model)?;
            character_bytes(&source[node.start_byte()..end], encoding, model)
        }
        "heredoc_beginning" => {
            let (content, single, squiggly) = heredoc_content(node, source, heredoc)?;
            // 行継続で 1 str になっても、字下げは各物理行から escape の復号前に除く。
            let dedented;
            let content = if squiggly {
                dedented = dedent_heredoc(content);
                &dedented
            } else {
                content
            };
            decode_string(content, single, None, encoding, model)
        }
        "string" => quoted_bytes(&source[node.byte_range()], encoding, model),
        _ => None,
    }
}
fn quoted_bytes(raw: &str, encoding: Encoding, model: StringModel) -> Option<Encoded> {
    let (body, single) = if let Some(body) = raw.strip_prefix("%q") {
        (body, true)
    } else if let Some(body) = raw.strip_prefix("%Q") {
        (body, false)
    } else {
        (raw, raw.starts_with('\''))
    };
    let opening = body.chars().next()?;
    let closing = match opening {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        other => other,
    };
    let content = body.get(opening.len_utf8()..)?.strip_suffix(closing)?;
    decode_string(content, single, Some(closing), encoding, model)
}
/// Ruby 2.7 の stmts / block_append / new_evstr の、静的候補に必要な構造だけ。
/// NODE_STR を先頭に持つ列は次の式へ置き換わるが、NODE_BEGIN(0) が一度入ると
/// NODE_BLOCK が残る。最後の str だけを選ぶと、中間の () が作る EVSTR を失う。
enum PhysicalString {
    Str(Encoded),
    EmptyBegin,
    Block,
}

fn physical_string_bytes<'t>(
    node: Node<'t>,
    source: &str,
    heredoc: &impl Fn(Node<'t>) -> Option<Node<'t>>,
    encoding: Encoding,
) -> Option<Encoded> {
    let mut stack = vec![(node, false)];
    let mut values = std::collections::HashMap::new();
    while let Some((current, visited)) = stack.pop() {
        if matches!(current.kind(), "interpolation" | "parenthesized_statements") {
            let parts = semantic_children(current, source)?;
            if !visited {
                stack.push((current, true));
                stack.extend(parts.into_iter().rev().map(|part| (part, false)));
                continue;
            }
            let mut cursor = current.walk();
            let leading_empty = current
                .named_children(&mut cursor)
                .find(|part| part.kind() != "comment")
                .is_some_and(|part| part.kind() == "empty_statement");
            let mut head =
                (leading_empty || parts.is_empty()).then_some(PhysicalString::EmptyBegin);
            for part in parts {
                let tail = values.remove(&part.id())?;
                head = Some(match head {
                    None | Some(PhysicalString::Str(_)) => tail,
                    Some(PhysicalString::EmptyBegin | PhysicalString::Block) => {
                        PhysicalString::Block
                    }
                });
            }
            values.insert(current.id(), head?);
        } else {
            values.insert(
                current.id(),
                PhysicalString::Str(literal_bytes(
                    current,
                    source,
                    heredoc,
                    StringModel::Mri27,
                    encoding,
                )?),
            );
        }
    }
    match values.remove(&node.id())? {
        PhysicalString::Str(bytes) => Some(bytes),
        PhysicalString::EmptyBegin | PhysicalString::Block => None,
    }
}
/// 空行を除く最小字下げを使い、tab は Ruby の 8 桁の境界で数える。
fn dedent_heredoc(content: &str) -> String {
    let indentation = |line: &str| {
        line.bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .fold(0, |column, byte| {
                column + if byte == b'\t' { 8 - column % 8 } else { 1 }
            })
    };
    let width = content
        .split_inclusive('\n')
        .filter(|line| !line.trim_matches([' ', '\t', '\r', '\n']).is_empty())
        .map(indentation)
        .min()
        .unwrap_or(0);
    let mut result = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        let mut column = 0;
        let mut start = 0;
        for byte in line.bytes() {
            let next = match byte {
                b' ' => column + 1,
                b'\t' => column + 8 - column % 8,
                _ => break,
            };
            if next > width {
                break;
            }
            column = next;
            start += 1;
        }
        result.push_str(&line[start..]);
    }
    result
}
/// 文字列のバイト escape と Unicode escape を分けて保持する。str の形の判定はこの前に
/// 終えてあり、dstr を実行時の文字列に展開して静的と取り違えない。
fn decode_string(
    content: &str,
    single: bool,
    closing: Option<char>,
    encoding: Encoding,
    model: StringModel,
) -> Option<Encoded> {
    let normalized = content.replace("\r\n", "\n");
    if single && closing.is_none() {
        return Encoded::raw(&normalized, encoding);
    }
    let chars: Vec<char> = normalized.chars().collect();
    let mut result = Encoded::new(encoding);
    let mut at = 0;
    while at < chars.len() {
        let character = chars[at];
        at += 1;
        if character != '\\' || at == chars.len() {
            result.append(Encoded::raw(&character.to_string(), encoding)?)?;
            continue;
        }
        let next = chars[at];
        if single {
            if next == '\\'
                || closing == Some(next)
                || closing.is_some_and(|close| {
                    matches!(
                        (next, close),
                        ('(', ')') | ('[', ']') | ('{', '}') | ('<', '>')
                    )
                })
            {
                at += 1;
                result.append(Encoded::raw(&next.to_string(), encoding)?)?;
            } else {
                result.bytes.push(b'\\');
            }
            continue;
        }
        at += 1;
        if next == '\n' {
            continue;
        }
        if next == 'u' {
            let mut value = String::new();
            if chars.get(at) == Some(&'{') {
                at += 1;
                let start = at;
                while chars.get(at) != Some(&'}') {
                    chars.get(at)?;
                    at += 1;
                }
                let points: String = chars[start..at].iter().collect();
                for point in points.split_whitespace() {
                    value.push(char::from_u32(u32::from_str_radix(point, 16).ok()?)?);
                }
                at += 1;
            } else {
                let code: String = chars.get(at..at + 4)?.iter().collect();
                at += 4;
                value.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
            }
            result.append(Encoded::raw(&value, Encoding::Utf8)?)?;
            continue;
        }
        let byte = if next == 'x' {
            let start = at;
            while at < (start + 2).min(chars.len()) && chars[at].is_ascii_hexdigit() {
                at += 1;
            }
            u8::from_str_radix(&chars[start..at].iter().collect::<String>(), 16).ok()?
        } else if matches!(next, '0'..='7') {
            let start = at - 1;
            while at < (start + 3).min(chars.len()) && matches!(chars[at], '0'..='7') {
                at += 1;
            }
            u16::from_str_radix(&chars[start..at].iter().collect::<String>(), 8).ok()? as u8
        } else if matches!(next, 'c' | 'C' | 'M') {
            let (mut control, mut meta) = (false, false);
            let mut modifiers = Vec::new();
            let mut modifier = next;
            loop {
                match modifier {
                    'c' => control = true,
                    'C' if chars.get(at) == Some(&'-') => {
                        control = true;
                        at += 1;
                    }
                    'M' if chars.get(at) == Some(&'-') => {
                        meta = true;
                        at += 1;
                    }
                    _ => return None,
                }
                modifiers.push(modifier != 'M');
                let mut terminal = *chars.get(at)?;
                let escaped = terminal == '\\';
                at += 1;
                if terminal == '\\' {
                    terminal = *chars.get(at)?;
                    at += 1;
                    if matches!(terminal, 'c' | 'C' | 'M') {
                        modifier = terminal;
                        continue;
                    }
                    // \M-\x43 のように modifier の終端にも byte escape が置ける。
                    // ここで ASCII の x と読むと、UTF-8 を組み立てた捕捉名を失う。
                    terminal = if terminal == 'x' {
                        let start = at;
                        while at < (start + 2).min(chars.len()) && chars[at].is_ascii_hexdigit() {
                            at += 1;
                        }
                        let byte = if matches!(model, StringModel::Whitequark) && at - start == 1 {
                            0
                        } else {
                            u8::from_str_radix(&chars[start..at].iter().collect::<String>(), 16)
                                .ok()?
                        };
                        if matches!(model, StringModel::Whitequark) {
                            // lexer-strings.rb は nested hex を slash_c_char → slash_m_char の順で読む。
                            // 単一の非 ASCII UTF-8 byte に対する String#ord は例外であり、正常値にしない。
                            if encoding == Encoding::Utf8 && byte >= 0x80 {
                                return None;
                            }
                            control = true;
                        }
                        char::from(byte)
                    } else if matches!(model, StringModel::Mri27) && matches!(terminal, '0'..='7') {
                        let start = at - 1;
                        while at < (start + 3).min(chars.len()) && matches!(chars[at], '0'..='7') {
                            at += 1;
                        }
                        char::from(
                            u16::from_str_radix(&chars[start..at].iter().collect::<String>(), 8)
                                .ok()? as u8,
                        )
                    } else {
                        match terminal {
                            'a' => '\x07',
                            'b' => '\x08',
                            't' => '\t',
                            'n' => '\n',
                            'v' => '\x0b',
                            'f' => '\x0c',
                            'r' => '\r',
                            'e' => '\x1b',
                            's' => ' ',
                            other => other,
                        }
                    };
                }
                let mut byte = u8::try_from(u32::from(terminal)).ok()?;
                if matches!(model, StringModel::Mri27) {
                    // MRI は内側から順に適用する。C-M-? は 9F、M-C-? は FF になる。
                    for (index, control) in modifiers.into_iter().rev().enumerate() {
                        if control {
                            byte = if index == 0 && !escaped && terminal == '?' {
                                0x7f
                            } else {
                                byte & 0x9f
                            };
                        } else {
                            byte |= 0x80;
                        }
                    }
                } else {
                    if control {
                        byte = if !escaped && terminal == '?' {
                            0x7f
                        } else {
                            byte & 0x9f
                        };
                    }
                    if meta {
                        byte |= 0x80;
                    }
                }
                break byte;
            }
        } else {
            match next {
                'a' => 7,
                'b' => 8,
                't' => 9,
                'n' => 10,
                'v' => 11,
                'f' => 12,
                'r' => 13,
                'e' => 27,
                's' => 32,
                other => {
                    result.append(Encoded::raw(&other.to_string(), encoding)?)?;
                    continue;
                }
            }
        };
        result.bytes.push(byte);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::Semantics;

    #[test]
    fn physical_capture_names_use_prisms_identifier_and_keyword_rules() {
        assert_eq!(
            sonicop_prism_compat::local_variable_name(b"name", "unknown"),
            None
        );
        assert_eq!(
            sonicop_prism_compat::local_variable_name(b"name", "UTF-8\0"),
            None
        );
        // 実 MRI 4.0.7 の local table と照合した。Regexp#names の非空だけでは束縛を証明できない。
        for (name, expected) in [
            ("name", true),
            ("Name", false),
            ("if", false),
            ("foo-bar", false),
            ("@name", false),
            ("あ", true),
            ("_name", true),
            ("_", true),
            ("__FILE__", false),
            ("foo?", false),
            ("Ä", false),
            ("it", true),
        ] {
            assert_eq!(
                super::capture_name_is_local(name, ""),
                Some(expected),
                "{name}"
            );
        }
        assert_eq!(
            super::capture_name_is_local("Ä", "# encoding: ASCII-8BIT\n"),
            Some(true)
        );
        assert_eq!(
            super::capture_name_is_local("name", "# encoding: UTF-7\n"),
            None
        );
    }

    #[test]
    fn configured_capture_names_keep_the_recorded_symbol_identity_in_both_models() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/capture_binding_encodings.json"
        ))
        .unwrap();
        let semantics = Semantics {
            static_interpolation: true,
            ..Semantics::default()
        };
        let mut checked = 0;
        let mut mismatches = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            let source = case["source"].as_str().unwrap();
            let tree = crate::parser::parse(source).unwrap();
            let mut pending = vec![tree.root_node()];
            let mut regex = None;
            while let Some(node) = pending.pop() {
                if node.kind() == "regex" {
                    regex = Some(node);
                    break;
                }
                pending.extend(super::children(node).into_iter().rev());
            }
            let regex = regex.unwrap();
            let buffer = case["white27"]["buffer_encoding"].as_str().unwrap();
            let mut upstream = Vec::new();
            for event in case["white27"]["trace"].as_array().unwrap() {
                let Some(declared) = event.get("declare") else {
                    continue;
                };
                let hex = declared["hex"].as_str().unwrap();
                let bytes: Vec<_> = hex
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect();
                if !bytes.is_ascii() && declared["encoding"] != buffer {
                    continue;
                }
                let name = if buffer == "ASCII-8BIT" {
                    bytes.into_iter().map(char::from).collect()
                } else {
                    String::from_utf8(bytes).unwrap()
                };
                if !upstream.contains(&name) {
                    upstream.push(name);
                }
            }
            let physical: Vec<String> = case["encoding_matched_names"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap().to_owned())
                .collect();
            // 物理的に有効な literal だけで積集合を証明する。無効原本の意味は保証しない。
            let expected = if case["mri27"]["status"] == "ok" {
                upstream
                    .into_iter()
                    .filter(|name| physical.contains(name))
                    .collect()
            } else {
                upstream
            };
            let actual = semantics.named_groups(regex, source, |_| None);
            if actual != expected {
                mismatches.push(format!("{}: {actual:?} / {expected:?}", case["id"]));
            }
            if case["mri27"]["status"] == "ok" {
                let actual = semantics
                    .physical_binding()
                    .named_groups(regex, source, |_| None);
                if actual != physical {
                    mismatches.push(format!(
                        "{} physical: {actual:?} / {physical:?}",
                        case["id"]
                    ));
                }
            }
            checked += 1;
        }
        assert_eq!(checked, 280);
        assert!(
            mismatches.is_empty(),
            "{} differences:\n{}",
            mismatches.len(),
            mismatches.join("\n")
        );
    }

    #[test]
    fn character_recovery_consumes_only_the_measured_lexeme() {
        let semantics = Semantics {
            static_interpolation: true,
            ..Semantics::default()
        };
        for expression in [r"?\C-\x43", r"?\c\x43", r"((?\C-\x43))", r"?\C-\x43; 'a'"] {
            let source = format!("/#{{{expression}}}(?<y>.)/ =~ text\n");
            let tree = crate::parser::parse(&source).unwrap();
            let regex = tree
                .root_node()
                .named_children(&mut tree.root_node().walk())
                .find(|node| node.kind() == "binary")
                .unwrap()
                .child_by_field_name("left")
                .unwrap();
            assert!(semantics.is_static(regex, &source, |_| None), "{source:?}");
            assert_eq!(
                semantics.named_groups(regex, &source, |_| None),
                ["y"],
                "{source:?}"
            );
        }
        // 字句外に残る式・ERROR・切れた escape を静的文字列へ読み替えない。
        for expression in [
            r"?\C-\x43; value",
            r"?\C-\x43 + ''",
            r"?\C-\x43x",
            r"?\C-\x43; raise('effect')",
        ] {
            let source = format!("/#{{{expression}}}(?<y>.)/ =~ text\n");
            let tree = crate::parser::parse(&source).unwrap();
            let regex = tree
                .root_node()
                .named_children(&mut tree.root_node().walk())
                .find(|node| node.kind() == "binary")
                .unwrap()
                .child_by_field_name("left")
                .unwrap();
            assert!(!semantics.is_static(regex, &source, |_| None), "{source:?}");
            assert!(
                semantics.named_groups(regex, &source, |_| None).is_empty(),
                "{source:?}"
            );
        }
        for raw in [r"?\C-", r"?\c\", r"?\M-\x", r"?\C-\C-a"] {
            assert!(
                super::character_end(raw, 0, super::StringModel::Mri27).is_none(),
                "{raw:?}"
            );
        }
    }
    #[test]
    fn literal_values_keep_the_measured_parser_and_mri_decoding_difference() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/regexp_literal_values.json"))
                .unwrap();
        assert_eq!(fixture["mri_runtime"]["version"], "2.7.8");
        assert_eq!(fixture["parser"]["version"], "3.3.12.0");
        let mut checked = 0;
        let mut failures = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            if case["classification"] != "literal_values" {
                continue;
            }
            let source = case["source"].as_str().unwrap();
            let encoding = super::buffer_encoding(source);
            for (key, model) in [
                ("parser27", super::StringModel::Whitequark),
                ("mri27", super::StringModel::Mri27),
            ] {
                // Parser が返す完全な leaf の原本範囲で primitive decoder を検査する。
                // 物理 tree の character 範囲欠落とは別の監査にする。
                let hex = case["parser27"]["literal_source_hex"].as_str().unwrap();
                let bytes: Vec<_> = hex
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect();
                let raw = if encoding == sonicop_onigmo_compat::Encoding::Binary {
                    bytes.into_iter().map(char::from).collect::<String>()
                } else {
                    String::from_utf8(bytes).unwrap()
                };
                let actual = if raw.starts_with('?') {
                    super::character_bytes(&raw, encoding, model)
                } else {
                    super::quoted_bytes(&raw, encoding, model)
                }
                .map(|value| {
                    (
                        value
                            .bytes
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>(),
                        if value.encoding == sonicop_onigmo_compat::Encoding::Binary {
                            "ASCII-8BIT"
                        } else {
                            "UTF-8"
                        },
                    )
                });
                let expected = (
                    case[key]["bytes_hex"].as_str().unwrap().to_owned(),
                    case[key]["encoding"].as_str().unwrap(),
                );
                if actual.as_ref() != Some(&expected) {
                    failures.push(format!("{} {key}: {actual:?} / {expected:?}", case["id"]));
                }
            }
            checked += 1;
        }
        assert_eq!(checked, 710);
        assert_eq!(
            fixture["summary"]["classifications"]["reference_error"],
            198
        );
        assert_eq!(fixture["summary"]["classifications"]["non_leaf"], 4);
        assert!(
            failures.is_empty(),
            "{} differences:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
    #[test]
    fn statement_folding_matches_the_real_ruby27_capture_bindings() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/regexp_ruby27_bindings.json"
        ))
        .unwrap();
        assert_eq!(fixture["ruby"]["version"], "2.7.8");
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 1872);
        let characters: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/regexp_ruby27_character_bindings.json"
        ))
        .unwrap();
        let character_cases = characters["cases"].as_array().unwrap();
        assert_eq!(character_cases.len(), 3132);
        let semantics = Semantics {
            static_interpolation: true,
            ..Semantics::default()
        };
        let mut failures = Vec::new();
        for case in cases.iter().chain(character_cases) {
            let source = case["source"].as_str().unwrap();
            let tree = crate::parser::parse(source).unwrap();
            let regex = tree
                .root_node()
                .named_children(&mut tree.root_node().walk())
                .find(|node| node.kind() == "binary")
                .unwrap()
                .child_by_field_name("left")
                .unwrap();
            let expected = case["declares_y"].as_bool().unwrap();
            let names = semantics.named_groups(regex, source, |_| None);
            if names != if expected { vec!["y"] } else { vec![] } {
                failures.push(format!("{source:?}: {names:?} / declares_y={expected}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} differences:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
    #[test]
    fn captures_do_not_invent_a_binding_from_an_empty_or_discarded_expression() {
        // Ruby 2.7.8 の実行と whitequark の AST を別々に確認した。後者は begin 全体を
        // 連結するが、前者は空 begin を残し、先行する値だけの文を捨てる。
        for (source, expected) in [
            (r"/#{'a'}(?<y>.)/", vec!["y"]),
            (r"/(?<y>.)#{}/", vec![]),
            (r"/#{; 'a';}(?<y>.)/", vec![]),
            (r"/#{('a';)}(?<y>.)/", vec!["y"]),
            (r"/#{'a'; 'b'}(?<y>.)/", vec!["y"]),
            (r"/#{'(?<discarded>.)'; 'a'}(?<kept>.)/", vec!["kept"]),
            (r"/#{'a'; '(?<kept>.)'}/", vec!["kept"]),
            (r"/#{'(?<b>.)'; '(?<a>.)'}/", vec!["a"]),
            (r"/#{'(?x)'; '#(?<hidden>.)'}/", vec![]),
            (r"/#{''}(?<y>.)/", vec!["y"]),
            (r"/#{('')}(?<y>.)/", vec!["y"]),
        ] {
            let tree = crate::parser::parse(source).unwrap();
            let regex = tree.root_node().named_child(0).unwrap();
            let semantics = Semantics {
                static_interpolation: true,
                ..Semantics::default()
            };
            assert!(semantics.is_static(regex, source, |_| None), "{source}");
            assert_eq!(
                semantics.named_groups(regex, source, |_| None),
                expected,
                "{source}"
            );
        }
    }
}
