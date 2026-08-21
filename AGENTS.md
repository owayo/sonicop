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

## Conformance measurement

`CONFORMANCE.md` records offense-by-offense comparisons against five pinned corpora (18,251 files).
The commits are pinned because the numbers move with them. Anything that changes file discovery or
path matching can move the target-file lists, so re-measure after touching `src/config/paths.rs`.
That run is long — hand it to a human rather than starting it inside an agent session.

## Versioning

`Cargo.toml` and `lib/sonicop/version.rb` must agree; `make version-check` enforces it and CI fails
otherwise. Use `rake version:set VERSION=x.y.z` rather than editing either by hand.
