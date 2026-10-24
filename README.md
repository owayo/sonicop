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

It uses the actively maintained [owayo/tree-sitter-ruby](https://github.com/owayo/tree-sitter-ruby) grammar, inspects files in parallel, and preserves hard links when applying corrections. Its RuboCop 1.89-compatible CLI and JSON output fit existing editor and CI integrations with minimal changes.

## Features

Sonicop implements cops in the Bundler, Gemspec, Layout, Lint, Metrics, Migration, Naming, Security, and Style departments. The binary itself is the authoritative list:

```bash
# Every recognized cop and its implementation status
sonicop --show-cops
```

**All 609 RuboCop 1.89 cop names are implemented**, matched name for name against the upstream registry, including project-index behavior. The registry includes the 159 shipped as `Enabled: pending` and the 56 shipped as `Enabled: false`, which a default run does not reach on either side — name them with `--only` or switch them on in a configuration, exactly as with RuboCop. Unknown cop names still fail validation unless `--ignore-unrecognized-cops` is supplied.

### Cop conformance

Measured on 2026-10-22 against RuboCop 1.89.0: **42,433 inputs in 1,543 configuration groups** from the upstream spec capture, plus **174 project-index cases and two filename-dependent cases**. The sweep preserves each group's `TargetRubyVersion`, filename type and Ruby configuration values. A cop counts as an **exact match** only when all offense records agree, including start and end positions, length, message, severity, correctability and duplicate records. Comparisons are kept separate for each run.

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

Sonicop reuses a stored report when the digest of the source content, file size, modification time, permissions, build, configuration and selected cops still match.

- Caching is on by default. `--cache false` turns it off, as does `AllCops/MaxFilesInCache: 0` in a configuration file.
- `--cache-root DIR` chooses where it lives. Without it the root is `$XDG_CACHE_HOME/sonicop`, or `~/Library/Caches/sonicop` on macOS, or `~/.cache/sonicop`. `--cache-root` cannot be combined with `--cache false`.
- `AllCops/MaxFilesInCache` bounds how many reports are kept, defaulting to RuboCop's 20,000.
- Autocorrect runs, `--stdin`, `--profile` and `--memory` neither read nor write it.
- It is not shared with RuboCop's cache: the formats are unrelated, and an entry is only served back to a build of Sonicop identical to the one that wrote it.

Cop settings are the silent case. A setting sonicop does not implement is ignored without any warning, and so is a setting whose name is simply misspelled. **A run that reports no offenses is therefore not evidence that a setting took effect**, because an ignored setting and a clean file produce the same output. [Non-default settings](docs/cop-conformance.md#non-default-settings) says which values have been measured.

Cop *names* are checked: an unrecognised cop in a configuration file stops the run with an error. It is the settings inside a recognised cop that pass unvalidated.

## Conformance

The 2026-10-24 Windows run used five existing public corpora with **18,670 target files**. The behavioral specification remains **RuboCop 1.89.0**, measured on Ruby 4.0.7 / Parser 3.3.12.0. All 609 cop names and their enabled 394 / pending 159 / disabled 56 groups agree. Default-config comparisons retain every offense field, duplicates and file order:

| Corpus | Files | RuboCop 1.89.0 | Sonicop | Missing | Extra | Field differences |
| --- | --- | --- | --- | --- | --- | --- |
| rails/rails | 3,588 | 176,909 | 176,909 | 0 | 0 | 0 |
| rubocop/rubocop | 1,807 | 7,957 | 7,957 | 0 | 0 | 0 |
| ruby/ruby | 7,522 / 7,524 | 776,604 | 776,679 | 3 | 78 | 3 |
| Homebrew/brew | 2,397 | 60,799 | 60,799 | 0 | 0 | 0 |
| mastodon/mastodon | 3,354 | 19,145 | 19,145 | 0 | 0 | 0 |

Ruby's full reference exits 2. Two individually confirmed failing files are excluded from the verified chunked comparison. Four corpora match completely; Ruby's remaining differences affect seven files. Registry counts, real-source agreement and the separate [2026-10-22 per-cop sweep](#cop-conformance) measure different things.

Diagnostics and correction bytes were also measured separately against current RuboCop 1.92.1. Its added JSON `correction` property remains a strict difference, with differences in the older shared fields reported separately. Independent correction copies produced the following results. Byte differences count files; this newer-version matrix has not passed compatibility P.

| Corpus | -a byte differences | -A byte differences | Candidate exit -a / -A | Second -A changes |
| --- | --- | --- | --- | --- |
| rails/rails | 3,212 | 3,301 | 2 / 2 | 0 |
| rubocop/rubocop | 177 | 236 | 1 / 1 | 0 |
| ruby/ruby | unavailable | unavailable | 2 / 2 | 1 |
| Homebrew/brew | 1,582 | 1,608 | 1 / 1 | 0 |
| mastodon/mastodon | 1,349 | 1,752 | 2 / 2 | 0 |

Syntax checks of completed writes found no candidate-only invalid files. Rails, Mastodon and Ruby retain exit 2 for rejected writes or other errors, so there is no overall safety S pass. Ruby's corrected reference is unavailable, and changes made by its second candidate pass are reported separately. Differences that disappear only after CRLF normalization are not treated as unregistered conformance passes.

See the [current conditions, residual differences and raw data](docs/maintenance/2026-10-24.md), [specification and historical evidence](CONFORMANCE.md), and [intentional-divergence manifest](tests/conformance/known_divergences.yml).

An additional copy converts the RuboCop corpus's `.rb` files to CRLF. Its 1,807 files and 6,167 offenses match specification 1.89.0 in every field. Correction results remain separate in the [detailed record](docs/maintenance/2026-10-24.md#crlf-corpus-and-comparison-controls).

## Performance

Measured on 2026-10-24 with Windows 11 build 26300.9711 and an AMD Ryzen 7 5700X (8 cores, 16 logical CPUs). The runtime used Ruby **4.0.9**, RuboCop **1.92.1**, Parser **3.3.13.0** and rubocop-ast **1.50.0**. Latest Prism 2.0.0 conflicts with rubocop-ast's `~>1.7` requirement; the loaded version is **1.8.2**. Measurements with every component at its latest release remain incomplete. These results use the available combination. This Windows Ruby provides neither YJIT nor RuboCop parallel execution.

Both tools receive the same external configuration (`NewCops: disable` and the verified target version). Caching is enabled: cold uses a fresh real-path root; warm reuses the corresponding cold run's populated cache. Two trials alternate tool order and reproduce each tool's own complete diagnostic baseline. Values are median wall seconds. The ratio is RuboCop ÷ Sonicop; **different diagnostics mean it is neither an equivalent-work speedup nor improvement over a previous build**.

| Corpus | Files | Cache | RuboCop single(s) | Sonicop single(s) | Time ratio | Sonicop parallel(s) |
| --- | --- | --- | --- | --- | --- | --- |
| rails/rails | 3,588 | cold | 214.52 | 41.25 | 5.20 | 6.71 |
| rails/rails | 3,588 | warm | 16.19 | 1.36 | 11.95 | 0.83 |
| rubocop/rubocop | 1,807 | cold | 98.31 | 15.41 | 6.38 | 1.74 |
| rubocop/rubocop | 1,807 | warm | 4.98 | 0.31 | 16.18 | 0.14 |
| mastodon/mastodon | 3,354 | cold | 75.07 | 11.03 | 6.81 | 1.55 |
| mastodon/mastodon | 3,354 | warm | 10.62 | 0.66 | 16.14 | 0.34 |
| Homebrew/brew | 2,397 | cold | 78.93 | 13.23 | 5.96 | 1.92 |
| Homebrew/brew | 2,397 | warm | 7.60 | 0.60 | 12.67 | 0.33 |
| ruby/ruby | 7,522 | cold | 411.07 | 68.96 | 5.96 | 17.35 |
| ruby/ruby | 7,522 | warm | 27.58 | 5.62 | 4.90 | 4.43 |

Each version enables 394 cops, with 393 shared names; these runs retain each version's enabled set. Target versions are Rails 3.3, Mastodon 4.0 and 2.7 for the others. Ruby timing copies omit only the two reference-aborting files, leaving 7,522 targets. Discovery times, fastest/maximum trials, child-inclusive user/system CPU, host activity, cache-file counts and exits are in the [raw results](docs/benchmarks/2026-10-24-windows.json) and [reproduction notes](docs/maintenance/2026-10-24.md). The [2026-10-22 Linux results](docs/benchmarks/2026-10-22.json) are historical measurements with different inputs and hardware.

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
