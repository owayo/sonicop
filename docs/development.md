# Development

Notes for working on sonicop beyond the [Development](../README.md#development) section of the README. `make` is the single entry point and lists every target; tool versions (Rust and Ruby) are pinned in `mise.toml`.

## Make and the Rakefile

The Rakefile holds only the gem packaging and version tasks, and `make` calls them: `make gem` builds the source gem, `make gem-platform GEM_PLATFORM=<platform>` builds a platform gem around the release binary, and `make version-sync` regenerates `lib/sonicop/version.rb`. `make ci` ends with `make gem-check`, which builds the source gem, installs it into `tmp/gems` and runs the installed command, so it includes a release build.

`make cop-coverage` counts the cops the hand-written tests reach. `make spec-capture` and `make spec-fixtures` re-record upstream's spec cases and their expectations; they need an upstream checkout and tools outside this repository, described in the comments of the Makefile.

## Corpus gates and benchmarks

`make corpus-check CORPUS=<name>` compares safe and unsafe correction on independent copies, checks the divergence manifest's paths and output hashes, checks Ruby syntax, and verifies a second `-A` leaves the tree unchanged. Set `GATE_CORPUS_ROOT`, `GATE_ORACLE_ROOT`, `OUT_DIR` and `SONICOP` to explicit paths. Use `MISE_ENV=conformance` for the optional Python toolchain in `mise.conformance.toml`. The **Corpus conformance** workflow prepares the pinned public inputs and RuboCop 1.89.0 automatically. Ruby's full corpus is not included in that correction matrix because the reference aborts; its lint comparison needs the chunked reference described in [CONFORMANCE.md](../CONFORMANCE.md).

`syntaxcheck.py <corpus> --candidate-only` checks a complete Sonicop correction report against valid original `.rb` files without claiming reference equality. The workflow runs this separate safety check on the full pinned Ruby corpus, where RuboCop cannot complete. Normal two-tool syntax classification requires complete, nonempty reports and equal path sets; an interrupted reference tree is never classified as a fully corrected one.

`make benchmark ARGS="--output tmp/benchmark --repeats 2"` measures the five pinned public corpora against RuboCop 1.92.0. Inputs live outside the project so a parent gemspec cannot change their target Ruby version; CLI probes verify both tools resolve the pinned corpus's expected version. The **Benchmark** workflow runs it on a separate runner and publishes the conditions, raw JSON, exit codes and timing results as artifacts. Every timed run must inspect the complete target set and reproduce its own baseline's full offense records. The newer gem is a timing reference; behavioral compatibility remains pinned to 1.89.0. Timing output includes offense counts because the two versions do different work.

## Adding a cop

A cop is one file under `src/rules/<department>/<cop>.rs` exposing a single `check(context, offenses)`, plus one line in that department's `mod.rs`:

```rust
department_rules! {
    "Layout";
    line_length => ("LineLength", Convention),
}
```

That line is the only place the cop's name and default severity are written. Inside the cop the name stays implicit: `context.setting("Max")` reads `Layout/LineLength: Max`, and `context.offense(message, range)` reports under the cop's own name at its configured severity. A cop that spelled its name a second time could disagree with the registry, and nothing in the type system would catch it.

Prefer `context.nodes_of("kind")` over walking every node: each cop runs on every file, so a full walk per cop is what makes inspection scale with the registry rather than with the file.

## Versions

`Cargo.toml` is the single source of truth for the version. `lib/sonicop/version.rb` is generated from it by `make version-sync` and committed, because the gemspec reads it at package time. CI fails when the two disagree.

## Files generated from upstream

`config/default.yml` is vendored from upstream RuboCop; re-fetch it with `scripts/sync_default_yml.sh <rubocop-version>`, which records the source version in the file header.

`src/display_width_table.rs` is generated and committed too. RuboCop measures display columns with the `unicode-display_width` gem, so the table is taken from the gem rather than restated by hand — an exception table written out by hand had already drifted far enough to draw the wrong number of carets under decomposed Japanese. Regenerate it with `ruby scripts/dump_display_width.rb > src/display_width_table.rs`, which records the gem and Unicode versions in the file header.

## Dependencies

Dependencies are updated with `depup --install --include-pinned`. Parser and project-index dependencies are pinned to exact versions or fork commits in `Cargo.toml` for reproducible builds. Compare behavioral changes against the pinned RuboCop before accepting an update.
