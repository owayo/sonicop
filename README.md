<h1 align="center">
  <img src="docs/images/sonicop_logo_header.png" width="600" alt="Sonicop">
</h1>

<p align="center">
  <strong>A fast, native RuboCop-compatible Ruby linter and formatter written in Rust.</strong>
</p>

<p align="center">
  <a href="https://github.com/owayo/sonicop/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/owayo/sonicop/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://rubygems.org/gems/sonicop"><img alt="Gem Version" src="https://img.shields.io/gem/v/sonicop"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/owayo/sonicop"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>

---

## Overview

Sonicop is a fast Ruby linter and formatter that runs as a native executable without starting a
Ruby process. Existing `.rubocop.yml` files work as-is, including nested configuration,
inheritance, file inclusion and exclusion, severity, and autocorrect settings.

It uses the actively maintained
[owayo/tree-sitter-ruby](https://github.com/owayo/tree-sitter-ruby) grammar, inspects files in
parallel, and applies corrections atomically. Its RuboCop 1.89-compatible CLI and JSON output fit
existing editor and CI integrations with minimal changes.

## Features

Sonicop implements cops in the Bundler, Gemspec, Layout, Lint, Metrics, Migration, Naming,
Security, and Style departments. The binary itself is the authoritative list:

```bash
# Every recognized cop and its implementation status
sonicop --show-cops
```

**All 609 RuboCop 1.89 cops are implemented**, matched name for name against the upstream registry.
That includes the 159 shipped as `Enabled: pending` and the 56 shipped as `Enabled: false`, which a
default run does not reach on either side — name them with `--only` or switch them on in a
configuration, exactly as with RuboCop. Unknown cop names still fail validation unless
`--ignore-unrecognized-cops` is supplied.

### Cop conformance

All 609 cops switched on, on both sides, over the 37,491 cases RuboCop's own specs supply, each
run at the `TargetRubyVersion` its spec asked for. A cop counts as an **exact match** only when its
offenses agree completely: every position, message, severity and correctable flag, with nothing
extra on either side.

<!-- conformance:start -->
| Department | Cops | Exercised | Exact match | Diverging |
|---|---:|---:|---:|---:|
| Bundler | 7 | 7 | **7 ✓** | 0 |
| Gemspec | 10 | 10 | **10 ✓** | 0 |
| Layout | 100 | 100 | 90 | 10 |
| Lint | 157 | 157 | 147 | 10 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 1 | **1 ✓** | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 7 | **7 ✓** | 0 |
| Style | 298 | 298 | 288 | 10 |
| **Total** | **609** | **609** | **579** | **30** |
<!-- conformance:end -->

**Read the *Exercised* column first.** A cop nothing here made fire contributes neither way — its
silence is indistinguishable from agreement, so it would be counted as agreement without ever
being asked. **Every one of the 609 fires here**, which is what makes the *Exact match* column
mean what it says.

Three of them took a run of their own. `Lint/DeprecatedReference`, `Lint/NameTypo` and
`Lint/UnusedPrivateMethod` report nothing without a `rubydex` project index, which needs the gem
installed and `AllCops/UseProjectIndex` switched on — and `Lint/DeprecatedReference` needs more
than that: the call has to sit inside a class inheriting the one whose method carries the
`@deprecated` tag. Upstream's own specs open with an `expect_no_offenses` saying the cop is silent
without the index, which is easy to read as "unreachable"; it is not.

Getting *Exact match* to 609 while reducing *Diverging* to zero is the current goal. More real Ruby
does not get there: the 56 cops RuboCop ships disabled, and much of what it ships as pending, never
fire in a plain run however large the tree. What does reach every one of them is the input its own
specs supply — the cases recorded in `tests/fixtures/upstream_spec_capture.jsonl` touch **609 of 609
cops**, measured. Two things about running them are easy to get wrong, and both silently shrink the
table rather than failing:

- **`TargetRubyVersion` is part of the input, not a global.** Pinning everything at 2.7 leaves
  `Style/ArrayIntersect`, `Naming/BlockForwarding`, `Style/ItBlockParameter` and eleven others
  unable to fire at all. Each case is run at the version its spec asked for.
- **The filename is what several cops inspect.** `Bundler/*` needs a `Gemfile`, `Gemspec/*` a
  `.gemspec`, and `Naming/FileName` reads the name itself. Writing every case as `.rb` had all 17
  of those cops matching nothing.

A separate direct oracle sweep on 2026-08-29 examined 11,506 cases extractable from the upstream
cop specs. RuboCop could not read 226 of those inputs and crashed on one; among the measurable
cases, Sonicop had **zero detection differences and zero correction differences**. This result is
not folded into the table above: 51 cops had no directly extractable case, and three directive cops
cannot be measured under `--only`, so the sweep does not prove that all 609 cops are exact. The
direct sweep also uses neutral/default conditions rather than preserving every example's
`TargetRubyVersion`. The four Ruby 3.4-sensitive cops changed in this pass were therefore compared
separately at 3.4, where their messages and locations matched RuboCop exactly.

Configuration is measured separately, because a cop that only matches at its default value is half
a cop. Every one of the 111 cops carrying an `Enforced*` setting was switched to a **non-default**
value at once and the corpus re-run: **99.995% of 622,317 offenses match**, with 85 of the 96 cops
that fired matching exactly. The residue is 10 cops of at most 17 offenses each, plus one where
RuboCop crashes and sonicop does not; the list is in [CONFORMANCE.md](CONFORMANCE.md).

Reproduce either table with `scripts/conformance_table.rb`.

## Installation

```bash
gem install sonicop
```

Platform gems include native executables for Linux, macOS, and Windows. When a prebuilt platform
gem is unavailable, the source gem builds the executable with Cargo during installation.

You can also install the latest source directly:

```bash
cargo install --git https://github.com/owayo/sonicop
```

## Usage

```bash
# Inspect the current project
sonicop

# Select cops or departments
sonicop --only Layout,Style/StringLiterals app spec

# Safe correction / all correction
sonicop -a
sonicop -A

# RuboCop-shaped JSON
sonicop --format json

# Editor input
printf '%s\n' 'value=10000' | sonicop --stdin example.rb --format json

# List recognized cops and their implementation status
sonicop --show-cops
```

Key compatibility flags include `-l`, `-x`, `--only`, `--except`, `-s/--stdin`, `-P/--parallel`,
`-f/--format`, `-a/--autocorrect`, `-A/--autocorrect-all`, `-L/--list-target-files`,
`-c/--config`, `-v/--version`, and `-V/--verbose-version`.

### Configuration

Sonicop resolves `.rubocop.yml` from each target file, so nested configurations apply within one
run. Local and HTTPS `inherit_from`, `inherit_gem`, `inherit_mode`,
`AllCops/DisabledByDefault`, `Include`, and `Exclude`, plus per-cop `Enabled`, `Exclude`,
`Severity`, `Safe`, `SafeAutoCorrect`, and cop settings are supported. Cops supplied by declared
plugins are accepted as recognized-but-unimplemented without executing Ruby plugin code.
Remote configuration requests use 30-second network timeouts, and each response is limited to
5 MiB.

```yaml
inherit_from: .rubocop_todo.yml

AllCops:
  Exclude:
    - "vendor/**/*"

Layout/LineLength:
  Max: 100

Style/StringLiterals:
  EnforcedStyle: single_quotes
```

The CLI accepts RuboCop's server/LSP/MCP and plugin flags to keep existing command lines
parse-compatible. Sonicop does not provide server transports, Ruby plugin execution, custom Ruby
cops, or cops outside the implemented set. Each of those flags says so: `--server`, `--no-server`,
`--lsp`, `--mcp`, and `--plugin` print a one-line notice on stderr.

The cache flags are honoured rather than merely parsed. Sonicop keeps a result cache of its own and
serves a stored report for a file whose size, modification time and permission bits have not moved
since it was inspected.

- Caching is on by default. `--cache false` turns it off, as does `AllCops/MaxFilesInCache: 0` in a
  configuration file.
- `--cache-root DIR` chooses where it lives. Without it the root is `$XDG_CACHE_HOME/sonicop`, or
  `~/Library/Caches/sonicop` on macOS, or `~/.cache/sonicop`. `--cache-root` cannot be combined
  with `--cache false`.
- `AllCops/MaxFilesInCache` bounds how many reports are kept, defaulting to RuboCop's 20,000.
- Autocorrect runs, `--stdin`, `--profile` and `--memory` neither read nor write it.
- It is not shared with RuboCop's cache: the formats are unrelated, and an entry is only served back
  to a build of Sonicop identical to the one that wrote it.

Cop settings are the silent case. A setting sonicop does not implement is ignored without
any warning, and so is a setting whose name is simply misspelled. **A run that reports no offenses
is therefore not evidence that a setting took effect**, because an ignored setting and a clean file
produce the same output. *Cop conformance* above says which values have been measured.

Cop *names* are checked: an unrecognised cop in a configuration file stops the run with an error.
It is the settings inside a recognised cop that pass unvalidated.

### Conformance

The implemented cops are verified against RuboCop 1.89.0 over five Ruby projects — RuboCop itself,
Rails, Ruby, Homebrew and Mastodon — totalling 18,251 files, with the upstream default
configuration on both sides. Every offense is compared by cop, path, line, column, last line, last
column, length, message, severity and correctability.

Three of the five match **exactly**: RuboCop's own tree (5,766 offenses), Rails (167,760) and
Mastodon (15,286), with no excess, no shortfall and no metadata differences. The target file lists
match exactly on all five — paths, not just counts, compared as sets. What remains is concentrated in
`Lint/Syntax`. Most of it is RuboCop's
LALR parser recovering from an error and emitting diagnostics a tree-sitter parse cannot
reconstruct, and the resulting position differences go in both directions. On Homebrew all 997
missing and 263 excess positions are `Lint/Syntax`, but the **sets of files rejected as syntax
errors are exactly the same: 569 versus 569**, with no file rejected only by Sonicop. The 263 excess
positions occur in 135 shared syntax-error files and every one follows a diagnostic at a position
shared by both tools, so they are recovery-position differences rather than a separate acceptance
bug. At Ruby 3.1, which supports the syntax used there, both tools report zero `Lint/Syntax` offenses.
Autocorrect is byte-identical on RuboCop's own tree and on Mastodon, the two corpora held as a hard
line: a change that breaks byte equality there is a regression, not a new known divergence.

See [CONFORMANCE.md](CONFORMANCE.md) for the commands, the corpus commits these counts were
measured at, and the two ways a measurement of this kind can mislead you.

### Performance

Measured over all five conformance corpora. Both tools were given their own bundled default
configuration (`--force-default-config`), so neither reads the project's `.rubocop.yml`, and on every
corpus the two resolve **the same number of files** — which is what these timings need, since it
means neither side is inspecting less. Path-by-path equality is a stronger claim, established under
*Conformance* above for all five, but on the pinned corpus revisions rather than on these timing runs.

Both tools run their full default set — **the same 394 cops**, matched name for name — so neither
side is restricted and the comparison is like-for-like as it stands. (394 is what is left of the 609
once the 159 RuboCop ships as `Enabled: pending` and the 56 it ships as `Enabled: false` are set
aside; a default run reaches neither group on either side.) Times are the fastest of two
warmed runs.

| Corpus | Files | Offenses | RuboCop parallel | Sonicop parallel | RuboCop single | Sonicop single |
|---|---:|---:|---:|---:|---:|---:|
| rubocop/rubocop | 1,780 | 5,826 | 12.84 s | **2.49 s** | 46.09 s | **8.01 s** |
| mastodon/mastodon | 3,292 | 15,293 | 18.66 s | **3.16 s** | 35.88 s | **7.10 s** |
| Homebrew/brew | 2,296 | 51,527 | 19.58 s | **2.93 s** | 40.25 s | **6.65 s** |
| rails/rails | 3,562 | 168,615 | 36.24 s | **8.79 s** | 87.41 s | **19.64 s** |
| ruby/ruby | 7,477 | 765,975 | 97.46 s | **18.28 s** | 191.01 s | **40.58 s** |

The gap is 4.1x to 6.7x in parallel and 4.5x to 6.1x single-process, so no single corpus summarizes
it. **Read the single-process column and treat the parallel one as indicative.** Measuring the same
two binaries three times over a day put the single-process figures within 16% of each other every
time, while the parallel ratio on RuboCop's own tree moved between 3.3x and 9.2x purely with what
else the machine was doing. Single-process measures the engines; parallel measures the engines plus
how well each one's scheduling happens to fit that tree on that run.

The speed is not bought by skipping work. Over those same 394 cops the two find the **same number of
offenses** on every corpus in the table, and on RuboCop's own tree and on Mastodon every one of them
is at the same position with the same message and severity. Rails, at 168,615 offenses, differs in
two of them — one `Style/CaseLikeIf` Sonicop reports and one `Metrics/AbcSize` it does not — and
RuboCop's own tree differs in one offense's `correctable` flag. Autocorrect is byte-identical on the
first and the last.

Four details matter for reproducing this. RuboCop **silently turns `--parallel` off when combined
with `--cache false`**, so its parallel runs here use a cache directory that is deleted before each
run rather than disabled; timing it with `--cache false --parallel` measures a single process and
overstates the difference. RuboCop's default is a single process, while Sonicop is parallel unless
`--no-parallel` is passed. Both sides need a **cold** cache: Sonicop caches by default too, so a
second run over the same tree answers from its own cache and measures nothing about the engine —
give each tool a throwaway cache root. And the cache root must be a **real path**: macOS `mktemp -d`
returns `/var/folders/…`, whose `/var` is a symlink, and RuboCop refuses such a location and runs
with no cache at all.

```bash
# RuboCop, parallel, cold cache, its full default set of 394 cops
root=$(mktemp -d /private/tmp/bench.XXXXXX)
rubocop --force-default-config --cache true --cache-root "$root" \
        --no-color --parallel -f quiet

# Sonicop, cold cache
sonicop --force-default-config --cache-root "$root" --format quiet
```

Writing the cache is part of these numbers, and it is not free: the index holds every offense with
the source line it was found on, which is 336 MB over `ruby/ruby`. A second run against a warm cache
answers in 1.88 s there, and in 0.42 s over Rails.

Machine: Apple M2 (8 cores), Ruby 4.0.6 with YJIT available, RubyGems-installed RuboCop 1.89.0.
Measured on 2026-08-30 against the corpora at `rubocop_rubocop` 2693129, `mastodon_mastodon` b59ddc7,
`Homebrew_brew` b42173b, `rails_rails` a19f07f and `ruby_ruby` 22e4a75. The one-minute load average
was between 3.3 and 10.1 as each row was taken — most of it RuboCop's own parallel workers, which is
inherent to measuring them. **The machine was in use, not idle.** Both tools ran back to back under
the same conditions on each corpus, so the ratios hold, but the absolute seconds are not a floor:
expect better on a quiet machine. Anything competing for cores inflates both sides, and not by the
same factor on each, which is what makes the parallel column move as much as it does. If the
absolute numbers matter to you, measure on an idle machine and record the load either side of the
run — a figure without that context cannot be compared with another one.

## Development

`make` is the single entry point; `make help` lists every target. The Rakefile holds the gem
packaging tasks that `make` delegates to.

```bash
make build   # debug build
make check   # fmt, clippy, Rust tests, Ruby wrapper tests, version consistency
make gem     # source gem
```

### Adding a cop

A cop is one file under `src/rules/<department>/<cop>.rs` exposing a single
`check(context, offenses)`, plus one line in that department's `mod.rs`:

```rust
department_rules! {
    "Layout";
    line_length => ("LineLength", Convention),
}
```

That line is the only place the cop's name and default severity are written. Inside the cop the
name stays implicit: `context.setting("Max")` reads `Layout/LineLength: Max`, and
`context.offense(message, range)` reports under the cop's own name at its configured severity. A
cop that spelled its name a second time could disagree with the registry, and nothing in the type
system would catch it.

Prefer `context.nodes_of("kind")` over walking every node: each cop runs on every file, so a full
walk per cop is what makes inspection scale with the registry rather than with the file.

`Cargo.toml` is the single source of truth for the version. `lib/sonicop/version.rb` is generated
from it by `make version-sync` and committed, because the gemspec reads it at package time. CI
fails when the two disagree.

`config/default.yml` is vendored from upstream RuboCop; re-fetch it with
`scripts/sync_default_yml.sh <rubocop-version>`, which records the source version in the file
header.

`src/display_width_table.rs` is generated and committed too. RuboCop measures display columns with
the `unicode-display_width` gem, so the table is taken from the gem rather than restated by hand —
an exception table written out by hand had already drifted far enough to draw the wrong number of
carets under decomposed Japanese. Regenerate it with
`ruby scripts/dump_display_width.rb > src/display_width_table.rs`, which records the gem and Unicode
versions in the file header.

Dependencies are updated with `depup --install`. The Ruby grammar dependency is pinned to an exact
fork commit in `Cargo.toml` for reproducible builds.

## License

[MIT](LICENSE). The bundled RuboCop default configuration and parser dependency retain their
upstream notices in [NOTICE](NOTICE) and [`licenses/`](licenses/).
