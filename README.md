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

**All 609 RuboCop 1.89 cops are implemented**, matched name for name against the upstream registry. That includes the 159 shipped as `Enabled: pending` and the 56 shipped as `Enabled: false`, which a default run does not reach on either side — name them with `--only` or switch them on in a configuration, exactly as with RuboCop. Unknown cop names still fail validation unless `--ignore-unrecognized-cops` is supplied.

### Cop conformance

All 609 cops switched on, on both sides, over the 37,491 cases RuboCop's own specs supply, each run at the `TargetRubyVersion` its spec asked for. A cop counts as an **exact match** only when its offenses agree completely: every position, message, severity and correctable flag, with nothing extra on either side.

The table below is the last complete spec sweep, measured on 2026-08-28. Later targeted fixes have not been folded into these counts; the table is not a claim that every cop is now exact.

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

**Read the *Exercised* column first.** A cop nothing here made fire contributes neither way — its silence is indistinguishable from agreement, so it would be counted as agreement without ever being asked. **Every one of the 609 fires here**, which is what makes the *Exact match* column mean what it says.

How every cop is reached, a separate direct oracle sweep over the upstream specs, and the measurement of non-default settings: [docs/cop-conformance.md](docs/cop-conformance.md).

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

The cache flags are honoured rather than merely parsed. Sonicop keeps a result cache of its own and serves a stored report for a file whose size, modification time and permission bits have not moved since it was inspected.

- Caching is on by default. `--cache false` turns it off, as does `AllCops/MaxFilesInCache: 0` in a configuration file.
- `--cache-root DIR` chooses where it lives. Without it the root is `$XDG_CACHE_HOME/sonicop`, or `~/Library/Caches/sonicop` on macOS, or `~/.cache/sonicop`. `--cache-root` cannot be combined with `--cache false`.
- `AllCops/MaxFilesInCache` bounds how many reports are kept, defaulting to RuboCop's 20,000.
- Autocorrect runs, `--stdin`, `--profile` and `--memory` neither read nor write it.
- It is not shared with RuboCop's cache: the formats are unrelated, and an entry is only served back to a build of Sonicop identical to the one that wrote it.

Cop settings are the silent case. A setting sonicop does not implement is ignored without any warning, and so is a setting whose name is simply misspelled. **A run that reports no offenses is therefore not evidence that a setting took effect**, because an ignored setting and a clean file produce the same output. [Non-default settings](docs/cop-conformance.md#non-default-settings) says which values have been measured.

Cop *names* are checked: an unrecognised cop in a configuration file stops the run with an error. It is the settings inside a recognised cop that pass unvalidated.

## Conformance

Sonicop implements **609 / 609** cops from its RuboCop 1.89.0 specification. On five pinned Ruby projects (18,251 target files), both tools discover the same path sets. Their default-config JSON reports match offense by offense on RuboCop's own tree (5,766 offenses), Rails (167,760) and Mastodon (15,286). Homebrew differs only in `Lint/Syntax` recovery positions: both reject the same 569 files. Ruby still has parser and cop differences; it is not counted as exact.

Autocorrect is also compared on separate copies of each project. Some differences are deliberate safety decisions: RuboCop can write invalid Ruby, lose a value through assignment order, or turn a local-variable read into a `NameError`. Each known difference is pinned by corrected-file hashes in [the divergence manifest](tests/conformance/known_divergences.yml). For the pinned trees, `-a` has 0 new differences and `-A` has 0 new differences among the four corpora RuboCop can finish; Ruby's reference aborts before a complete correction tree exists. The corrected trees are byte-identical on Homebrew, while the other known differences remain visible in the gate.

[CONFORMANCE.md](CONFORMANCE.md) records the corpus commits, reference versions, exact offense fields, measured counts and limitations. The [cop conformance table](#cop-conformance) uses upstream spec cases to exercise all 609 cops; its 579 exact matches are a separate measurement and the remaining 30 are still open.

## Performance

Measured on 2026-09-27 with the latest stable Ruby 4.0.7 and RuboCop 1.91.0 on an Apple M2 (8 cores). This is a speed comparison with the newer gem; [conformance](#conformance) still uses RuboCop 1.89.0 as the behavioral specification. RuboCop 1.91.0 has 613 cops and Sonicop implements the 609 from 1.89.0. Each has 394 default-enabled cops, but one name differs in each set. For timing, RuboCop excludes `Lint/CopDirectiveSyntax` and Sonicop excludes `Style/DoubleCopDisableDirective`, leaving the **same 393 default-enabled cop names**.

Both use `--force-default-config`, and their target path sets were checked before timing. Ruby's two files that make RuboCop abort are omitted **from both tools** in the timing copy; that row covers 7,464 of its 7,466 targets. Times are the fastest of two cold runs per condition. Parallel RuboCop uses `--parallel --cache true` with a fresh real-path cache root for each run; parallel Sonicop uses its default parallel execution with a fresh root. Single-process runs use `--cache false` on both sides and `--no-parallel` on Sonicop. Exit status and inspected-file counts were checked for every run.

| Corpus | Revision | Files | RuboCop offenses | Sonicop offenses | RuboCop parallel | Sonicop parallel | RuboCop single | Sonicop single |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| rubocop/rubocop | `f009b33` | 1,765 | 5,501 | 5,766 | 17.02 s | 2.25 s | 158.62 s | 21.22 s |
| mastodon/mastodon | `fad3685` | 3,290 | 15,292 | 15,286 | 16.45 s | 2.42 s | 125.86 s | 22.36 s |
| Homebrew/brew | `38ee325` | 2,179 | 49,926 | 49,326 | 15.71 s | 2.37 s | 132.17 s | 20.27 s |
| rails/rails | `62b5458` | 3,551 | 167,899 | 167,760 | 32.84 s | 7.34 s | 300.27 s | 60.17 s |
| ruby/ruby | `3349f41` | 7,464 | 763,186 | 761,189 | 82.64 s | 16.64 s | 776.44 s | 113.71 s |

The offense columns are measured under these exact timing conditions. The newer RuboCop may have changed cop behavior since the 1.89.0 conformance reference, so equal cop names alone do not guarantee equal work. Read the offense counts alongside the times, and use the single-process column to judge engine cost: parallel wall time also depends on machine scheduling. The load average before and after each corpus is recorded below; high load can make absolute times and ratios move.

| Corpus | Before | After | Highest recorded sample |
|---|---:|---:|---:|
| rubocop/rubocop | 16.38 | 19.28 | 43.68 |
| mastodon/mastodon | 19.28 | 18.65 | 26.13 |
| Homebrew/brew | 18.65 | 18.69 | 22.94 |
| rails/rails | 18.69 | 14.91 | 45.20 |
| ruby/ruby | 14.91 | 18.51 | 45.85 |

The machine remained busy with other OS activity. These are observed times under that load, not an idle-machine lower bound.

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
