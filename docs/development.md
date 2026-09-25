# Development

Notes for working on sonicop beyond the [Development](../README.md#development) section of the README. `make` is the single entry point and lists every target; tool versions (Rust and Ruby) are pinned in `mise.toml`.

## Make and the Rakefile

The Rakefile holds only the gem packaging and version tasks, and `make` calls them: `make gem` builds the source gem, `make gem-platform GEM_PLATFORM=<platform>` builds a platform gem around the release binary, and `make version-sync` regenerates `lib/sonicop/version.rb`. `make ci` ends with `make gem-check`, which builds the source gem, installs it into `tmp/gems` and runs the installed command, so it includes a release build.

`make cop-coverage` counts the cops the hand-written tests reach. `make spec-capture` and `make spec-fixtures` re-record upstream's spec cases and their expectations; they need an upstream checkout and tools outside this repository, described in the comments of the Makefile.

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

Dependencies are updated with `depup --install`. The Ruby grammar dependency is pinned to an exact fork commit in `Cargo.toml` for reproducible builds.
