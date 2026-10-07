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

**All 609 RuboCop 1.89 cop names are registered**, matched name for name against the upstream registry. The three cops that require a project index currently support only the index-disabled path; their indexed behavior is not implemented. That includes the 159 shipped as `Enabled: pending` and the 56 shipped as `Enabled: false`, which a default run does not reach on either side — name them with `--only` or switch them on in a configuration, exactly as with RuboCop. Unknown cop names still fail validation unless `--ignore-unrecognized-cops` is supplied.

### Cop conformance

Measured on 2026-10-08 against RuboCop 1.89.0: **42,435 inputs in 1,545 configuration groups** from the upstream spec capture, preserving each group's `TargetRubyVersion`, filename type and Ruby configuration values. A cop counts as an **exact match** only when all offense records agree, including start and end positions, length, message, severity, correctability and duplicate records. Comparisons are kept separate for each run.

Six other configuration groups (161 inputs) are unmeasured: RuboCop's CLI rejects their null `EnforcedStyle` settings, although its spec helpers accept them. The table describes the measurable inputs, not every possible configuration.

<!-- conformance:start -->
| Department | Cops | Exercised | Exact match | Diverging |
|---|---:|---:|---:|---:|
| Bundler | 7 | 7 | **7 ✓** | 0 |
| Gemspec | 10 | 10 | **10 ✓** | 0 |
| Layout | 100 | 100 | **100 ✓** | 0 |
| Lint | 157 | 154 | 153 | 1 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 1 | **1 ✓** | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 7 | **7 ✓** | 0 |
| Style | 298 | 298 | 296 | 2 |
| **Total** | **609** | **606** | **603** | **3** |
<!-- conformance:end -->

**Read the *Exercised* column first.** Only 606 cops fire in this sweep. `Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod` require a project index, which Sonicop does not yet provide. They are not counted as exact matches. The three measured cops still differing are `Lint/Syntax`, `Style/FormatStringToken` and `Style/RedundantArgument`.

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

The cache flags are honoured rather than merely parsed. Sonicop keeps a result cache of its own and serves a stored report for a file whose size, modification time and permission bits have not moved since it was inspected.

- Caching is on by default. `--cache false` turns it off, as does `AllCops/MaxFilesInCache: 0` in a configuration file.
- `--cache-root DIR` chooses where it lives. Without it the root is `$XDG_CACHE_HOME/sonicop`, or `~/Library/Caches/sonicop` on macOS, or `~/.cache/sonicop`. `--cache-root` cannot be combined with `--cache false`.
- `AllCops/MaxFilesInCache` bounds how many reports are kept, defaulting to RuboCop's 20,000.
- Autocorrect runs, `--stdin`, `--profile` and `--memory` neither read nor write it.
- It is not shared with RuboCop's cache: the formats are unrelated, and an entry is only served back to a build of Sonicop identical to the one that wrote it.

