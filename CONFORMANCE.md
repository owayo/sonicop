# RuboCop conformance

Snapshot date: 2026-08-12
Reference: RuboCop 1.89.0 on Ruby 4.0.6
Configuration: RuboCop 1.89.0 built-in defaults (`--force-default-config`) on both sides

The bundled `config/default.yml` records the upstream version it was vendored from in its header.
Re-fetch it with `scripts/sync_default_yml.sh <rubocop-version>`.

## What is measured

Five Ruby projects, 18,244 target files between them, are linted by both tools and compared offense
by offense. An offense is keyed by cop name, path, line and column; at each shared key the last
line, last column, length, message, severity and correctability are compared as well.

| Corpus | Commit | Target files | Reference offenses |
|---|---|---:|---:|
| rubocop/rubocop | `e82df38` | 1,765 | 4,142 |
| rails/rails | `729d2e9` | 3,550 | 117,541 |
| ruby/ruby | `52975b7` | 7,465 | 603,331 |
| Homebrew/brew | `5d49126` | 2,175 | 38,742 |
| mastodon/mastodon | `e5db3aa` | 3,289 | 7,610 |

The **target file lists match exactly** on all five — not just the counts but the paths. That is
worth stating separately because file discovery is where a port silently diverges first: RuboCop
never reads `.gitignore`, applies a shebang test only to extensionless files, descends through
directory symlinks, and treats a hidden path by its *first* component rather than by any dot in it.

Only the cops Sonicop implements are compared; the reference process is restricted to them with
`--only`. RuboCop's extension plugins are not installed, so this measures the range that can be
checked without them.

```bash
cops=$(sonicop --show-cops --force-default-config \
       | grep -B3 'Implemented: true' | grep '^[A-Z].*:$' | tr -d ':' | paste -sd, -)

rubocop --force-default-config --cache false --only "$cops" -f json
sonicop --force-default-config --format json
```

## Results

| Corpus | Excess | Missing | Field differences |
|---|---:|---:|---|
| rubocop/rubocop | 0 | 0 | none |
| rails/rails | 0 | 0 | none |
| mastodon/mastodon | 0 | 0 | none |
| Homebrew/brew | 253 | 996 | none |
| ruby/ruby | 89 | 346 | 1 |

Homebrew's remaining difference is entirely `Lint/Syntax`; every other cop matches exactly. Most of
ruby/ruby's is too — 117 of the 346 missing and 70 of the 89 excess are `Lint/Syntax` itself. The
rest follows from it: a file the two disagree about is inspected by one tool and skipped by the
other, so every offense in it lands on one side of the ledger. Both are explained under
*Known divergences*.

Autocorrect is compared the same way, byte for byte over the whole tree: run `-a` (and separately
`-A`) with both tools from a clean checkout and diff the results. On all four corpora measured this way —
rubocop/rubocop, rails/rails, Homebrew/brew and mastodon/mastodon — the corrected trees are
**identical**.

Run the comparison on a copy of the corpus. Autocorrect rewrites the tree in place, so a run that
shares a checkout with anything else — another comparison, a lint measurement — has both tools
reading different files and reports differences that are not there.

## Reading a measurement

Two traps make a run look better or worse than it is, and both have produced wrong conclusions here.

**Counts cancel out.** Comparing offense totals hides an excess and a shortfall of the same size.
Compare the *set* of locations, then compare fields at the locations both tools produced.

**RuboCop can stop early and still emit valid JSON.** On ruby/ruby its parser raises
`invalid byte sequence in UTF-8` on `test/ruby/test_regexp.rb` and the whole run unwinds; the JSON
formatter still writes a well-formed document from an `ensure` block. The result reports 6,924 of
7,465 files inspected, and the 541 files after that one in sort order were never looked at — so
every offense Sonicop found in them counts as "excess". Check `summary.inspected_file_count`
against `summary.target_file_count` on every run. To get a complete reference for that corpus, split
the file list into chunks and re-run each chunk past the file that killed it.

## Known divergences

`tests/conformance/known_divergences.yml` is the machine-checked record: a divergence that is listed
but no longer reproduces fails the test, and so does one that appears without being listed.

### Error recovery after a syntax error

RuboCop parses with `parser`, an LALR parser that recovers from an error and keeps going, emitting
further diagnostics from the recovered state. Sonicop parses with tree-sitter, which does not model
that recovery, so the follow-on diagnostics cannot be reproduced. On Homebrew this accounts for the
996 missing `Lint/Syntax` offenses: `class definition in method body`, `dynamic constant assignment`,
`cannot assign to a keyword`, and repeated `unexpected token` inside one multi-line hash.

What the divergence does not change is **which** files are held to be unparseable, and that is what
decides whether a file is inspected at all: RuboCop runs no cop other than `Lint/Syntax` on a file
that does not parse, and Sonicop does the same. On Homebrew both tools flag the same 568 files —
neither has one the other lacks. This is why every cop other than `Lint/Syntax` matches exactly there
despite the 1,249 offenses of difference.

Within those 568 files the agreement is partial, as recovery cannot be reproduced: 499 report the
same first diagnostic (position and message), and 223 report an identical list end to end. The 69
files whose first diagnostic differs follow one shape — an endless method definition (`def to_s =
to_str`, valid from Ruby 3.0, rejected by the default `TargetRubyVersion: 2.7`) leaves `parser`'s
method context open, so the enclosing `class` emits `class definition in method body` when it is
finally reduced. The diagnostic is reported at the `class` keyword, far above the line that actually
failed, which puts it first in source order.

### Grammar gaps

Where tree-sitter rejects code that Ruby accepts, Sonicop reports a syntax error and — following the
rule above — reports nothing else for that file. One such gap costs every offense in the file at
once, which makes them worth hunting: fixing eight lexer rules in the grammar fork took ruby/ruby
from 24 affected files to 5, and the offenses Sonicop was missing there from 2,457 to 346.

What remains are constructs whose ambiguity Ruby resolves with information a grammar does not have.
`$a?0:1` is the clearest: whether `?0` is a character literal or the start of a ternary depends on
whether the token before it completed an expression, which in turn depends on knowing that `a` is a
local variable rather than a method call.

### Encoding — the one deliberate difference

RuboCop's autocorrect ends in a plain `File.write`, so a corrected Shift_JIS file is written back as
UTF-8 while its magic comment still claims Shift_JIS. The result no longer loads: read it back as
cp932 and you get mojibake, not the source you had.

**Sonicop writes the correction back in the encoding the file declares**, and refuses to write at all
when the correction holds a character that encoding cannot represent, leaving the file untouched
rather than substituting something else. Drop-in compatibility reaches a long way, but not as far as
reproducing data loss on purpose. This is the only place Sonicop knowingly differs, and it only
shows up on files that declare a non-UTF-8 encoding — 8 of the 18,244 files measured here.

A source declaring `ASCII-8BIT` or `binary` needs no such treatment: Ruby measures it one byte at a
time, so Sonicop reads it that way too and the bytes go back out unchanged.

## Limits

A clean run is a property of the corpora, not a general claim. `known_divergences.yml` carries the
current list of what these corpora never exercise; the main ones are non-default configuration
values, extension plugins, and Windows line endings. Cops that never fire contribute nothing to a
match count, and their silence is indistinguishable from agreement.

Closing that gap properly means porting RuboCop's own spec suite, which exercises each cop against
inputs written to break it. Until that lands, this document records what was measured.

Timings live in the README's Performance section.
