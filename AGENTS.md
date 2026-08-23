# AGENTS.md

Working notes for AI agents and new contributors. `CLAUDE.md` is a symlink to this file.

## What this project is

Sonicop is a native RuboCop-compatible Ruby linter and formatter. **RuboCop 1.89.0 is the
specification.** Where the two disagree, Sonicop is wrong unless the difference is recorded in
`tests/conformance/known_divergences.yml` with a reason.

That single rule decides most questions. Before changing behaviour, run the real thing and compare:

```bash
rubocop --only <Cop/Name> --format json path.rb
sonicop --only <Cop/Name> --format json path.rb
```

Never write an expectation from Sonicop's current output — that bakes today's bugs in as the spec.

## Layout

| Path | What lives there |
|---|---|
| `src/engine.rs` | Inspection pipeline, the correction tree, and the result cache |
| `src/cli.rs` | Argument parsing, run modes, `--auto-gen-config` |
| `src/config/` | `.rubocop.yml` loading, `inherit_from` / `inherit_mode`, Include/Exclude matching |
| `src/rules/` | One file per cop, grouped by department; registered in `src/rules/mod.rs` |
| `src/formatter.rs` | Every output format |
| `src/directives.rs` | `# rubocop:disable` / `enable` handling |
| `config/default.yml` | Vendored from upstream; refresh with `scripts/sync_default_yml.sh <version>` |
| `tests/cops.rs` | Per-cop regression tests using caret annotations |
| `tests/conformance.rs` | Hand-written cases plus the known-divergence manifest |
| `tests/spec_fixtures.rs` | Every case RuboCop's own specs supply, checked against what upstream really reports |
| `tests/cli.rs` | End-to-end tests that drive the binary |
| `lib/`, `exe/`, `script/` | The Ruby gem wrapper that ships the binary |

## Commands

The Makefile is the single entry point; `Rakefile` holds only gem packaging and version syncing.

```bash
make build            # cargo build
make test             # Rust + Ruby wrapper tests
make check            # version check, fmt, clippy -D warnings, all tests  <- the gate CI runs
make install          # build --release and install to /usr/local/bin
```

`make check` is what CI runs. Run it before saying anything is done.

## Conventions

**Comment language.** Production code under `src/` is documented in English; tests under `tests/`,
the build tooling (`Makefile`, `Rakefile`, CI workflows) and `known_divergences.yml` are in
Japanese. In-file `#[cfg(test)]` blocks follow whichever the surrounding file already uses. Match
the file you are editing rather than converting it.

**Comments say why, not what.** The existing comments name the upstream RuboCop method being
mirrored, or record the measurement behind a decision. That is the house style — a comment
restating the code is worse than none.

**One cop per file.** Add a cop under its department directory and register it in
`src/rules/mod.rs`. `every_cop_is_registered_once` and
`every_registered_cop_exists_in_the_default_configuration` will catch a mistake.

## Invariants worth knowing before you edit

**Byte offsets are not character offsets.** tree-sitter reports byte ranges; slicing a `&str` at a
non-boundary panics, and a linter must never abort on a valid file. Offsets a cop derived by
arithmetic are pulled back to a boundary rather than sliced — see `SourceFile::line_column` and
`diagnostic::character_length`. Lengths reported to the user are counted in characters, because
that is the unit RuboCop reports.

**Display width is generated, not written.** `src/display_width_table.rs` is produced from the
`unicode-display_width` gem by `scripts/dump_display_width.rb`; do not hand-edit it. A hand-written
table stood there before and drifted — it counted the combining marks U+3099/U+309A as two columns,
so decomposed Japanese drew the wrong number of carets. Regenerate rather than patch:

```bash
ruby scripts/dump_display_width.rb > src/display_width_table.rs
```

`tests/fixtures/regexp_trees.jsonl` follows the same pattern; its provenance is in the neighbouring
`.PROVENANCE` file.

**The result cache must never serve a stale verdict.** Everything a cop's answer depends on has to
be part of the cache identity or the per-file stat: the build fingerprint, the configuration digest,
the cop selection, and the file's size, modification time *and* permission bits — `Lint/ScriptPermission`
reads the mode, which `chmod` changes without touching the bytes. The stat is taken **before** the
file is read; taken afterwards, a rewrite landing in between pairs the old report with the new stat
and the next run accepts it as fresh. Bump `RESULT_CACHE_SCHEMA` when the stored shape changes.

**Autocorrect writes to real source files.** Corrections go through a temp file so a killed writer
cannot leave a truncated file, and permissions are preserved. Anything touching that path needs a
test that inspects the file on disk afterwards, not just the reported offenses.

## Adding a cop test

`tests/cops.rs` uses upstream's caret notation: the annotation points at the preceding source line,
leading spaces give the column and the run of `^` gives the length.

```rust
expect_offense("Style/RedundantReturn", r#"
    def foo
      return 1
      ^^^^^^ Redundant `return` detected.
    end
"#);
expect_no_offenses("Style/RedundantReturn", "def foo\n  1\nend\n");
expect_correction("Style/RedundantReturn", before, after);
```