Cop settings are the silent case. A setting sonicop does not implement is ignored without any warning, and so is a setting whose name is simply misspelled. **A run that reports no offenses is therefore not evidence that a setting took effect**, because an ignored setting and a clean file produce the same output. [Non-default settings](docs/cop-conformance.md#non-default-settings) says which values have been measured.

Cop *names* are checked: an unrecognised cop in a configuration file stops the run with an error. It is the settings inside a recognised cop that pass unvalidated.

## Conformance

Sonicop registers **609 / 609** cop names from its RuboCop 1.89.0 specification; project-index behavior is still missing for three of them. On five pinned Ruby projects (18,251 target files), both tools discover the same path sets. Their default-config JSON reports match offense by offense on RuboCop's own tree (5,766 offenses), Rails (167,760) and Mastodon (15,286). Homebrew differs only in `Lint/Syntax` recovery positions: both reject the same 569 files. Ruby still has parser and cop differences; it is not counted as exact.

Autocorrect is also compared on separate copies of each project. Some differences are deliberate safety decisions: RuboCop can write invalid Ruby, lose a value through assignment order, or turn a local-variable read into a `NameError`. Each known difference is pinned by corrected-file hashes in [the divergence manifest](tests/conformance/known_divergences.yml). For the pinned trees, `-a` has 0 new differences and `-A` has 0 new differences among the four corpora RuboCop can finish; Ruby's reference aborts before a complete correction tree exists. The corrected trees are byte-identical on Homebrew, while the other known differences remain visible in the gate.

[CONFORMANCE.md](CONFORMANCE.md) records the corpus commits, reference versions, exact offense fields, measured counts and limitations. The [cop conformance table](#cop-conformance) is a separate measurement: 606 cops fired, 603 matched exactly, three differed and three required unavailable project-index behavior. The goal of 609 exercised and exact cops remains open.

## Performance

Measured on 2026-10-08 with Ruby 4.0.7 (YJIT disabled) and RuboCop 1.91.0 on an Apple M2 (8 cores), using Sonicop commit `ca1de57`. [Conformance](#conformance) uses RuboCop 1.89.0. The newer gem has 613 cops; Sonicop implements the 609 from 1.89.0. Each defaults to 394 enabled cops, with one different name. RuboCop excludes `Lint/CopDirectiveSyntax` and Sonicop excludes `Style/DoubleCopDisableDirective` for timing, leaving **393 shared cop names**.

Both use `--force-default-config`; target path sets and complete inspected-file counts were checked in JSON runs. Ruby's two files that abort RuboCop are omitted from both timing copies, covering 7,464 of its 7,466 targets. Every time below is **fastest / median**, in seconds, from two cold runs. Parallel RuboCop uses `--parallel --cache true` and a fresh real-path cache root; parallel Sonicop also uses a fresh root. Cache files were verified after every parallel run. Single-process runs disable caches, with `--no-parallel` on Sonicop. All timing runs exited with status 1 after reporting offenses.

| Corpus | Revision | Files | RuboCop offenses | Sonicop offenses | RuboCop parallel | Sonicop parallel | RuboCop single | Sonicop single |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| rails/rails | `62b5458` | 3,551 | 167,899 | 167,760 | 31.91 / 32.15 | 7.01 / 7.56 | 307.00 / 308.05 | 59.37 / 59.72 |
| rubocop/rubocop | `f009b33` | 1,765 | 5,501 | 5,766 | 16.13 / 17.79 | 2.15 / 2.39 | 162.09 / 167.21 | 21.22 / 23.30 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,292 | 15,286 | 15.84 / 15.99 | 2.41 / 2.44 | 119.31 / 120.57 | 22.62 / 22.97 |
| Homebrew/brew | `38ee325` | 2,179 | 49,926 | 49,326 | 16.12 / 16.22 | 2.37 / 2.40 | 127.63 / 129.47 | 19.39 / 20.09 |
| ruby/ruby | `3349f41` | 7,464 | 763,186 | 761,186 | 79.96 / 81.06 | 14.93 / 16.36 | 692.99 / 698.78 | 96.85 / 102.63 |

Offense counts were collected under the same cop selection and configuration. Newer cop behavior changes the work performed, so these times do not establish equivalent-work speed ratios. Single-process CPU times below help distinguish engine cost from scheduling delays; parallel wall time also includes scheduling effects.

| Corpus | RuboCop user CPU | Sonicop user CPU | RuboCop system CPU | Sonicop system CPU |
| --- | --- | ---: | ---: | ---: |
| rails/rails | 162.10 / 162.52 | 31.83 / 32.09 | 31.94 / 32.05 | 6.43 / 6.45 |
| rubocop/rubocop | 88.46 / 91.19 | 11.41 / 12.31 | 14.13 / 15.11 | 2.26 / 2.35 |
| mastodon/mastodon | 64.74 / 65.36 | 11.97 / 12.02 | 12.59 / 12.59 | 2.61 / 2.63 |
| Homebrew/brew | 68.65 / 69.28 | 10.36 / 10.57 | 13.60 / 13.65 | 2.18 / 2.23 |
| ruby/ruby | 379.22 / 383.38 | 53.25 / 55.57 | 66.76 / 67.12 | 10.21 / 10.95 |

The machine remained busy with other OS activity. These are times observed under that load; they do not demonstrate an optimization against an earlier build. The recorded load averages are:

| Corpus | Before | After | Highest recorded sample |
| --- | --- | ---: | ---: |
| rails/rails | 14.69 | 17.18 | 26.37 |
| rubocop/rubocop | 17.18 | 16.07 | 33.48 |
| mastodon/mastodon | 16.07 | 16.17 | 25.26 |
| Homebrew/brew | 16.17 | 19.35 | 34.06 |
| ruby/ruby | 16.98 | 17.21 | 48.19 |

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
