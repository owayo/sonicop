# RuboCop conformance

Snapshot date: 2026-08-11
Reference: RuboCop 1.89.0 on Ruby 4.0.6
Configuration: RuboCop 1.89.0 built-in defaults (`--force-default-config`) on both sides

The bundled `config/default.yml` records the upstream version it was vendored from in its header.
Re-fetch it with `scripts/sync_default_yml.sh <rubocop-version>`.

## What is measured

Five Ruby projects, 18,242 target files between them, are linted by both tools and compared offense
by offense. An offense is keyed by cop name, path, line and column; at each shared key the last
line, last column, length, message, severity and correctability are compared as well.

| Corpus | Commit | Target files | Reference offenses |
|---|---|---:|---:|
| rubocop/rubocop | `e82df38` | 1,763 | 4,063 |
| rails/rails | `729d2e9` | 3,550 | 117,541 |
| ruby/ruby | `52975b7` | 7,465 | 603,341 |
| Homebrew/brew | `5d49126` | 2,175 | 38,765 |
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
| ruby/ruby | 262 | 2,457 | 146 |

Homebrew's remaining difference is entirely `Lint/Syntax`; every other cop matches exactly. On
ruby/ruby, 2,309 of the 2,457 missing offenses sit in 24 files that only Sonicop treats as
unparseable. Both are explained under *Known divergences*.

Autocorrect is compared the same way, byte for byte over the whole tree: run `-a` (and separately
`-A`) with both tools from a clean checkout and diff the results. On rubocop/rubocop the corrected
trees are **identical**.

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

The first diagnostic in each file does match, which is what decides whether the file is inspected at
all: RuboCop runs no cop other than `Lint/Syntax` on a file that does not parse, and Sonicop does the
same.

### Grammar gaps

Where tree-sitter rejects code that Ruby accepts, Sonicop reports a syntax error and — following the
rule above — reports nothing else for that file. On ruby/ruby, 24 files are affected, and they carry
2,309 of the 2,457 offenses Sonicop misses there. The remaining 148 are spread thinly across cops.
These are grammar defects rather than cop defects, and are tracked in the tree-sitter-ruby fork.

### Encoding

RuboCop's autocorrect ends in a plain `File.write`, so a corrected Shift_JIS file is written back as
UTF-8 while its magic comment still claims Shift_JIS. Sonicop reproduces this rather than fixing it,
because writing different bytes than RuboCop for the same input is the one thing a drop-in cannot do.

A source declaring `ASCII-8BIT` or `binary` is the exception: Ruby measures it one byte at a time, so
Sonicop reads it that way too and writes the same bytes back.

## Limits

A clean run is a property of the corpora, not a general claim. `known_divergences.yml` carries the
current list of what these corpora never exercise; the main ones are non-default configuration
values, extension plugins, and Windows line endings. Cops that never fire contribute nothing to a
match count, and their silence is indistinguishable from agreement.

Closing that gap properly means porting RuboCop's own spec suite, which exercises each cop against
inputs written to break it. Until that lands, this document records what was measured.

Timings live in the README's Performance section.
