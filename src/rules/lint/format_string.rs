//! `Utils::FormatString`, the scanner behind `Lint/FormatParameterMismatch`.
//!
//! Upstream matches one regexp against the whole literal. It is written with a negative lookbehind
//! and three ordered alternatives, so it is spelled out here as a scan that tries the same shapes
//! in the same order -- the order is what decides which of `%<a>d` and `%{a}` a sequence is.

/// One `%...` sequence the literal holds.
pub(super) struct Sequence {
    /// `match[0]`, which `arity` and `max_digit_dollar_num` are read off.
    source: String,
    pub name: Option<String>,
    pub is_percent: bool,
}

impl Sequence {
    /// `arity`: one argument, plus one for every `*` the width or precision takes.
    pub(super) fn arity(&self) -> usize {
        self.source.matches('*').count() + 1
    }

    /// `max_digit_dollar_num`: the highest `N$` the sequence names.
    pub(super) fn max_digit_dollar_num(&self) -> Option<String> {
        digit_dollar_numbers(&self.source)
            .into_iter()
            .max_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)))
    }
}

/// `FormatString#format_sequences`.
pub(super) fn parse(source: &str) -> Vec<Sequence> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }
        match sequence(bytes, index) {
            Some((end, sequence)) => {
                found.push(Sequence {
                    source: source[index..end].to_owned(),
                    ..sequence
                });
                index = end;
            }
            None => index += 1,
        }
    }
    found
}

/// `FormatString#valid?`: whether the literal mixes named, numbered and unnumbered sequences.
pub(super) fn is_valid(sequences: &[Sequence]) -> bool {
    let mut kinds: Vec<u8> = Vec::new();
    for sequence in sequences.iter().filter(|sequence| !sequence.is_percent) {
        let kind = if sequence.name.is_some() {
            0
        } else if sequence.max_digit_dollar_num().is_some() {
            1
        } else {
            2
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds.len() <= 1
}

/// One sequence starting at the `%`, if the literal has one there.
fn sequence(bytes: &[u8], start: usize) -> Option<(usize, Sequence)> {
    let after_percent = start + 1;
    if bytes.get(after_percent) == Some(&b'%') {
        return Some((
            after_percent + 1,
            Sequence {
                source: String::new(),
                name: None,
                is_percent: true,
            },
        ));
    }
    // `FLAG*` is greedy but the regexp backtracks into it when its last flag can begin the width
    // too. The important real-world case is `%-#{width}s`: `#` is a flag on its own and the first
    // byte of an interpolated width, so trying only the longest flag run loses that sequence.
    for index in flag_candidates(bytes, after_percent) {
        // The three ordered alternatives that end in a conversion type.
        for shape in [
            Shape::WidthPrecisionName,
            Shape::WidthNamePrecision,
            Shape::NameFlags,
        ] {
            if let Some((end, name)) = shape.matches(bytes, index) {
                return Some((
                    end,
                    Sequence {
                        source: String::new(),
                        name,
                        is_percent: false,
                    },
                ));
            }
        }
        // `%{name}`, whose `{` must not be the one an interpolation opened.
        let after_width = optional(bytes, index, width);
        let after_precision = optional(bytes, after_width, precision);
        if let Some((end, name)) = template_name(bytes, after_precision) {
            return Some((
                end,
                Sequence {
                    source: String::new(),
                    name: Some(name),
                    is_percent: false,
                },
            ));
        }
    }
    None
}

/// The three ordered shapes upstream's alternation lists before the template form.
enum Shape {
    WidthPrecisionName,
    WidthNamePrecision,
    NameFlags,
}

impl Shape {
    /// The end of the match and the name it captured, trying the optional parts greedily first.
    fn matches(&self, bytes: &[u8], start: usize) -> Option<(usize, Option<String>)> {
        match self {
            Self::WidthPrecisionName => {
                for after_width in candidates(bytes, start, width) {
                    for after_precision in candidates(bytes, after_width, precision) {
                        for (after_name, name) in named_candidates(bytes, after_precision) {
                            if let Some(end) = conversion(bytes, after_name) {
                                return Some((end, name));
                            }
                        }
                    }
                }
                None
            }
            Self::WidthNamePrecision => {
                for after_width in candidates(bytes, start, width) {
                    let Some((after_name, name)) = name(bytes, after_width) else {
                        continue;
                    };
                    for after_precision in candidates(bytes, after_name, precision) {
                        if let Some(end) = conversion(bytes, after_precision) {
                            return Some((end, Some(name.clone())));
                        }
                    }
                }
                None
            }
            Self::NameFlags => {
                let (after_name, name) = name(bytes, start)?;
                for after_flags in flag_candidates(bytes, after_name) {
                    for after_width in candidates(bytes, after_flags, width) {
                        for after_precision in candidates(bytes, after_width, precision) {
                            if let Some(end) = conversion(bytes, after_precision) {
                                return Some((end, Some(name.clone())));
                            }
                        }
                    }
                }
                None
            }
        }
    }
}

/// The two ways an optional part can match, greedy first.
fn candidates(bytes: &[u8], start: usize, part: fn(&[u8], usize) -> Option<usize>) -> Vec<usize> {
    match part(bytes, start) {
        Some(end) if end != start => vec![end, start],
        _ => vec![start],
    }
}

fn named_candidates(bytes: &[u8], start: usize) -> Vec<(usize, Option<String>)> {
    match name(bytes, start) {
        Some((end, name)) => vec![(end, Some(name)), (start, None)],
        None => vec![(start, None)],
    }
}

fn optional(bytes: &[u8], start: usize, part: fn(&[u8], usize) -> Option<usize>) -> usize {
    part(bytes, start).unwrap_or(start)
}

/// Every end position `FLAG*` can backtrack to, longest first.
fn flag_candidates(bytes: &[u8], start: usize) -> Vec<usize> {
    let mut index = start;
    let mut ends = vec![start];
    loop {
        match bytes.get(index) {
            Some(b' ' | b'#' | b'0' | b'+' | b'-') => {
                index += 1;
                ends.push(index);
            }
            Some(byte) if byte.is_ascii_digit() => match digit_dollar(bytes, index) {
                Some(end) => {
                    index = end;
                    ends.push(index);
                }
                None => break,
            },
            _ => break,
        }
    }
    ends.reverse();
    ends
}

/// `\d+\$`.
fn digit_dollar(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    (index > start && bytes.get(index) == Some(&b'$')).then_some(index + 1)
}

/// `NUMBER`: a plain number, a `*` argument, or an interpolation.
fn width(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'*' => {
            let index = start + 1;
            Some(digit_dollar(bytes, index).unwrap_or(index))
        }
        b'#' if bytes.get(start + 1) == Some(&b'{') => {
            let mut index = start + 2;
            while index < bytes.len() && bytes[index] != b'}' {
                index += 1;
            }
            (index < bytes.len()).then_some(index + 1)
        }
        byte if byte.is_ascii_digit() => {
            let mut index = start;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
            Some(index)
        }
        _ => None,
    }
}

/// `\.NUMBER?`.
fn precision(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'.') {
        return None;
    }
    Some(width(bytes, start + 1).unwrap_or(start + 1))
}

