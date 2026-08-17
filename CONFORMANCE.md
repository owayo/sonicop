# RuboCop conformance

Snapshot date: per row — see the `Measured` column of *Results*
Reference: RuboCop 1.89.0 on Ruby 4.0.6
Configuration: RuboCop 1.89.0 built-in defaults (`--force-default-config`) on both sides

The bundled `config/default.yml` records the upstream version it was vendored from in its header.
Re-fetch it with `scripts/sync_default_yml.sh <rubocop-version>`.

## What is measured

Five Ruby projects, 18,251 target files between them, are linted by both tools and compared offense
by offense. An offense is keyed by cop name, path, line and column; at each shared key the last
line, last column, length, message, severity and correctability are compared as well.

| Corpus | Commit | Target files | Reference offenses |
|---|---|---:|---:|
| rubocop/rubocop | `f009b33` | 1,765 | 5,766 |
| rails/rails | `62b5458` | 3,551 | 167,760 |
| ruby/ruby | `3349f41` | 7,466 | 761,578 |
| Homebrew/brew | `38ee325` | 2,179 | 49,920 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,286 |

The commits are pinned because the numbers move with them: check the corpora out at these before
comparing, or a difference in the corpus will read as a difference in the port.

ruby/ruby's reference had to be assembled from chunked runs, and two files are excluded from it.
`test/ruby/test_regexp.rb` cannot be inspected at all — RuboCop's lexer raises `invalid byte
sequence in UTF-8` on it — and `test/ruby/test_file_exhaustive.rb` holds an offense whose text is
not valid UTF-8, which stops the JSON formatter from emitting a document. The comparison for that
corpus therefore covers 7,464 of its 7,466 files.

The **target file lists match exactly** on all five — not just the counts but the paths, compared as
sets: rubocop/rubocop (1,765), rails/rails (3,551), mastodon/mastodon (3,290), Homebrew/brew (2,179)
and ruby/ruby (7,466), with nothing on either side of any of the five. Counts alone would not settle
it — a set that loses one file and gains another keeps its count — so the check compares the sorted
lists and reports what only one side holds. That is
worth stating separately because file discovery is where a port silently diverges first: RuboCop
never reads `.gitignore`, applies a shebang test only to extensionless files, descends through
directory symlinks, and treats a hidden path by its *first* component rather than by any dot in it.

Sonicop implements **all 609 of RuboCop's cops**, matched name for name, with nothing extra on either
side. That includes the 159 RuboCop ships as `Enabled: pending` and the 56 it ships as
`Enabled: false`, neither of which a default run reaches — checking those means naming them with
`--only` or switching them on in a configuration, exactly as with RuboCop.

Because the two cop sets are identical, neither side is restricted with `--only`; the plain run is
already like for like. RuboCop's extension plugins are not installed, so this measures the range
that can be checked without them.

```bash
rubocop --force-default-config --cache false -f json
sonicop --force-default-config --format json
```

## Results

The two kinds of difference are counted separately because they mean different things. A
`Lint/Syntax` difference says the two disagree about whether a file parses, and everything else in
that file follows from it; a difference in any other cop says the port reads the same tree and
draws a different conclusion. Only the second is a defect in a cop.

| Corpus | Excess | Missing | of which `Lint/Syntax` | Other cops | Field differences | Measured |
|---|---:|---:|---|---:|---|---|
| rubocop/rubocop | 0 | 0 | — | 0 | correctable ×1 | 2026-08-17 |
| rails/rails | 0 | 0 | — | 0 | none | 2026-08-17 |
| mastodon/mastodon | 0 | 0 | — | 0 | none | 2026-08-17 |
| Homebrew/brew | 263 | 997 | **all of them** | **0** | none | 2026-08-17 |
| ruby/ruby | 142 | 585 | 117 missing, 44 excess | 92 | 5 | 2026-08-16 |

The last column is per row on purpose. A single date at the top of the file would say the five were
measured together, and they were not: the first four are re-measured on every release, ruby/ruby is
not — it is the one corpus whose reference cannot be produced in a single run (see above), so it is
re-measured deliberately rather than routinely. Its row is the older of the two and should be read
as such.

**No cop other than `Lint/Syntax` differs on four of the five corpora**, Homebrew included: across
its 2,179 files and 49,920 offenses the two agree on every position, message, severity and
correctability that is not a syntax diagnostic.

Homebrew's 1,260 differences — 997 missing and 263 excess — share one cause, and it is not a parser
bug on either side. Homebrew is a Ruby 4.0
codebase — `Library/Homebrew/.ruby-version` says `4.0.6` — but that file sits below the directory the
run starts from, and `TargetRubyVersion` is only inferred from the working directory's ancestors. Both
tools therefore fall back to the default of 2.7 (RuboCop says so itself: `Using Ruby 2.7 parser`) and
both call `dry_run:,` — a hash value omission, valid since 3.1 — a syntax error. **Run the same corpus
at 3.1 and both report zero `Lint/Syntax`.** What is left is not disagreement about which files parse:
the file sets are identical, 569 on each side, and the first diagnostic in each file lands at the same
position. Only the second and later diagnostics diverge, because the two parsers recover from the
error differently — which is what puts offenses on both sides of the ledger rather than only on
RuboCop's. Those are not chased; see *Known divergences*.

What is checked here is that every one of the 1,260 is a `Lint/Syntax` offense, in both directions.
What is not checked is which recovery produced which diagnostic: the 263 Sonicop reports and RuboCop
does not have not been read one by one. They are accounted for as a class, not individually.

Much of ruby/ruby's difference is the same shape,
with 117 of the 585 missing and 44 of the 142 excess being `Lint/Syntax` itself,
and the rest follows from it: a file the two disagree about is inspected by one tool and skipped by
the other, so every offense in it lands on one side of the ledger. Counting that through, 635 of the
727 differences sit in files one tool or the other calls a syntax error, and they are spread thinly
across the Layout department rather than sitting in one cop. Of the 92 that do not, 77 are the
`Style/RedundantParentheses` case under *Differences that should not be closed*, which leaves 15 —
ten of them in TRICK entries or encoding fixtures. Both are explained under *Known divergences*.

ruby/ruby also carries five differences at positions both tools reported: two `correctable` flags and
one `last_column`/`length` pair, all on indentation cops inside files the two parse differently, plus
one `Lint/Syntax` message naming a different token (`tLCURLY` where RuboCop says `tLAMBEG`).

Autocorrect is compared the same way, byte for byte over the whole tree: run `-A` with both tools
from a clean checkout and diff the results.

| Corpus | Corrected tree |
|---|---|
| rubocop/rubocop | **identical** |
| mastodon/mastodon | **identical** |
| Homebrew/brew | 2 files differ |
| rails/rails | 25 files differ |
| ruby/ruby | not measurable |

RuboCop's own tree and Mastodon are the hard line: a change that breaks byte equality on either is a
regression to be fixed, not a new known divergence to be recorded.

The rails/rails residue is one shape, and Homebrew contributes one more of it: the body of a
`begin`/`rescue`/`end` ends up indented two columns off. It appears only in a full `-A` run — the
first-pass detection matches exactly, and reducing the case to a single file reproduces nothing —
because what differs is which correction pass a nested indentation fix lands in. Homebrew's other
file pairs a `disable`/`enable` around `Lint/EmptyBlock`, a cop RuboCop ships as pending; the
upstream run leaves both comments alone and Sonicop removes them.

ruby/ruby cannot be measured this way at all: RuboCop's own run does not finish on it (see *Reading a
measurement*), so there is no complete reference tree to diff against.

Run the comparison on a copy of the corpus. Autocorrect rewrites the tree in place, so a run that
shares a checkout with anything else — another comparison, a lint measurement — has both tools
reading different files and reports differences that are not there.

## Reading a measurement

Two traps make a run look better or worse than it is, and both have produced wrong conclusions here.

**Counts cancel out.** Comparing offense totals hides an excess and a shortfall of the same size.
Compare the *set* of locations, then compare fields at the locations both tools produced.

**RuboCop can stop early.** On ruby/ruby its lexer raises `invalid byte sequence in UTF-8` on
`test/ruby/test_regexp.rb` and the whole run unwinds. A single-invocation run reports 6,909 of 7,466
files inspected, and the 557 files after that one in sort order were never looked at — so every
offense Sonicop found in them counts as "excess". Check `summary.inspected_file_count` against
`summary.target_file_count` on every run. To get a complete reference for that corpus, split the file
list into chunks and re-run each chunk past the file that killed it.

Read those two counts with `-f offenses`, not `-f json`: with the full cop set the JSON formatter
never gets to write them (see below), and the run looks like a crash rather than a truncation.

**And sometimes it emits nothing at all.** `test/ruby/test_file_exhaustive.rb` holds an offense whose
text is not valid UTF-8, and `JSONFormatter#finished` raises `source sequence is illegal/malformed
utf-8` while serializing — before a single byte reaches stdout. The failure is not per-file but
per-document: one such file discards the JSON for the entire run, summary included, which is why the
counts above have to come from another formatter.

Two things about this are easy to get backwards. **What matters is the encoding of the offense text,
not of the source.** `test/ruby/enc/test_euc_jp.rb` and `test/ruby/enc/test_shift_jis.rb` do hold
bytes that are not valid UTF-8, yet both serialize fine on their own — their magic comments make
RuboCop read them in their declared encoding — while `test_file_exhaustive.rb` is valid UTF-8 as a
file. **And which files are affected depends on the cop set, not on the corpus.** Running the full
609 excludes two files from ruby/ruby; running `--only Lint/LiteralAsCondition,Layout/IndentationWidth`
excludes one, because neither of those two cops produces an offense whose text is ill-formed. A
recorded exclusion count is part of a measurement's conditions, not a property of the corpus.

A chunked reference run has to tell this apart from the case above: retrying one file at a time is
right when a file killed the parser, and useless when the run never had a chance to start. Two other
stdout notices break JSON the same way — `--parallel` being ignored under `--cache false`, and the
plugin suggestions RuboCop prints after a run — so a reference reader should skip to the first `{`
rather than trust byte zero.

## Known divergences

`tests/conformance/known_divergences.yml` is the machine-checked record: a divergence that is listed
but no longer reproduces fails the test, and so does one that appears without being listed.

### Error recovery after a syntax error

RuboCop parses with `parser`, an LALR parser that recovers from an error and keeps going, emitting
further diagnostics from the recovered state. Sonicop parses with tree-sitter, which does not model
that recovery, so the follow-on diagnostics cannot be reproduced. On Homebrew this accounts for the
997 missing `Lint/Syntax` offenses: `class definition in method body`, `dynamic constant assignment`,
`cannot assign to a keyword`, and repeated `unexpected token` inside one multi-line hash.

What the divergence does not change is **which** files are held to be unparseable, and that is what
decides whether a file is inspected at all: RuboCop runs no cop other than `Lint/Syntax` on a file
that does not parse, and Sonicop does the same. On Homebrew the two agree on the *set*, not merely on
its size: 569 files, with the difference in either direction empty. This is why every cop other than
`Lint/Syntax` matches exactly there despite the 1,260 offenses of difference.

The agreement is not universal. On ruby/ruby the sets differ by six files: 56 agree, five are flagged
only by Sonicop and one only by RuboCop. The five are TRICK contest entries — deliberately obfuscated
programs, one of which is English prose that happens to parse as Ruby — where tree-sitter reports an
error and `parser` does not. Those five hold 465 of that corpus's 773 differences.

Within Homebrew's 569 files the agreement is partial, as recovery cannot be reproduced: 499 report the
same first diagnostic (position and message), and 222 report an identical list end to end. The 277
files in between are the divergence in its purest form — the two agree on where the file first goes
wrong and part company on what follows. The 70
files whose first diagnostic differs follow one shape — an endless method definition (`def to_s =
to_str`, valid from Ruby 3.0, rejected by the default `TargetRubyVersion: 2.7`) leaves `parser`'s
method context open, so the enclosing `class` emits `class definition in method body` when it is
finally reduced. The diagnostic is reported at the `class` keyword, far above the line that actually
failed, which puts it first in source order.

### What is left on ruby/ruby

ruby/ruby is the only corpus with a residue that is not one shape, so it is worth taking apart. Of
its 727 differences:

| | Count | |
|---|---:|---|
| in files one tool calls a syntax error | 635 | the recovery difference above |
| `Style/RedundantParentheses` in one file | 77 | RuboCop's defect, see below |
| everything else | 15 | |

The last 15 are the honest remainder. Ten of them sit in TRICK entries or encoding fixtures —
`Layout/SpaceInsideParens` five times in one obfuscated program, `Layout/TrailingWhitespace` and
`Layout/LeadingCommentSpace` on a UTF-16 fixture that holds no BOM. Three are the `"…"%[…]` lexing
gap described under *Grammar gaps*. That leaves two — `Lint/NestedMethodDefinition` on
`def (obj.bar = Object.new).baz` and `Style/MixinUsage` on a top-level `include RbConfig` — which
have not been investigated.

Naming that number is the point. It was 138 before the cop fixes that landed with the 609th cop, and
every step down came from taking one shape at a time rather than from a general improvement.

### Grammar gaps

Where tree-sitter rejects code that Ruby accepts, Sonicop reports a syntax error and — following the
rule above — reports nothing else for that file. One such gap costs every offense in the file at
once, which makes them worth hunting: fixing eight lexer rules in the grammar fork took ruby/ruby
from 24 affected files to 5, and cut the offenses Sonicop was missing there by more than sevenfold.
What is left moves with how many cops are implemented — one unparseable file costs every offense
every cop would have reported in it — so the shortfall grows as coverage does.

What remains are constructs whose ambiguity Ruby resolves with information a grammar does not have.
`$a?0:1` is the clearest: whether `?0` is a character literal or the start of a ternary depends on
whether the token before it completed an expression, which in turn depends on knowing that `a` is a
local variable rather than a method call.

A gap does not have to reject the file to cost offenses. `"%3d %s"%[l+1, line]` parses, but not as
Ruby reads it: a `%` written against a string literal can only be the operator, while tree-sitter
takes `%[…]` for a percent-literal string and yields two adjacent strings instead of a binary
operation. Nothing downstream can recover it — the operator node the cop would report on is not in
the tree — so `Layout/SpaceAroundOperators` misses both the `%` and the `+` nested inside the
brackets. Three offenses on ruby/ruby come from this, all on one line of `libexec/erb`.

### Encoding — the one deliberate difference

RuboCop's autocorrect ends in a plain `File.write`, so a corrected Shift_JIS file is written back as
UTF-8 while its magic comment still claims Shift_JIS. The result no longer loads: read it back as
cp932 and you get mojibake, not the source you had.

**Sonicop writes the correction back in the encoding the file declares**, and refuses to write at all
when the correction holds a character that encoding cannot represent, leaving the file untouched
rather than substituting something else. Drop-in compatibility reaches a long way, but not as far as
reproducing data loss on purpose. This is the only place Sonicop knowingly differs, and it only
shows up on files that declare a non-UTF-8 encoding — 8 of the 18,251 files measured here.

A source declaring `ASCII-8BIT` or `binary` needs no such treatment: Ruby measures it one byte at a
time, so Sonicop reads it that way too and the bytes go back out unchanged.

## Differences that should not be closed

Everything above records a place Sonicop has not reached. This section records the opposite: a
difference where RuboCop is the one that is wrong, and matching it would mean discarding a correct
report. The two are worth separating, because a table that mixes them reads as though every
difference counts against the port.

On ruby/ruby's `test/-ext-/bignum/test_pack.rb`, `Style/RedundantParentheses` finds 3 offenses in
RuboCop and 79 in Sonicop. The 76 are real: they are `(-n)`, `(-n+1)` and `(+n-1)` written as method
arguments, the same shape RuboCop reports elsewhere in the same file and in a file of its own.

RuboCop drops them because of an unrelated line further down. Truncating the file shows it exactly:

```
first 82 lines + a closing end   → RuboCop reports 8
first 83 lines + a closing end   → RuboCop reports 0    ← the 8 already reported are withdrawn
line 83 with \xFF changed to \x00 → RuboCop reports 9
```

Line 83 is `assert_equal([-1, "\xFF"], Bug::Bignum.test_pack((-0x0FF), 1, 1, 0, BIG_ENDIAN))`, in a
file whose first line is `# coding: ASCII-8BIT`. Reading that far makes RuboCop abandon
`Style/RedundantParentheses` for the whole file, silently:

- it is specific to that cop — `Style/StringLiterals` still reports its 10 offenses in the same file
- it is not the cop-error path — `--raise-cop-error` says nothing
- the file is inspected — the run reports `inspected: 1` and simply finds no offense of that cop
- it is not a parse failure — neither tool emits `Lint/Syntax`, and `ruby -c` accepts the file
- a synthesised minimum does not reproduce it; the surrounding file is part of the trigger

Sonicop reports all 79. Bringing that down to 3 would mean suppressing correct output to match a
defect, so the difference stands as it is.

## Limits

A clean run is a property of the corpora, not a general claim. `known_divergences.yml` carries the
current list of what these corpora never exercise; the main ones are non-default configuration
values, extension plugins, and Windows line endings. Cops that never fire contribute nothing to a
match count, and their silence is indistinguishable from agreement.

That limit is not hypothetical, and one instance is now measured. `Style/HashSyntax` defaults
`EnforcedShorthandSyntax` to `either`, under which neither tool reports anything; the port
implements none of the other four values, so half the cop is missing. Both the corpus runs and a
sweep of RuboCop's own specs report agreement for it, because both run at the default. A cop can be
half absent and still match everywhere the default reaches. Where a cop's behaviour is selected by
configuration, this document covers the default branch only — the others are neither measured nor
claimed.

The 215 cops RuboCop ships switched off are a limit of a different kind. They are implemented, but a
default run never reaches them, so the corpus numbers above say nothing about them. What stands
behind those is a separate measurement: the same corpora linted with all 609 cops switched on, on
both sides, and the residue classified. That is weaker evidence than it sounds — a cop that never
fires on these corpora contributes nothing either way — and the residue is not yet zero. The figures
for that run are tracked in `known_divergences.yml` rather than here, because they are still moving.

Closing that gap properly means porting RuboCop's own spec suite, which exercises each cop against
inputs written to break it. Until that lands, this document records what was measured.

Timings live in the README's Performance section.
