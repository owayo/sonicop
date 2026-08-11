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

Sonicop implements cops in the Layout, Lint, Metrics, Naming, Security, and Style departments.
The implemented set grows with each release, so the binary itself is the authoritative list:

```bash
# Every recognized cop and its implementation status
sonicop --show-cops
```

The bundled upstream configuration recognizes all 609 RuboCop 1.89 cops. Implemented cops run
normally; recognized but not-yet-implemented cops remain configuration-compatible and are
reported by `--debug`. Truly unknown cop names still fail validation unless
`--ignore-unrecognized-cops` is supplied.

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
parse-compatible. These flags are reported as compatibility no-ops when they request unsupported
functionality. Sonicop does not currently provide server transports, Ruby plugin execution, cache
reuse, custom Ruby cops, or cops outside the implemented set.

### Conformance

The implemented cops are verified against RuboCop 1.89.0 over five Ruby projects — RuboCop itself,
Rails, Ruby, Homebrew and Mastodon — totalling 18,242 files, with the upstream default
configuration on both sides. Every offense is compared by cop, path, line, column, last line, last
column, length, message, severity and correctability.

Three of the five match **exactly**: RuboCop's own tree (4,063 offenses), Rails (117,541) and
Mastodon (7,610), with no excess, no shortfall and no metadata differences. The target file lists
match exactly on all five. What remains is concentrated in `Lint/Syntax`, where RuboCop's LALR
parser recovers from an error and emits diagnostics a tree-sitter parse cannot reconstruct. Running
`-a` and `-A` across RuboCop's own tree produces byte-identical output.

See [CONFORMANCE.md](CONFORMANCE.md) for the commands, the per-corpus numbers, and the two ways a
measurement of this kind can mislead you.

### Performance

Measured on the Rails 8.2.0.alpha source tree. Both tools were given their own bundled default
configuration (`--force-default-config`), so neither reads Rails' `.rubocop.yml`, and both resolve
**the same 3,550 files**.

| Run | RuboCop 1.89.0 | Sonicop | Ratio |
|---|---:|---:|---:|
| The same 28 cops, parallel | 8.80 s | **3.85 s** | 2.3x |
| The same 28 cops, single process | 32.92 s | **15.49 s** | 2.1x |
| Every cop each tool enables by default | 20.58 s *(394 cops, parallel)* | **3.85 s** *(28 cops, parallel)* | — |

The last row is what the two commands do out of the box, and it is **not a like-for-like
comparison**: Sonicop implements 28 of RuboCop's 609 cops, so it is answering a much smaller
question. The first two rows restrict RuboCop to the same 28 cops, which is the honest measure of
the engines. Over those cops the two agree on **all 117,541 offenses** on this tree, so the speed is
not bought by skipping work.

Two details matter for reproducing this. RuboCop **silently turns `--parallel` off when combined
with `--cache false`**, so its parallel runs here use a cache directory that is deleted before each
run rather than disabled; timing it with `--cache false --parallel` measures a single process and
overstates the difference. RuboCop's default is a single process, while Sonicop is parallel unless
`--no-parallel` is passed.

```bash
# RuboCop, parallel, cold cache, restricted to the cops Sonicop implements
rubocop --force-default-config --cache true --cache-root "$(mktemp -d)" \
        --no-color --parallel --only "$COPS" -f quiet

# Sonicop
sonicop --force-default-config --format quiet
```

Machine: Apple M2 (8 cores), Ruby 4.0.6 with YJIT available, RubyGems-installed RuboCop 1.89.0.
Each figure is the fastest of two to three warmed runs.

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
