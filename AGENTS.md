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

The Makefile is the single entry point; `Rakefile` holds only gem packaging and version syncing. Tool versions (Rust and Ruby) are pinned in `mise.toml`, and every target runs its tools through `mise exec`; `make help` lists every target.

```bash
make setup            # install the pinned toolchain (mise) and fetch dependencies
make build            # cargo build
make test             # Rust + Ruby wrapper tests
make check            # fmt-check, clippy -D warnings, version check
make ci               # check + test + build, install and run the source gem  <- the gate CI runs
make install          # build --release and install to /usr/local/bin
```

`make ci` is what CI runs. Run it before saying anything is done.

## Conventions

**Comment language.** Write comments in Japanese in code and tests changed for this project. When
an existing comment in the edited section is relevant to the change, translate it where useful.
Do not translate unrelated files just to make a language-only diff. The `Makefile` stays in English
so that `make help` matches the README's Development table.

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

**The column `line_column` returns is one of those characters, and `line` is sliced by bytes**, so
`source.line(line)[..column - 1]` is the trap the two units make: it reads as "the text before the
node" and is right for every ASCII line. Two cops carried it — `Style/AccessModifierDeclarations`
and `Style/RequireOrder`, both asking whether anything precedes a node on its own line — and both
aborted the run on a line opening with a multi-byte character. Measure against the line's own start
instead, the way `rules/lint/cop_directives.rs` does:
`[..node.start_byte() - source.line_start(line)]`. Both sides of that subtraction are byte offsets,
so no unit converts.

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

**Remote inherited configurations are bounded.** Network operations time out after 30 seconds and
each response may contain at most 5 MiB. `ureq` rejects a body whose length equals the value passed
to `BodyWithConfig::limit`, so pass one byte beyond the logical maximum. Preserve tests for both the
exact limit and the first rejected byte when changing this path.

**Ask the index about the tree's shape, never the parser.** `Node::parent` walks down from the
root comparing byte ranges, and `Node::named_children` / `Node::children` open a tree cursor on
every call. A sampling profile of a run over RuboCop's own tree put 84% of the working CPU inside
those two, and it was structure being re-derived rather than anything a cop decided. `AstIndex`
records the answers on the walk it already makes:

| Instead of | Use | Shape |
|---|---|---|
| `node.parent()` | `node.parent_of(context)`, `index.parent_in_tree(node)` | one array lookup |
| `named_children(node)` | `named_children_of(node, context)` | a copy of a recorded slice |
| `nodes::children(node)` | `nodes::children_in(node, context)` | the same, filtered |
| a stack walk over descendants | `walk_named(node, context, &mut …)` | a slice of the pre-order list |
| `node.children(&mut cursor)` | `context.children(node)` | an iterator over the pre-order list |

A node the index does not know -- one of the extra trees `Metrics` parses to recover the fragments
the grammar swallowed -- falls back to the parser, so every one of these answers the same list
either way. `the_index_answers_what_the_parser_answers` holds the two to that.

The cursor forms are still there, because a helper without a context in reach cannot use the index.
**Reach for the index form when the context is at hand**; the difference is not small.

**The grammar lives in a `static`, and that is what makes a kind name `&'static str`.** Since
tree-sitter 0.27 the name tables borrow from the `Language` they were read out of -- `Node::kind`,
`TreeCursor::field_name`, `Language::node_kind_for_id` and `Language::field_name_for_id` all
returned `&'static str` before it and return a tied borrow now. `NodeExt::kind_str` is compared
against a literal at some 3,600 call sites and several helpers hand the field name on as
`&'static str`, so the answer has to outlive the tree it came from. `node_ext::LANGUAGE` is the one
`Language` the process holds; `KIND_NAMES` and `FIELD_NAMES` are built from it, and both keep
answering `&'static str` because a `static` is never dropped. **Do not build a `Language` per call
to read a name out of it** -- the borrow will not outlive the call.

Two consequences worth knowing. `ERROR` and `_ERROR` carry ids above `node_kind_count()` (65535 and
65534 against 368 kinds), so they are named by `kind_str`'s fallback rather than by the table, and
`Lint/Syntax` rests on that name: `an_error_node_is_named_the_way_the_parser_names_it` holds it to
what the parser says. And a field is read out of `FIELD_NAMES` by id rather than off the cursor --
`crate::rules::field_name_for_id(cursor.field_id())`, which
`the_field_table_answers_what_the_cursor_answers` holds to `TreeCursor::field_name`. Field ids mean
the same thing in every tree built from the same grammar, so this holds for the extra trees
`Metrics` parses too.

**A walk over a subtree is iterative, in a cop as much as in the index.** `AstIndex::collect` says
why: a rayon worker's stack is far smaller than the main thread's, so a recursion deep enough to
exhaust it aborts the whole process rather than failing one file. The tree nests once per operand
of a chain and once per bracket of a nested literal, so the depth is the source's to choose —
`Lint/UnreachableLoop` and `Lint/DuplicateHashKey` each went down on a generated file until their
walks were rewritten around an explicit stack. `push_named_children` and `push_named_children_in`
push a node's children so that popping yields them in source order; reach for one of those rather
than calling the walk from inside itself.

