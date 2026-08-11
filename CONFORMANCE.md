# RuboCop conformance

Snapshot date: 2026-08-10  
Reference: RuboCop 1.89.0  
Corpus: the complete 1,759-file RuboCop source tree  
Configuration: RuboCop 1.89.0 built-in defaults (`--force-default-config`)

The bundled `config/default.yml` records the upstream version it was vendored from in its header.
Re-fetch it with `scripts/sync_default_yml.sh <rubocop-version>`.

The comparison covers every cop Sonicop implemented at the snapshot date, listed verbatim in the
`cops` variable of the command below. Each JSON offense is normalized by path, cop, line, column,
length, severity, correctability, and message. Run `sonicop --show-cops` for the current set.

| Result | Count |
|---|---:|
| Reference offense locations | 4,052 |
| Sonicop offense locations | 4,052 |
| Matching locations | 4,052 |
| False positives | **0** |
| False negatives | **0** |
| Location recall | **100%** |
| Message/severity/correctability differences at matching locations | **0** |

All offenses from the implemented cops match RuboCop by location, message, severity, and
correctability. This includes RuboCop's source-line indexing for metric cops, line-length
correctability and exemptions, directive lexing around heredocs, and target-version syntax errors.

Run the harness against a local RuboCop and corpus. It defaults to `rubocop` on `PATH`
(`gem install rubocop`) as the reference and `./target/release/sonicop` (`make release`) as the
candidate, so both only need `--reference` / `--candidate` when they live elsewhere — for example
`--reference "bundle exec rubocop"` inside a bundled project:

```bash
cops="Layout/EmptyLineAfterMagicComment,Layout/EndOfLine,Layout/LineLength,Layout/SpaceAfterComma,Layout/SpaceAroundOperators,Layout/SpaceInsideParens,Layout/TrailingEmptyLines,Layout/TrailingWhitespace,Lint/DuplicateMethods,Lint/Syntax,Lint/UnusedBlockArgument,Lint/UselessAssignment,Metrics/BlockLength,Metrics/ClassLength,Metrics/MethodLength,Metrics/ModuleLength,Metrics/ParameterLists,Naming/AsciiIdentifiers,Naming/ConstantName,Naming/MethodName,Naming/VariableName,Security/Eval,Style/FrozenStringLiteralComment,Style/HashSyntax,Style/NumericLiterals,Style/RedundantReturn,Style/Semicolon,Style/StringLiterals"

scripts/conformance_diff.sh \
  --force-default-config \
  --cop "${cops}" \
  -- /path/to/rubocop
```

`Lint/Syntax` is also run independently over the corpus. The latest owayo tree-sitter grammar
accepts the modern syntax in all 1,759 files, while Sonicop resolves Ruby 2.6 from the corpus
gemspec and applies a post-parse syntax feature gate. Its four fatal offenses, including the legacy
parser recovery diagnostics, exactly match RuboCop.

## Limits of this measurement

The 100% above is a property of *this corpus*, not a general accuracy claim. RuboCop's own source
tree is RuboCop-clean and stylistically uniform, so entire classes of input never occur in it and
the comparison cannot exercise them. A cop that is never triggered contributes nothing to the
match count, and its absence is indistinguishable from agreement.

Two implemented cops are confirmed to produce **zero offenses on both sides** over this corpus, so
this run says nothing about them:

| Cop | Offenses in this corpus | Verified elsewhere |
|---|---:|---|
| `Layout/TrailingEmptyLines` | 0 | Diverges on line, column, length, and message for a file ending in a blank line |
| `Naming/AsciiIdentifiers` | 0 | Rails contains exactly two occurrences; both diverge on message casing |

Running the same harness over application code rather than linter source surfaces divergences this
corpus does not. Treat a clean run on any single corpus as evidence about that corpus.

Two properties of the harness itself also bound what a run can tell you:

- Only the cops passed to `--cop` are compared. Cops that RuboCop reports and Sonicop has not
  implemented show up as false negatives, which is expected rather than a defect.
- RuboCop emits **only** `Lint/Syntax` for a file it considers fatally unparseable and suppresses
  every other cop for that file, while Sonicop keeps inspecting it. Whenever the two disagree
  about syntax validity — most easily triggered by a `TargetRubyVersion` the source does not
  satisfy — the false-positive count is dominated by that single behaviour rather than by cop
  logic. Check the reference output for `"severity": "fatal"` before reading a large false-positive
  count as a cop problem.

Closing these gaps properly means migrating RuboCop's own spec suite, which exercises each cop
against inputs written to break it. Until that lands, this document records what was measured, not
an upper bound on what could differ.

## Rails configuration and scale

Rails source snapshot: `adf307b03b4241cbc0ed3821faf3b153ca6cd5cd` (Rails 8.2.0.alpha).
The reference process uses RuboCop 1.89.0 with the five plugins declared by Rails:
rubocop-minitest 0.40.0, rubocop-packaging 0.6.0, rubocop-performance 1.26.1,
rubocop-rails 2.36.0, and rubocop-md 2.0.4.

This corpus exercises `AllCops/DisabledByDefault`, external plugin cops, recursive excludes,
hidden paths, and the inherited `guides/.rubocop.yml` configuration.

| Compatibility check | RuboCop | Sonicop | Result |
|---|---:|---:|---|
| Ruby target files from repository root | 3,453 | 3,453 | Exact paths and order |
| Enabled cops for `activerecord/lib/active_record.rb` | 107 | 107 | Exact names and order |
| Enabled cops for a file under `guides/` | 102 | 102 | Exact names and order |
| Full-tree false positives | — | 0 | Pass |

RuboCop also discovers 75 Markdown targets through rubocop-md. Markdown extraction is outside
Sonicop's current parser scope, so those files are removed before comparing `-L` output.

The full 3,453-Ruby-file offense comparison reports 10 reference locations and no Sonicop
locations: zero false positives and 10 visible false negatives. The missing locations are three
`Style/RedundantPercentQ`, two `Lint/RedundantCopDisableDirective`, two
`Layout/IndentationWidth`, two unsupported `Lint/DuplicateMethods` cases, and one
`Lint/Debugger`. Artifacts: `/tmp/sonicop-conformance-rails-full-2`.

Timings live in the README's Performance section. An earlier measurement here reported roughly
12.5x on the 402-file `activerecord/lib` subset "with cache disabled", which overstated the
difference: RuboCop turns `--parallel` off when it is combined with `--cache false`, so that figure
compared a parallel Sonicop against a single-process RuboCop. Restricting RuboCop to the cops
Sonicop implements and letting it run in parallel puts the gap at about 3.8x over the whole Rails
tree.
