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

All 609 cops switched on, on both sides, over four projects — rubocop/rubocop, mastodon/mastodon,
rails/rails and Homebrew/brew, 10,792 files between them. A cop counts as an **exact match** only
when its offenses agree completely: every position, message, severity and correctable flag, with
nothing extra on either side.

<!-- conformance:start -->
| Department | Cops | Exercised | Exact match | Diverging |
|---|---:|---:|---:|---:|
| Bundler | 7 | 3 | **3 ✓** | 0 |
| Gemspec | 10 | 4 | **4 ✓** | 0 |
| Layout | 100 | 84 | **84 ✓** | 0 |
| Lint | 157 | 80 | 78 | 2 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 0 | 0 | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 6 | **6 ✓** | 0 |
| Style | 298 | 234 | 232 | 2 |
| **Total** | **609** | **440** | **436 (99.1%)** | **4** |
<!-- conformance:end -->

**Read the *Exercised* column first.** A cop these corpora never made fire contributes nothing
either way — its silence is indistinguishable from agreement — so the 169 that did not fire are
outside the measurement rather than passing it. That is why the percentage is taken over 440 and
not over 609.

The four that diverge: `Lint/Syntax` (1,275 positions, all of them Homebrew's — the two parsers
recover differently after a syntax error, and **the set of files each calls unparseable is
identical**), `Style/EmptyElse` (42), `Style/DisableCopsWithinSourceCodeDirective` (3) and
`Lint/InterpolationCheck` (2).

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

The CLI accepts RuboCop's server/LSP/MCP, plugin, and cache flags to keep existing command lines
parse-compatible. Sonicop does not provide server transports, Ruby plugin execution, cache reuse,
custom Ruby cops, or cops outside the implemented set.

Most of those flags say so. `--server`, `--no-server`, `--lsp`, `--mcp`, and `--plugin` each print
a one-line notice on stderr. The cache flags print nothing, and only one of them is silent for a
good reason:

- `--cache=false` asks for no caching, which sonicop already satisfies, so silence is the correct
  answer.
- `--cache=true` asks for cache reuse, which sonicop does not provide, and it is silent anyway.

Cop settings behave like the second case. A setting sonicop does not implement is ignored without
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

| Corpus | Files | RuboCop parallel | Sonicop parallel | RuboCop single | Sonicop single |
|---|---:|---:|---:|---:|---:|
| rubocop/rubocop | 1,765 | 12.71 s | **4.59 s** | 42.74 s | **13.75 s** |
| mastodon/mastodon | 3,290 | 29.95 s | **6.48 s** | 37.60 s | **15.88 s** |
| Homebrew/brew | 2,179 | 18.20 s | **4.31 s** | 38.72 s | **11.67 s** |
| rails/rails | 3,551 | 52.55 s | **16.88 s** | 162.55 s | **63.97 s** |
| ruby/ruby | 7,466 | 132.17 s | **43.10 s** | 199.78 s | **76.07 s** |

The gap is 2.8x to 4.6x in parallel and 2.4x to 3.3x single-process, so no single corpus summarizes
it. **Read the single-process column and treat the parallel one as indicative.** Measuring the same
two binaries three times over a day put the single-process figures within 16% of each other every
time, while the parallel ratio on RuboCop's own tree moved between 3.3x and 9.2x purely with what
else the machine was doing. Single-process measures the engines; parallel measures the engines plus
how well each one's scheduling happens to fit that tree on that run.

The speed is not bought by skipping work: over those same 394 cops the two agree on **every offense**
on RuboCop's own tree, on Rails and on Mastodon — 188,812 offenses with nothing on either side of the
ledger — and autocorrect is byte-identical on the first and the last.

Two details matter for reproducing this. RuboCop **silently turns `--parallel` off when combined
with `--cache false`**, so its parallel runs here use a cache directory that is deleted before each
run rather than disabled; timing it with `--cache false --parallel` measures a single process and
overstates the difference. RuboCop's default is a single process, while Sonicop is parallel unless
`--no-parallel` is passed.

```bash
# RuboCop, parallel, cold cache, its full default set of 394 cops
rubocop --force-default-config --cache true --cache-root "$(mktemp -d)" \
        --no-color --parallel -f quiet

# Sonicop
sonicop --force-default-config --format quiet
```

Machine: Apple M2 (8 cores), Ruby 4.0.6 with YJIT available, RubyGems-installed RuboCop 1.89.0.
The one-minute load average was 4.0 when the run started and 3.1 when it finished — the machine was
in use, not idle. RuboCop's own tree was re-measured on its own afterwards, at load 3.7 rising to 4.0,
because the first pass over it ran while a release build was still finishing and its numbers came out
60% high; a row measured under a different load cannot sit in the same table as the others. Both tools
ran under the same conditions, so the ratios hold, but the absolute
seconds are not a floor: expect better on a quiet machine. Anything competing for cores inflates
both sides, and not by the same factor on each, which is what makes the parallel column move as much
as it does. If the absolute numbers matter to you, measure on an idle machine and record the load
either side of the run — a figure without that context cannot be compared with another one.

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

Dependencies are updated with `depup --install`. The Ruby grammar dependency is pinned to an exact
fork commit in `Cargo.toml` for reproducible builds.

## License

[MIT](LICENSE). The bundled RuboCop default configuration and parser dependency retain their
upstream notices in [NOTICE](NOTICE) and [`licenses/`](licenses/).
