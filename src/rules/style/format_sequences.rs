//! `RuboCop::Cop::Utils::FormatString`, the scanner behind the format-string cops.
//!
//! Upstream spells the grammar as one regexp whose alternatives reuse the same capture names and
//! whose template branch needs a look-behind -- neither of which this engine's regexes allow -- so
//! the ordered choice and the greedy back-tracking are written out here instead.

/// The shape of one format sequence, which decides which style it belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SequenceStyle {
    /// `%<name>s`.
    Annotated,
    /// `%{name}`.
    Template,
    /// `%s`.
    Unannotated,
    /// `%%`, which every caller skips.
    Percent,
}

pub(super) struct Sequence {
    /// Byte offsets into the scanned text.
    pub begin: usize,
    pub end: usize,
    pub flags: String,
    pub width: String,
    pub precision: String,
    pub name: Option<String>,
    /// The type character, absent from a template sequence.
    pub kind: Option<char>,
    pub style: SequenceStyle,
}

/// The type characters `sprintf` accepts.
const TYPES: &[u8] = b"bBdiouxXeEfgGaAcps";

/// Every format sequence in `text`, in source order, as `String#scan` finds them.
pub(super) fn sequences(text: &str) -> Vec<Sequence> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        match match_at(text, bytes, offset) {
            Some(sequence) => {
                offset = sequence.end;
                found.push(sequence);
            }
            None => offset += next_boundary(text, offset),
        }
    }
    found
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text[offset..].chars().next().map_or(1, char::len_utf8)
}

fn match_at(text: &str, bytes: &[u8], start: usize) -> Option<Sequence> {
    if bytes.get(start) != Some(&b'%') {
        return None;
    }
    if bytes.get(start + 1) == Some(&b'%') {
        return Some(Sequence {
            begin: start,
            end: start + 2,
            flags: String::new(),
            width: String::new(),
            precision: String::new(),
            name: None,
            kind: Some('%'),
            style: SequenceStyle::Percent,
        });
    }

    // `FLAG*` is greedy, so the longest run is tried first and given up one flag at a time.
    let mut flag_ends = vec![start + 1];
    let mut cursor = start + 1;
    while let Some(next) = match_flag(bytes, cursor) {
        cursor = next;
        flag_ends.push(cursor);
    }
    for flag_end in flag_ends.iter().rev() {
        let flags = text[start + 1..*flag_end].to_owned();
        if let Some(sequence) = match_body(text, bytes, start, *flag_end, flags) {
            return Some(sequence);
        }
    }
    None
}

