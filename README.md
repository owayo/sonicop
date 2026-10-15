<p align="center">
  <img src="docs/images/sonicop_logo_header.png" width="320" alt="sonicop">
</p>

<h1 align="center">sonicop</h1>

<p align="center">
  A fast, native RuboCop-compatible Ruby linter and formatter written in Rust
</p>

<!-- standard:badges:start -->
<h3 align="center">Supported Platforms</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
</p>

<p align="center">
  <a href="https://github.com/owayo/sonicop/actions/workflows/ci.yml"><img src="https://github.com/owayo/sonicop/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/sonicop/releases/latest"><img src="https://img.shields.io/github/v/release/owayo/sonicop" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/owayo/sonicop" alt="License"></a>
  <a href="https://rubygems.org/gems/sonicop"><img src="https://img.shields.io/gem/v/sonicop" alt="RubyGems"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>
<!-- standard:badges:end -->

---

Sonicop is a fast Ruby linter and formatter that runs as a native executable without starting a Ruby process. Existing `.rubocop.yml` files work as-is, including nested configuration, inheritance, file inclusion and exclusion, severity, and autocorrect settings.

It uses the actively maintained [owayo/tree-sitter-ruby](https://github.com/owayo/tree-sitter-ruby) grammar, inspects files in parallel, and applies corrections atomically. Its RuboCop 1.89-compatible CLI and JSON output fit existing editor and CI integrations with minimal changes.

## Features

Sonicop implements cops in the Bundler, Gemspec, Layout, Lint, Metrics, Migration, Naming, Security, and Style departments. The binary itself is the authoritative list:

```bash
# Every recognized cop and its implementation status
sonicop --show-cops
```

**All 609 RuboCop 1.89 cop names are implemented**, matched name for name against the upstream registry, including project-index behavior. The registry includes the 159 shipped as `Enabled: pending` and the 56 shipped as `Enabled: false`, which a default run does not reach on either side — name them with `--only` or switch them on in a configuration, exactly as with RuboCop. Unknown cop names still fail validation unless `--ignore-unrecognized-cops` is supplied.

### Cop conformance

Measured on 2026-10-15 against RuboCop 1.89.0: **42,433 inputs in 1,543 configuration groups** from the upstream spec capture, plus **174 project-index cases and two filename-dependent cases**. The sweep preserves each group's `TargetRubyVersion`, filename type and Ruby configuration values. A cop counts as an **exact match** only when all offense records agree, including start and end positions, length, message, severity, correctability and duplicate records. Comparisons are kept separate for each run.

Six other configuration groups (161 inputs) are unmeasured: RuboCop's CLI rejects their null `EnforcedStyle` settings, although its spec helpers accept them. The table describes the measurable inputs, not every possible configuration.

<!-- conformance:start -->

| Department | Cops | Exercised | Exact match | Diverging |
|---|---:|---:|---:|---:|
| Bundler | 7 | 7 | **7 ✓** | 0 |
| Gemspec | 10 | 10 | **10 ✓** | 0 |
| Layout | 100 | 100 | **100 ✓** | 0 |
| Lint | 157 | 157 | **157 ✓** | 0 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 1 | **1 ✓** | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 7 | **7 ✓** | 0 |
| Style | 298 | 298 | **298 ✓** | 0 |
| **Total** | **609** | **609** | **609** | **0** |

<!-- conformance:end -->

**All 609 cops fired and matched in this sweep.** `Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod` are exercised with `AllCops: UseProjectIndex: true`, including references across files. This coverage result does not remove the real-code parser recovery differences recorded below.

The measurement conditions and remaining gaps are described in [docs/cop-conformance.md](docs/cop-conformance.md).

## Installation

<!-- standard:install:start -->
### Cargo

Requires Rust 1.98 or later.

```bash
cargo install --git https://github.com/owayo/sonicop --locked
```

### RubyGems

```bash
gem install sonicop
```

### From Source

Requires [mise](https://mise.jdx.dev/) (the Rust toolchain is pinned in `mise.toml`).

```bash
git clone https://github.com/owayo/sonicop.git
cd sonicop
make install
```

`make install` installs to `/usr/local/bin`. Set `INSTALL_PATH` to change it (for example `make install INSTALL_PATH="$HOME/.local/bin"`).
<!-- standard:install:end -->

### Platform gems

Platform gems on [RubyGems](https://rubygems.org/gems/sonicop) include native executables for Linux, macOS, and Windows. When a prebuilt platform gem is unavailable, the source gem builds the executable with Cargo during installation.

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

Key compatibility flags include `-l`, `-x`, `--only`, `--except`, `-s/--stdin`, `-P/--parallel`, `-f/--format`, `-a/--autocorrect`, `-A/--autocorrect-all`, `-L/--list-target-files`, `-c/--config`, `-v/--version`, and `-V/--verbose-version`.

## Configuration

Sonicop resolves `.rubocop.yml` from each inspected file, so nested configurations apply to its cops. For target discovery, a directory argument uses that directory's configuration across its walk, matching RuboCop; list a nested directory separately to apply its own `Include` and `Exclude` rules. Local and HTTPS `inherit_from`, `inherit_gem`, `inherit_mode`, `AllCops/DisabledByDefault`, `Include`, and `Exclude`, plus per-cop `Enabled`, `Exclude`, `Severity`, `Safe`, `SafeAutoCorrect`, and cop settings are supported. Cops supplied by declared plugins are accepted as recognized-but-unimplemented without executing Ruby plugin code. Remote configuration requests use 30-second network timeouts, and each response is limited to 5 MiB.

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

The CLI accepts RuboCop's server/LSP/MCP and plugin flags to keep existing command lines parse-compatible. Sonicop does not provide server transports, Ruby plugin execution, custom Ruby cops, or cops outside the implemented set. Each of those flags says so: `--server`, `--no-server`, `--lsp`, `--mcp`, and `--plugin` print a one-line notice on stderr.

Set `AllCops/UseProjectIndex: true` to enable project-wide analysis, including `Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod`. The native executable can index project sources without Ruby. With `AllCops/ProjectIndexIncludesGems: true`, the Ruby gem wrapper also indexes bundled gem sources when Bundler is loaded. Project-index runs bypass the result cache, since changes to another file can change a diagnostic in the inspected file.

The cache flags are honoured rather than merely parsed. Sonicop keeps a result cache of its own and serves a stored report for a file whose size, modification time and permission bits have not moved since it was inspected.

- Caching is on by default. `--cache false` turns it off, as does `AllCops/MaxFilesInCache: 0` in a configuration file.
- `--cache-root DIR` chooses where it lives. Without it the root is `$XDG_CACHE_HOME/sonicop`, or `~/Library/Caches/sonicop` on macOS, or `~/.cache/sonicop`. `--cache-root` cannot be combined with `--cache false`.
- `AllCops/MaxFilesInCache` bounds how many reports are kept, defaulting to RuboCop's 20,000.
- Autocorrect runs, `--stdin`, `--profile` and `--memory` neither read nor write it.
- It is not shared with RuboCop's cache: the formats are unrelated, and an entry is only served back to a build of Sonicop identical to the one that wrote it.

Cop settings are the silent case. A setting sonicop does not implement is ignored without any warning, and so is a setting whose name is simply misspelled. **A run that reports no offenses is therefore not evidence that a setting took effect**, because an ignored setting and a clean file produce the same output. [Non-default settings](docs/cop-conformance.md#non-default-settings) says which values have been measured.

Cop *names* are checked: an unrecognised cop in a configuration file stops the run with an error. It is the settings inside a recognised cop that pass unvalidated.

## Conformance

Sonicop implements **609 / 609** cop names from its RuboCop 1.89.0 specification, including project-index behavior. On five pinned Ruby projects (18,251 target files), both tools discover the same path sets. Their default-config JSON reports match offense by offense on RuboCop's own tree (5,766 offenses), Rails (167,760) and Mastodon (15,286). Homebrew differs only in `Lint/Syntax` recovery positions: both reject the same 569 files. Ruby still has parser and cop differences; it is not counted as exact.

Autocorrect is also compared on separate copies of each project. Some differences are deliberate safety decisions: RuboCop can write invalid Ruby, lose a value through assignment order, or turn a local-variable read into a `NameError`. Each known difference is pinned by corrected-file hashes in [the divergence manifest](tests/conformance/known_divergences.yml). For the pinned trees, `-a` has 0 new differences and `-A` has 0 new differences among the four corpora RuboCop can finish; Ruby's reference aborts before a complete correction tree exists. The corrected trees are byte-identical on Homebrew, while the other known differences remain visible in the gate.

The full Ruby corpus has not passed the idempotence gate. Like RuboCop, Sonicop can reach the 200-pass limit, report an infinite loop and write partial corrections that change again on the next invocation. This is not reported as successful convergence; see [the measured limits](CONFORMANCE.md#autocorrect-and-safety).

[CONFORMANCE.md](CONFORMANCE.md) records the corpus commits, reference versions, exact offense fields, measured counts and limitations. The [cop conformance table](#cop-conformance) is a separate measurement: all 609 cops fired and matched exactly on the measured inputs. This does not establish agreement on every Ruby program or configuration.

## Performance

Measured on 2026-10-15 in a [dedicated GitHub Actions run](https://github.com/owayo/sonicop/actions/runs/38730861317) with Ruby 4.0.7 (`RUBYOPT=--yjit`), RuboCop 1.91.0 and Sonicop revision `2ae1ec7`. Hardware: AMD EPYC 7763 64-Core Processor (4 logical CPUs), x86_64-pc-linux, runner image `20261012.334.1`. [Conformance](#conformance) remains pinned to RuboCop 1.89.0. The newer gem has 613 cops; Sonicop implements 609. Each defaults to 394 enabled cops, with one different name. Timing excludes RuboCop's `Lint/CopDirectiveSyntax` and Sonicop's `Style/DoubleCopDisableDirective`, leaving **393 shared cop names**.

Input copies live outside project directories. CLI probes require target Ruby 3.3 for Rails (`RAILS_VERSION=8.2.0.alpha`), 4.0 for Mastodon and 2.7 for the other corpora on both tools. Both use `--force-default-config` and the **JSON formatter**. Every timed run checks complete inspection, target path sets and all offense fields against its own baseline. Ruby's two files that abort the reference are omitted from both timing copies: 7,464 of 7,466 targets are measured. Times are **fastest / median**, in seconds, from two runs in alternating tool order. Parallel runs use all available logical CPUs, `--cache true` and a fresh real-path cache root. Single-process runs use `--cache false`, with `--no-parallel` on Sonicop. Every timed run exited 1 after reporting offenses. Cache-file counts are retained in the results; no warm-cache timings are claimed.

| Corpus | Revision | Files | RuboCop offenses | Sonicop offenses | RuboCop parallel | Sonicop parallel | RuboCop single | Sonicop single |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| rails/rails | `62b5458` | 3,551 | 167,899 | 167,760 | 57.54 / 57.60 | 10.92 / 10.94 | 101.77 / 101.83 | 26.03 / 26.05 |
| rubocop/rubocop | `f009b33` | 1,765 | 5,501 | 5,766 | 31.42 / 31.44 | 3.76 / 3.76 | 51.26 / 51.46 | 9.13 / 9.14 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,292 | 15,286 | 27.67 / 27.74 | 4.01 / 4.01 | 43.55 / 43.69 | 9.73 / 9.75 |
| Homebrew/brew | `38ee325` | 2,179 | 49,926 | 49,326 | 30.24 / 30.34 | 3.57 / 3.59 | 47.51 / 47.63 | 8.40 / 8.41 |
| ruby/ruby | `3349f41` | 7,464 | 763,186 | 761,186 | 131.83 / 132.02 | 22.45 / 22.78 | 248.03 / 248.42 | 49.91 / 50.05 |

The newer reference performs different work, as the offense counts show. These times do not establish equivalent-work speed ratios or improvement over a previous Sonicop build. Single-process CPU times use the same fastest / median format:

| Corpus | RuboCop user CPU | Sonicop user CPU | RuboCop system CPU | Sonicop system CPU |
| --- | ---: | ---: | ---: | ---: |
| rails/rails | 98.59 / 98.59 | 25.81 / 25.83 | 3.11 / 3.17 | 0.22 / 0.22 |
| rubocop/rubocop | 48.97 / 49.15 | 9.08 / 9.10 | 2.25 / 2.26 | 0.04 / 0.05 |
| mastodon/mastodon | 41.28 / 41.41 | 9.65 / 9.67 | 2.22 / 2.23 | 0.08 / 0.09 |
| Homebrew/brew | 44.79 / 44.91 | 8.32 / 8.32 | 2.65 / 2.66 | 0.08 / 0.08 |
| ruby/ruby | 242.65 / 243.12 | 49.16 / 49.30 | 5.12 / 5.20 | 0.74 / 0.75 |

One-minute load averages at the first and last timed run, and the highest recorded sample:

| Corpus | First | Last | Highest sample |
| --- | ---: | ---: | ---: |
| rails/rails | 1.59 | 1.06 | 4.34 |
| rubocop/rubocop | 1.23 | 1.31 | 3.55 |
| mastodon/mastodon | 1.13 | 1.41 | 3.58 |
| Homebrew/brew | 1.72 | 1.51 | 4.44 |
| ruby/ruby | 1.80 | 1.00 | 5.18 |

The [recorded results](docs/benchmarks/2026-10-15.json) include cop counts, diagnostic differences, exit codes, CPU time, cache-file counts and runtime versions. Reproduce with `make benchmark`; see [development notes](docs/development.md#corpus-gates-and-benchmarks).

## Development

<!-- standard:dev:start -->
Requires [mise](https://mise.jdx.dev/). Tool versions are pinned in `mise.toml`.

```bash
make setup   # Install the toolchain (mise) and dependencies
make ci      # Run the same checks as CI (no changes)
```

| Command | Description |
|---|---|
| `make setup` | Install the toolchain (mise) and dependencies |
| `make build` | Build a debug binary |
| `make release` | Build a release binary |
| `make run` | Run the debug binary (arguments via ARGS="...") |
| `make test` | Run the tests |
| `make lint` | Run clippy with warnings as errors |
| `make fmt` | Format the code (rewrites files) |
| `make fmt-check` | Check the formatting (no changes) |
| `make check` | Run fmt-check and lint (no changes) |
| `make ci` | Run the same checks as CI (no changes) |
| `make install` | Install the release binary to INSTALL_PATH (default /usr/local/bin) |
| `make uninstall` | Remove the binary from INSTALL_PATH |
| `make clean` | Remove build artifacts |

Run `make` to list every target. Releases are published from GitHub Actions (**Actions → Release → Run workflow**).
<!-- standard:dev:end -->

The Rakefile holds only the gem packaging and version tasks, which `make gem`, `make gem-platform` and `make version-sync` call, and `Cargo.toml` is the single source of truth for the version. Adding a cop, the files generated from upstream, and the other targets: [docs/development.md](docs/development.md).

## License

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->

The bundled RuboCop default configuration and parser dependency retain their upstream notices in [NOTICE](NOTICE) and [`licenses/`](licenses/).