/// `<name>`.
fn name(bytes: &[u8], start: usize) -> Option<(usize, String)> {
    if bytes.get(start) != Some(&b'<') {
        return None;
    }
    let mut index = start + 1;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        index += 1;
    }
    if index == start + 1 || bytes.get(index) != Some(&b'>') {
        return None;
    }
    Some((
        index + 1,
        String::from_utf8_lossy(&bytes[start + 1..index]).into_owned(),
    ))
}

/// `(?<!\#)\{name\}`.
fn template_name(bytes: &[u8], start: usize) -> Option<(usize, String)> {
    if bytes.get(start) != Some(&b'{') || (start > 0 && bytes[start - 1] == b'#') {
        return None;
    }
    let mut index = start + 1;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        index += 1;
    }
    if index == start + 1 || bytes.get(index) != Some(&b'}') {
        return None;
    }
    Some((
        index + 1,
        String::from_utf8_lossy(&bytes[start + 1..index]).into_owned(),
    ))
}

/// `TYPE`.
fn conversion(bytes: &[u8], start: usize) -> Option<usize> {
    const TYPES: &[u8] = b"bBdiouxXeEfgGaAcps";
    TYPES.contains(bytes.get(start)?).then_some(start + 1)
}

fn digit_dollar_numbers(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit()
            && let Some(end) = digit_dollar(bytes, index)
        {
            let digits = source[index..end - 1].trim_start_matches('0');
            found.push(if digits.is_empty() {
                "0".to_owned()
            } else {
                digits.to_owned()
            });
            index = end;
            continue;
        }
        index += 1;
    }
    found
}