/// The alternation after the flags: three shapes closed by a type character, then the template one.
fn match_body(
    text: &str,
    bytes: &[u8],
    start: usize,
    after_flags: usize,
    flags: String,
) -> Option<Sequence> {
    // `WIDTH? PRECISION? NAME?` TYPE
    for width in optional(number_ends(text, bytes, after_flags)) {
        let after_width = width.unwrap_or(after_flags);
        for precision in optional(precision_ends(text, bytes, after_width)) {
            let after_precision = precision.unwrap_or(after_width);
            for name in optional(name_end(bytes, after_precision).into_iter().collect()) {
                let position = name.unwrap_or(after_precision);
                if let Some(kind) = type_at(bytes, position) {
                    return Some(build(
                        text,
                        start,
                        position + 1,
                        flags,
                        span(text, after_flags, width),
                        precision_text(text, after_width, precision),
                        name.map(|end| text[after_precision + 1..end - 1].to_owned()),
                        Some(kind),
                    ));
                }
            }
        }
    }
    // `WIDTH? NAME PRECISION?` TYPE
    for width in optional(number_ends(text, bytes, after_flags)) {
        let after_width = width.unwrap_or(after_flags);
        let Some(after_name) = name_end(bytes, after_width) else {
            continue;
        };
        for precision in optional(precision_ends(text, bytes, after_name)) {
            let position = precision.unwrap_or(after_name);
            if let Some(kind) = type_at(bytes, position) {
                return Some(build(
                    text,
                    start,
                    position + 1,
                    flags,
                    span(text, after_flags, width),
                    precision_text(text, after_name, precision),
                    Some(text[after_width + 1..after_name - 1].to_owned()),
                    Some(kind),
                ));
            }
        }
    }
    // `NAME FLAG* WIDTH? PRECISION?` TYPE
    if let Some(after_name) = name_end(bytes, after_flags) {
        let mut more = after_name;
        while let Some(next) = match_flag(bytes, more) {
            more = next;
        }
        let extra = text[after_name..more].to_owned();
        for width in optional(number_ends(text, bytes, more)) {
            let after_width = width.unwrap_or(more);
            for precision in optional(precision_ends(text, bytes, after_width)) {
                let position = precision.unwrap_or(after_width);
                if let Some(kind) = type_at(bytes, position) {
                    return Some(build(
                        text,
                        start,
                        position + 1,
                        format!("{flags}{extra}"),
                        span(text, more, width),
                        precision_text(text, after_width, precision),
                        Some(text[after_flags + 1..after_name - 1].to_owned()),
                        Some(kind),
                    ));
                }
            }
        }
    }
    // `WIDTH? PRECISION? TEMPLATE_NAME`
    for width in optional(number_ends(text, bytes, after_flags)) {
        let after_width = width.unwrap_or(after_flags);
        for precision in optional(precision_ends(text, bytes, after_width)) {
            let position = precision.unwrap_or(after_width);
            if let Some(end) = template_name_end(bytes, position) {
                return Some(build(
                    text,
                    start,
                    end,
                    flags,
                    span(text, after_flags, width),
                    precision_text(text, after_width, precision),
                    Some(text[position + 1..end - 1].to_owned()),
                    None,
                ));
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn build(
    text: &str,
    begin: usize,
    end: usize,
    flags: String,
    width: String,
    precision: String,
    name: Option<String>,
    kind: Option<char>,
) -> Sequence {
    let source = &text[begin..end];
    let style = match &name {
        Some(_) if source.contains('<') => SequenceStyle::Annotated,
        Some(_) if source.contains('{') => SequenceStyle::Template,
        _ => SequenceStyle::Unannotated,
    };
    Sequence {
        begin,
        end,
        flags,
        width,
        precision,
        name,
        kind,
        style,
    }
}

/// A greedy `X?`: the match is tried before the absence of one.
fn optional(ends: Vec<usize>) -> Vec<Option<usize>> {
    let mut all: Vec<Option<usize>> = ends.into_iter().map(Some).collect();
    all.push(None);
    all
}

fn span(text: &str, start: usize, end: Option<usize>) -> String {
    end.map_or_else(String::new, |end| text[start..end].to_owned())
}

/// The `precision` capture, which excludes the `.` that introduces it.
fn precision_text(text: &str, start: usize, end: Option<usize>) -> String {
    end.map_or_else(String::new, |end| text[start + 1..end].to_owned())
}

/// `FLAG`: one of the sprintf flags, or an argument number.
fn match_flag(bytes: &[u8], position: usize) -> Option<usize> {
    match bytes.get(position)? {
        b' ' | b'#' | b'0' | b'+' | b'-' => Some(position + 1),
        _ => digit_dollar_end(bytes, position),
    }
}

/// `\d+\$`.
fn digit_dollar_end(bytes: &[u8], position: usize) -> Option<usize> {
    let mut cursor = position;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    (cursor > position && bytes.get(cursor) == Some(&b'$')).then_some(cursor + 1)
}

/// `NUMBER`, as the ends its alternatives can reach, longest first.
fn number_ends(text: &str, bytes: &[u8], position: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut digits = position;
    while bytes.get(digits).is_some_and(u8::is_ascii_digit) {
        digits += 1;
    }
    // `\d+` is greedy and gives one digit back at a time.
    ends.extend((position + 1..=digits).rev());
    if bytes.get(position) == Some(&b'*') {
        if let Some(end) = digit_dollar_end(bytes, position + 1) {
            ends.push(end);
        }
        ends.push(position + 1);
    }
    if let Some(end) = interpolation_end(text, bytes, position) {
        ends.push(end);
    }
    ends
}

/// `\#\{.*?\}`, whose `.` stops at a line break.
fn interpolation_end(text: &str, bytes: &[u8], position: usize) -> Option<usize> {
    if bytes.get(position) != Some(&b'#') || bytes.get(position + 1) != Some(&b'{') {
        return None;
    }
    let rest = &text[position + 2..];
    let close = rest.find('}')?;
    match rest[..close].contains('\n') {
        true => None,
        false => Some(position + 2 + close + 1),
    }
}

/// `\.(?<precision>NUMBER?)`.
fn precision_ends(text: &str, bytes: &[u8], position: usize) -> Vec<usize> {
    if bytes.get(position) != Some(&b'.') {
        return Vec::new();
    }
    let mut ends = number_ends(text, bytes, position + 1);
    ends.push(position + 1);
    ends
}

/// `<(?<name>\w+)>`.
fn name_end(bytes: &[u8], position: usize) -> Option<usize> {
    if bytes.get(position) != Some(&b'<') {
        return None;
    }
    let mut cursor = position + 1;
    while bytes.get(cursor).is_some_and(is_word_byte) {
        cursor += 1;
    }
    (cursor > position + 1 && bytes.get(cursor) == Some(&b'>')).then_some(cursor + 1)
}

/// `(?<!\#)\{(?<name>\w+)\}`.
fn template_name_end(bytes: &[u8], position: usize) -> Option<usize> {
    if bytes.get(position) != Some(&b'{') || position == 0 || bytes[position - 1] == b'#' {
        return None;
    }
    let mut cursor = position + 1;
    while bytes.get(cursor).is_some_and(is_word_byte) {
        cursor += 1;
    }
    (cursor > position + 1 && bytes.get(cursor) == Some(&b'}')).then_some(cursor + 1)
}

fn type_at(bytes: &[u8], position: usize) -> Option<char> {
    bytes
        .get(position)
        .filter(|byte| TYPES.contains(byte))
        .map(|byte| *byte as char)
}

fn is_word_byte(byte: &u8) -> bool {
    byte.is_ascii_alphanumeric() || *byte == b'_'
}
