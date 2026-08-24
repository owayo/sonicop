use std::ops::RangeInclusive;

use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;
use crate::rules::node_ext::NodeExt;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let allow_in_heredoc: bool = context.setting("AllowInHeredoc").unwrap_or(false);
    let heredocs = heredocs(context);
    let text = context.source.text();

    // `/[[:blank:]]\z/` and `sub(/[[:blank:]]+\z/, '')`: **the widest of Ruby's three whitespace
    // sets, and the only one that reaches past ASCII.** A line ending in a no-break space or an
    // ideographic space carries trailing whitespace upstream, and reading the run as `[' ', '\t']`
    // left those lines unreported. (A line that *begins* with one parses as an identifier, so the
    // judgement has to be made on the line's text rather than on a node.)
    //
    // **On a source that declares itself binary the set collapses back to ASCII.** `[[:blank:]]`
    // names Unicode's `Zs`, and an `ASCII-8BIT` string holds no Unicode characters for it to
    // name -- Ruby matches only tab and space there. The decoder maps each byte of such a file to
    // one `char` so that columns count bytes, which hands this cop U+00A0 for byte `0xA0`: the
    // tail of `à`, `Р` or `ภ` written in a comment. Reading that as a no-break space reported 35
    // lines of `ruby/ruby`'s `test/ruby/test_transcode.rb` that upstream leaves alone, and the
    // correction **deleted the byte**, leaving a lone `0xC3` where a character had been. The file
    // still parsed, so nothing downstream noticed.
    let blank: fn(char) -> bool = match crate::engine::declared_literal_encoding(text) {
        crate::engine::LiteralEncoding::Binary => |character| matches!(character, ' ' | '\t'),
        _ => crate::rules::support::is_ruby_blank,
    };
    // `processed_source.lines` stops at `__END__`; whitespace in the data section is data.
    for line_number in 1..=crate::rules::support::last_code_line(context) {
        let range = context.source.line_range(line_number);
        let line = &text[range.clone()];
        let content_end = line.trim_end_matches(['\r', '\n']).len();
        let trimmed_end = line[..content_end].trim_end_matches(blank).len();
        if trimmed_end == content_end {
            continue;
        }
        let heredoc = heredocs
            .iter()
            .find(|heredoc| heredoc.lines.contains(&line_number));
        if allow_in_heredoc && heredoc.is_some() {
            continue;
        }

        let start = range.start + trimmed_end;
        let end = range.start + content_end;
        let offense = context.offense("Trailing whitespace detected.", start..end);
        offenses.push(match heredoc {
            None => offense.corrected_by(Edit {
                start,
                end,
                replacement: String::new(),
                safe: true,
            }),
            Some(heredoc) => match heredoc_correction(heredoc, text, trimmed_end, start, end) {
                Some(edit) => offense.corrected_by(edit),
                None => offense,
            },
        });
    }
}

/// Trailing whitespace inside a heredoc is part of the string, so removing it would change the
/// program. RuboCop only deletes it when it is indentation the heredoc strips anyway; otherwise it
/// preserves the characters by wrapping them in an interpolation, and gives up entirely on a
/// non-interpolating heredoc, where that trick is unavailable.
fn heredoc_correction(
    heredoc: &Heredoc,
    text: &str,
    trimmed_end: usize,
    start: usize,
    end: usize,
) -> Option<Edit> {
    let whitespace_only = trimmed_end == 0;
    if whitespace_only && (end - start) <= heredoc.indent {
        return Some(Edit {
            start,
            end,
            replacement: String::new(),
            safe: true,
        });
    }
    if heredoc.static_literal {
        return None;
    }
    // The stripped indentation stays outside the interpolation, so an indented blank line keeps its
    // shape rather than gaining a visible `#{'...'}` in the margin.
    let start = if whitespace_only {
        start + heredoc.indent
    } else {
        start
    };
    Some(Edit {
        start,
        end,
        replacement: format!("#{{'{}'}}", &text[start..end]),
        safe: true,
    })
}

struct Heredoc {
    /// The body's own lines. The terminator is not one of them.
    lines: RangeInclusive<usize>,
    /// The smallest indentation any non-blank body line carries, which is what `<<~` strips.
    indent: usize,
    /// A `<<~'EOS'` heredoc, which cannot carry an interpolation.
    static_literal: bool,
}

fn heredocs(context: &RuleContext<'_>) -> Vec<Heredoc> {
    let bodies: Vec<_> = context.nodes_of("heredoc_body").collect();
    context
        .nodes_of("heredoc_beginning")
        .zip(bodies)
        .filter_map(|(beginning, body)| {
            let first = context.source.line_column(body.start_byte()).0;
            // The body node runs through the terminator, which is a line of its own.
            let terminator = body
                .named_children(&mut body.walk())
                .find(|child| child.kind_str() == "heredoc_end")
                .map(|child| context.source.line_column(child.start_byte()).0)
                .unwrap_or_else(|| context.source.line_column(body.end_byte()).0);
            if terminator <= first {
                return None;
            }
            let content = &context.source.text()[body.start_byte()..];
            let content_end = context.source.line_start(terminator) - body.start_byte();
            Some(Heredoc {
                lines: first..=terminator - 1,
                indent: indent_level(&content[..content_end]),
                static_literal: context.source.node_text(beginning).ends_with('\''),
            })
        })
        .collect()
}

/// The smallest leading-whitespace run over the body's non-blank lines, mirroring RuboCop's
/// `indent_level`. A line that holds nothing but whitespace does not constrain the indentation.
fn indent_level(body: &str) -> usize {
    body.split_inclusive('\n')
        .map(|line| {
            let rest = line.trim_start_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c']);
            &line[..line.len() - rest.len()]
        })
        .filter(|indent| !indent.ends_with('\n'))
        .map(str::len)
        .min()
        .unwrap_or(0)
}
