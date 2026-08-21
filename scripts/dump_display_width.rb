# frozen_string_literal: true

# Regenerates `src/display_width_table.rs` from `Unicode::DisplayWidth`, the gem RuboCop measures
# display columns with. The table is the generated artefact; this script is how it is rebuilt when
# the gem or its Unicode version moves.
#
#   ruby scripts/dump_display_width.rb > src/display_width_table.rs
#
# The gem is the reference rather than the Unicode data files because reproducing its answer is the
# whole point: `alignment.rb` and `clang_style_formatter.rb` both call `Unicode::DisplayWidth.of`,
# so a table derived from `EastAsianWidth.txt` would still have to re-derive every exception the gem
# layers on top (Hangul fillers, `Default_Ignorable_Code_Point`, the two-em dash, backspace).
#
# Only widths other than 1 are emitted; a code point the table does not cover is one column wide.
# That keeps a table spanning the whole code space to a few hundred rows.
#
# `Unicode::DisplayWidth.of` is called with no options, which is how RuboCop calls it.

require 'unicode/display_width'

# Widths are taken as a delta against a pad rather than from the character alone, because `of`
# clamps its result at zero: `of(BACKSPACE)` is 0 while the character is worth -1, which is
# observable as soon as anything precedes it (`of("a" + BACKSPACE)` is 0, not 1). U+0008 is the
# only code point where the two readings disagree, and the delta is the one that composes.
PAD = 'aaaa'
PAD_WIDTH = Unicode::DisplayWidth.of(PAD)

# Surrogates are not scalar values, so no `char` can hold one and no row is needed for them.
SURROGATES = (0xD800..0xDFFF).freeze

def width_of(code_point)
  Unicode::DisplayWidth.of(PAD + code_point.chr(Encoding::UTF_8)) - PAD_WIDTH
end

# Consecutive code points of equal width collapse into one row. A surrogate breaks a run rather
# than extending it, so no row can span the gap and claim a width for something unrepresentable.
rows = []
(0..0x10FFFF).each do |code_point|
  if SURROGATES.cover?(code_point)
    rows.last&.tap { |row| row[:closed] = true }
    next
  end

  width = width_of(code_point)
  next if width == 1

  last = rows.last
  if last && !last[:closed] && last[:last] == code_point - 1 && last[:width] == width
    last[:last] = code_point
  else
    rows << { first: code_point, last: code_point, width: width, closed: false }
  end
end

puts <<~HEADER
  //! Character widths as `Unicode::DisplayWidth` reports them. **Generated; do not edit.**
  //!
  //! Regenerate with `ruby scripts/dump_display_width.rb > src/display_width_table.rs`, which
  //! documents why the gem rather than the Unicode data files is the source.
  //!
  //! Generated from unicode-display_width #{Unicode::DisplayWidth::VERSION} (Unicode #{Unicode::DisplayWidth::UNICODE_VERSION}).
  //!
  //! RuboCop's gemspec allows `>= 2.4.0, < 4.0`, so a user's own RuboCop may measure with a
  //! different release of the gem and disagree about a handful of code points. Pinning to one
  //! release is as close as a table can get; the version above is what the table reproduces.

  /// `(first, last, width)` for every inclusive range whose width is not 1, sorted by `first` and
  /// non-overlapping. A code point matching no range is one column wide.
  pub(crate) static WIDTHS: &[(u32, u32, i8)] = &[
HEADER

rows.each do |row|
  puts format('    (0x%04X, 0x%04X, %d),', row[:first], row[:last], row[:width])
end

puts '];'
