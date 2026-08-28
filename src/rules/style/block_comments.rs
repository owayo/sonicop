use crate::diagnostic::{Edit, Offense};
use crate::rules::RuleContext;

const MSG: &str = "Do not use block comments.";
/// `"=begin\n".length`, the head upstream removes whatever the first line actually holds.
const BEGIN_LENGTH: usize = 7;
/// `"\n=end".length`, which upstream measures from the *end* of the comment rather than from the
/// `=end` it names, so a trailing comment on that line shifts what gets removed.
const END_LENGTH: usize = 5;

pub(super) fn check(context: &RuleContext<'_>, offenses: &mut Vec<Offense>) {
    let text = context.source.text();
    for node in context.nodes_of("comment") {
        if !context.source.node_text(node).starts_with("=begin") {
            continue;
        }
        // Upstream's comment range runs to the end of the line `=end` sits on, newline included;
        // the grammar stops at the last character of that line.
        let start = node.start_byte();
        let mut end = node.end_byte();
        // **A file that does not end in a newline still has one to the parser**, and a block
        // comment's range runs one character past even that. Only the report reaches there; every
        // read below stays inside the text.
        let closed_by_newline = text.as_bytes().get(end) == Some(&b'\n');
        if closed_by_newline {
            end += 1;
        }
        let reported_end = match closed_by_newline {
            true => end,
            false => end + 2,
        };

        // `=begin\n=end` is the shortest block comment the parser accepts; anything shorter would
        // only reach here from a file that does not parse, where no cop runs at all.
        if end - start < BEGIN_LENGTH + 4 {
            continue;
        }
        let begin_fence = start..start + BEGIN_LENGTH;
        let end_fence = match text[start..end].ends_with('\n') {
            true => end - END_LENGTH..end,
            // `chomp` changes nothing, so upstream steps one character further back and stops two
            // short of the end.
            false => end - END_LENGTH - 1..end - 2,
        };
        // `=begin\n=end` puts the two fences the wrong way round, and the range upstream builds
        // between them makes its rewriter raise. The cop dies with nothing reported for the file,
        // so nothing is what this reports too.
        if end_fence.start < begin_fence.end {
            continue;
        }
        let contents = begin_fence.end..end_fence.start;

        let mut edits = vec![Edit {
            start: begin_fence.start,
            end: begin_fence.end,
            replacement: String::new(),
            safe: true,
        }];
        if !contents.is_empty() {
            edits.push(Edit {
                start: contents.start,
                end: contents.end,
                replacement: commented(&text[contents]),
                safe: true,
            });
        }
        edits.push(Edit {
            start: end_fence.start,
            end: end_fence.end,
            replacement: String::new(),
            safe: true,
        });
        offenses.push(context.offense(MSG, start..reported_end).corrected_by_all(edits));
    }
}

/// `source.gsub(/\A/, '# ').gsub("\n\n", "\n#\n").gsub(/\n(?=[^#])/, "\n# ")`.
///
/// The three substitutions run one after another over the whole string, and the second one's
/// output is what the third one reads: a run of blank lines is paired off from the left, and the
/// `#` the second put down then shields the newline before it from the third.
fn commented(source: &str) -> String {
    let headed = format!("# {source}");
    let blanked = headed.replace("\n\n", "\n#\n");
    let bytes = blanked.as_bytes();
    let mut out = String::with_capacity(blanked.len());
    for (index, character) in blanked.char_indices() {
        out.push(character);
        if character == '\n' && bytes.get(index + 1).is_some_and(|next| *next != b'#') {
            out.push_str("# ");
        }
    }
    out
}