Cases that match upstream belong in `tests/cops.rs`. Cases that do not belong in
`tests/conformance.rs` together with an entry in the divergence manifest explaining why.

## The cases nobody wrote by hand

Hand-written tests only cover what somebody thought to write down. Counted on 2026-08-23, the
1,918 of them reach 606 cops but check an actual **offense** for only 427 — and `make cop-coverage`
recounts it, because reading the test source does not answer the question (cop names get passed
through `const`s).

`tests/spec_fixtures.rs` closes that gap from the other side: every case RuboCop's own specs
supply becomes a case here. **The expectation is not the spec text — it is what upstream really
reports**, recorded once by `make spec-fixtures`, because upstream does not always behave the way
its specs say. The recording is committed, so a normal test run needs no rubocop gem.

As of 2026-08-23 all **11,300** recorded cases match, across the **555** cops the specs reach, with
no entry in `spec_known_divergences.yml`. A difference appearing there is a regression, not a
backlog item.

Two things about this gate are easy to get backwards.

**Its value is in the cases where upstream stays silent.** Corpora can only show what upstream
reports, so **over-detection is invisible to them** — the shapes upstream is quiet about are
infinite and appear in real code only by accident. Roughly 44% of the recorded cases are
`no_offenses` ones, and that half is the point of the file.

**Reproducing the recording means reproducing its conditions.** Four ways of getting that wrong
have already turned the harness into a difference generator, and each looked exactly like a bug in
a cop:

| Mistake | What it produced |
|---|---|
| Running `.rb` where upstream ran `.gemspec` / `.gemfile` | Every Gemspec and Bundler cop silently matched nothing |
| Passing the printed `length` as the caret count | Every offense spanning lines became a `range` difference — 57 in three Metrics cops alone |
| Feeding a recorded output through `CopCase::corrected` | It dedents, for the hand-written `<<~RUBY` cases; recorded output loses its leading newline and indentation. **20 differences, all of them mine** — use `corrected_verbatim` |
| Treating `foo&.bar` as a `send` | The grammar spells `send` and `csend` alike; upstream's `:send` arm excludes `csend`, and reading it as one makes a cop claim its own receiver is non-nil |

The first two accounted for 54 of the first 65 failures. Before believing a difference, reproduce
it from the command line against the real upstream — the recording says what upstream did, not
what the harness asked it.

**Print the tree before guessing at it.** Most of what is left in that gate is a place where the
grammar and upstream's AST disagree about shape, and reasoning about which node holds what is
slower and less reliable than looking:

```bash
cargo run --release --example dump_ast -- 'do_something(**{foo: bar, **{baz: qux}})'
cargo run --release --example dump_ast -- --file path/to/source.rb
```

Two differences that had each survived a round of guessing fell in minutes once the tree was on
screen: a nested `**{…}` is a `hash_splat_argument` **among the pairs** of the hash above it (so
`node.pairs` is not `children`), and `not bar ? a : b` parses as `unary(not, conditional(…))`
rather than as a conditional over a negation.

**Print the corrections too, when a correction does not land.** A cop can build a perfectly good
set of edits and still leave the file untouched: the edit applier drops what falls outside the
offense's anchor, and the syntax guard throws away a pass whose output does not parse. Neither
says which edit was at fault.

```bash
cargo run --release --example dump_corrections -- --config .rubocop.yml --only Style/Foo file.rb
```

`Style/Next` was reported as "correctable" and never corrected, because its edits reach the `then`
and the `end` beyond the range it reports and nothing anchored them there. `Style/MutableConstant`
under `Recursive: true` was withheld as unparsable, because a hash key is a `hash_key_symbol` here
and was missing from the immutable list, so the second pass tried to append `.freeze` to it.

**The same source can have two right answers.** `TargetRubyVersion` changes the tree upstream
builds, not just which constructs it accepts: `a, b = 1, 2 rescue nil` puts the `rescue` around the
whole assignment before 2.7 and around the right-hand side after it, and the correction differs
accordingly. When a recording disagrees with the upstream you run by hand, check the recorded
`target_ruby` before concluding either is wrong.

**A construct the grammar cannot read is not a syntax error.** `Lint/Syntax` reports what `parser`
reports; an `ERROR` node over Ruby the real thing accepts is a false positive that also stops every
other cop from running on the file. Two are known and skipped by name in `src/rules/lint/syntax.rs`
-- a multi-line array pattern (`in bar,\n baz`) and a heredoc opened inside an interpolation. Both
also mislead the cops that then do run: silencing the first one turned
`Style/MultilineInPatternThen` into a false positive, because the pattern the grammar closed early
looked single-line.

## Conformance measurement

`CONFORMANCE.md` records offense-by-offense comparisons against five pinned corpora (18,251 files).
The commits are pinned because the numbers move with them. Anything that changes file discovery or
path matching can move the target-file lists, so re-measure after touching `src/config/paths.rs`.
That run is long — hand it to a human rather than starting it inside an agent session.

## Versioning

`Cargo.toml` and `lib/sonicop/version.rb` must agree; `make version-check` enforces it and CI fails
otherwise. Use `rake version:set VERSION=x.y.z` rather than editing either by hand.