**A pattern built from the configuration cannot live in a `LazyLock`, so it needs the cache.**
`crate::rules::regex_cache::compiled` keeps a compiled pattern for the life of the process. Without
it, a cop rebuilds the same automaton for every file: `Layout/LineLength`'s `URISchemes` regex was
the single largest cop cost of a run until it went through the cache. `grep -n "Regex::new" src/`
and check that every hit outside a `LazyLock` is either cached or unreachable in a default run.

**The result cache index is one JSON file holding every offense, so it is written buffered.**
It carries each offense's message and the source line it was found on, which comes to 336 MB over
`ruby/ruby`. `serde_json::to_writer` hands the writer a token at a time, so pointing it at a bare
`File` turns one index into millions of `write` calls -- 115 seconds of a 135-second run, all of it
blocked in the kernel while `user` time stayed flat. A default run has the cache on, so this is the
path most users are on and the one a `--cache false` comparison never measures. Keep the
`BufWriter` in `persist_cache_index`, and record `real` beside `user` when timing this path.

**Autocorrect writes to real source files.** Corrections go through a temp file so a killed writer
cannot leave a truncated file, and permissions are preserved. Anything touching that path needs a
test that inspects the file on disk afterwards, not just the reported offenses.

**`Style/CaseLikeIf` has an unsafe upstream correction.** For an `if`/`elsif` chain whose `else`
contains another `if`, `unless`, or a modifier form of either, RuboCop reports an offense but
rewrites the inner keyword to `when` after the `else`. Ruby rejects the result. The grammar
currently accepts it without an error node, so the general syntax guard cannot detect it. Keep
the diagnostic compatible and withhold that correction; the intentional differences are recorded
in `tests/conformance/known_divergences.yml` and covered by a CLI test that checks the file on disk.

**A guard clause can change a local variable into a method call.** If the condition first assigns
`error`, converting `if error = value; raise error; end` to `raise error if error = value` moves the
read before Ruby has parsed the declaration. RuboCop's correction raises `NameError` at runtime;
the later `Lint/UselessAssignment` correction only removes an assignment that is already unused.
Keep the offense but withhold this correction when the guard reads a variable first declared in
its condition. A variable declared earlier in the scope still permits the upstream correction.
Register the intentional difference in the divergence manifest for each correction mode where it
actually occurs; full `-A` runs can take another correction path and converge to identical bytes.

**Preserve CRLF bytes when correcting a CRLF file.** RuboCop's source buffer normalizes CRLF to LF;
its writeback then changes even the bytes after `__END__`, which changes `DATA.read`. Sonicop keeps
the original line ending format on disk. After a correction it re-inspects LF text, as RuboCop does,
so later passes still find the same offenses. New lines inserted by corrections must become CRLF
before writeback. The intentional byte difference is recorded in the divergence manifest.
The tree-sitter comment node includes the `\r` in CRLF, while RuboCop's comment token excludes it.
Trim that byte from indexed comment ranges before cops calculate offense locations or source text.

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

**The input is collected by running the specs, not by reading them.** `make spec-capture` loads
`spec_capture.rb` into upstream's own suite and records what reaches `expect_offense` — by then
the source is a plain string and the cop is configured exactly as the example meant it. Reading
the files instead leaves four whole categories out, and with them 54 of the 609 cops: a
`shared_examples` driven by `it_behaves_like`, a `#{}` the spec evaluates, an
`expect_offense(wrap(<<~RUBY))` where the heredoc is an argument, and a `cop_config` a surrounding
`context` builds. Two cops (`Lint/Syntax`, `Bundler/GemFilename`) never call `expect_offense` at
all, so the hook also wraps `CopHelper#_investigate` and `Commissioner#investigate`.

Running the suite needs upstream's development bundle, which the pinned tree at
`~/tmp/rubocop-v1.89.0` does not ship with. `bundle install` fails there under Ruby 4 — install
`rspec`, `webmock` and `mcp` as plain gems and drive RSpec through
`ruby -e 'require "rspec/core"; exit RSpec::Core::Runner.run(ARGV)'` instead. The Makefile does
not know where the upstream tree, the capture hook and the generator live; pass them as
`UPSTREAM_SPEC_TREE=`, `SPEC_CAPTURE_HOOK=` and `SPEC_FIXTURE_GEN=`.

As of 2026-08-23 all **11,300** recorded cases match, across the **555** cops the specs reach, with
no entry in `spec_known_divergences.yml`. A difference appearing there is a regression, not a
backlog item.

Two things about this gate are easy to get backwards.

**Its value is in the cases where upstream stays silent.** Corpora can only show what upstream
reports, so **over-detection is invisible to them** — the shapes upstream is quiet about are
infinite and appear in real code only by accident. Roughly 44% of the recorded cases are
`no_offenses` ones, and that half is the point of the file.

**Reproducing the recording means reproducing its conditions.** Seven ways of getting that wrong
have already turned the harness into a difference generator, and each looked exactly like a bug in
a cop:

| Mistake | What it produced |
|---|---|
| Running `.rb` where upstream ran `.gemspec` / `.gemfile` | Every Gemspec and Bundler cop silently matched nothing |
| Passing the printed `length` as the caret count | Every offense spanning lines became a `range` difference — 57 in three Metrics cops alone |
| Feeding a recorded output through `CopCase::corrected` | It dedents, for the hand-written `<<~RUBY` cases; recorded output loses its leading newline and indentation. **20 differences, all of them mine** — use `corrected_verbatim` |
| Treating `foo&.bar` as a `send` | The grammar spells `send` and `csend` alike; upstream's `:send` arm excludes `csend`, and reading it as one makes a cop claim its own receiver is non-nil |
| Reading and writing the case with Python's default newline handling | Universal-newline translation turns a CRLF case into an LF one on the way in **and** on the way out, so `Layout/EndOfLine` — the cop whose whole subject is the line ending — records an expectation nobody can reproduce. Both sides need `newline=""` |
| Serialising the recorded `cop_config` as JSON | JSON has no symbol, so `{ any?: :none? }` lands as `{"any?": "none?"}` and YAML reads it back as string keys. `Style/InverseMethods` looks its methods up by symbol, so the recorded configuration disabled the very thing the case was testing. Record with `inspect`, and write the YAML **as a block mapping** — Psych reads an unquoted `:any?` as a symbol, a flow mapping `{:any?: :none?}` is a syntax error, and quoting it makes it a string again |
| Omitting the `Enabled: true` the generator writes for `--only` | A cop that ships disabled behaves differently with that one line: `Style/DisableCopsWithinSourceCodeDirective` refuses to be switched off by a directive **only** when `Enabled` is explicitly true, so without it the case's own `# rubocop:disable Style` silenced the cop under test |

The first two accounted for 54 of the first 65 failures; the last three for 39 of the 133 that
remained after the cop work. Before believing a difference, reproduce
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

**One grammar node often stands for several upstream ones.** These have each cost more than one
cop, and the second time is always in a cop whose author had read the first fix:

| Written as | Grammar | Upstream |
|---|---|---|
| `a && b`, `a \|\| b` | `binary` | `and` / `or` — **not** a `send`, so `operator_method?` says no |
| `a << b`, `a + b` | `binary` | `send` — an operator call, which `(send (lvar :a) :<< …)` matches |
| `/re/ =~ str` | `binary` | `match_with_lvasgn` — answers nothing to `arguments` |
| `foo&.bar` | `call` | `csend` — upstream's `:send` arm excludes it, `on_csend` is separate |
| `%w[…]`, `%i[…]` | `string_array` / `symbol_array` | `array`, with `percent_literal?` true |
| `%w[…]`'s elements | `bare_string` / `bare_symbol` | `str` / `sym` |
| `/foo/`'s text | `string_content` | `str` |
| a heredoc's body | one `heredoc_content` per run | one `str` **per line** |
| `{ a: 1 }`'s key | `hash_key_symbol` | `sym` |
| `(foo)` | `parenthesized_statements` | `begin` — and node accessors see through it |
| a `def`'s body | `body_statement` between the two | the body hangs off the `def` directly |
| the top level | `program` is a parent | `node.parent` is **nil** |
| `_1`, `it` | `block` | `numblock` / `itblock` |

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

**The grammar can also accept Ruby that `parser` rejects.** With a previously declared local,
`value [0] += 1` is an index assignment; without that declaration, Ruby reads `value [0]` as a
method call with an array argument and rejects the assignment operator. The tree-sitter grammar
builds `operator_assignment` in both cases. `Lint/Syntax` uses the existing variable analysis to
distinguish them. Check both sides against RuboCop when updating the grammar: fixing the valid
case alone can turn a false syntax offense into a missing one on invalid source, allowing `-A` to
rewrite a file RuboCop refuses to inspect. Numbered parameters (`_1` through `_9`) also need a
version check: RuboCop accepts their spaced index assignment starting with target Ruby 3.2,
including at top level; the variable analysis does not register implicit numbered parameters as
ordinary locals. When the right-hand side is missing at the operator, RuboCop stops recovery at
that operator; tree-sitter's later errors from the same expression must not become extra offenses.

## Conformance measurement

`CONFORMANCE.md` records offense-by-offense comparisons against five pinned corpora (18,251 files).
The commits are pinned because the numbers move with them. Anything that changes file discovery or
path matching can move the target-file lists, so re-measure after touching `src/config/paths.rs`.
The behavioral oracle stays RuboCop 1.89.0 even when the corpus checkout has advanced to a later
release. If a measurement script's version guard reads that checkout as its source version, put the
installed 1.89.0 gem's `lib/rubocop/version.rb` in a temporary `CORPUS_ROOT` and pass the pinned
corpus by absolute path. Keep the guard enabled and record the gem version and corpus commit.

## Versioning

`Cargo.toml` and `lib/sonicop/version.rb` must agree; `make version-check` enforces it and CI fails
otherwise. Use `rake version:set VERSION=x.y.z` rather than editing either by hand.
