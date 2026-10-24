# RuboCop conformance

The current real-source record is **2026-10-24 on Windows**. The behavioral specification remains **RuboCop 1.89.0**. Newest-release measurements use a separate runtime and do not redefine that specification. The [full conditions and residuals](docs/maintenance/2026-10-24.md) and [raw records](docs/maintenance/2026-10-24.json) identify every source revision and binary.

## Scope and method

All 609 registered names match RuboCop 1.89.0, including enabled 394, pending 159 and disabled 56. Default-config inspection covers **18,670 discovered targets** across five unchanged public checkouts. Both tools discover the same paths in the same order. Each comparison checks positive, complete inspection counts and offense multisets, retaining location, length, message, severity, correctability, duplicates and additional JSON fields.

The specification oracle uses Ruby 4.0.7 / Parser 3.3.12.0 / Prism 1.8.1. The newer oracle uses Ruby 4.0.9 / RuboCop 1.92.1 / Parser 3.3.13.0 / rubocop-ast 1.50.0 / loaded Prism 1.8.2. Latest Prism 2.0.0 cannot satisfy the AST dependency; an all-latest-component measurement remains incomplete. The newest versions and actual loaded gems are recorded separately from the pinned build environment.

## Detection results

| Corpus | Files | RuboCop 1.89.0 | Sonicop | Missing | Extra | Field differences |
| --- | --- | --- | --- | --- | --- | --- |
| rails/rails | 3,588 | 176,909 | 176,909 | 0 | 0 | 0 |
| rubocop/rubocop | 1,807 | 7,957 | 7,957 | 0 | 0 | 0 |
| ruby/ruby | 7,522 / 7,524 | 776,604 | 776,679 | 3 | 78 | 3 |
| Homebrew/brew | 2,397 | 60,799 | 60,799 | 0 | 0 | 0 |
| mastodon/mastodon | 3,354 | 19,145 | 19,145 | 0 | 0 | 0 |

Ruby's full reference exits 2. Two failing paths are isolated individually, and only completed chunks contribute to its 7,522-file comparison: `test/ruby/test_file_exhaustive.rb` and `test/ruby/test_regexp.rb` are excluded. The residue affects seven files: six reference-only records and 81 candidate-only records, including three missing positions, 78 extra positions and three same-position field differences. Four other corpora match every field and file order.

Newer-reference differences are reported separately, including its added `correction` JSON property and differences in shared fields. Public project-config runs are excluded where plugins are missing or native custom/new cop names are unsupported; user-global config is never substituted. See the dated record for names and counts.

The **2026-10-22 cop sweep** remains a separate, dated measurement: registration 609 / exercised 609 / exact 609, from 42,433 inputs in 1,543 groups plus 174 project-index and two filename cases. This maintenance does not claim a fresh full 609-cop sweep; the generated [cop table](README.md#cop-conformance) is unchanged.

An additional RuboCop corpus copy converts `.rb` files to CRLF and preserves other file types. All 1,807 files /6,167 offenses match the 1.89.0 oracle in every field and file order. Its correction bytes, syntax and second-pass results are recorded separately in the dated report.

## Autocorrect and safety

The current matrix compares independent copies against RuboCop 1.92.1. Differences count files whose actual corrected bytes differ:

| Corpus | -a byte differences | -A byte differences | Candidate exit -a / -A | Second -A changes |
| --- | --- | --- | --- | --- |
| rails/rails | 3,212 | 3,301 | 2 / 2 | 0 |
| rubocop/rubocop | 177 | 236 | 1 / 1 | 0 |
| ruby/ruby | unavailable | unavailable | 2 / 2 | 1 |
| Homebrew/brew | 1,582 | 1,608 | 1 / 1 | 0 |
| mastodon/mastodon | 1,349 | 1,752 | 2 / 2 | 0 |

This matrix has not passed compatibility P. CRLF-normalization-only differences and changed upstream corrections remain visible. They are not automatically accepted as registered intentional differences. Ruby's full safe reference aborts; unsafe reference failures are demonstrated on its two individual leaves, while full unsafe reference output remains unmeasured. Candidate-only syntax and idempotence evidence are kept separate from reference equality.

Syntax checks of completed writes find no candidate-only newly invalid Ruby. The reference alone introduces invalid syntax in one RuboCop file under `-A` and one Mastodon file in each mode. Rails, Mastodon and Ruby candidate runs retain exit 2, so this is not an overall safety S pass. A 200-pass partial output is not convergence, and any second-pass changes remain an idempotence failure.

The existing [intentional-divergence manifest](tests/conformance/known_divergences.yml) continues to monitor unsafe reference corrections, local-variable binding, encoding and line-ending preservation. No new divergence was declared merely to remove a measured difference. The [2026-10-22 archive](docs/conformance/2026-10-22.md#autocorrect-and-safety) retains earlier hash-pinned safety evidence; its different inputs and successful subsets are not reused as the current result.

## Maintenance regressions

The current changes fix Windows diff-path parsing in the correction gate, displayed-path directory ordering, and source-gem Cargo path quoting. Each has a before-fix failure and an after-fix regression. [PR80](https://github.com/owayo/sonicop/pull/80), [PR82](https://github.com/owayo/sonicop/pull/82) and [PR84](https://github.com/owayo/sonicop/pull/84) passed eight CI checks each and were squash-merged. Final local `make ci` passes through source-gem installation and execution.

The suite has 2,466 unique Rust functions (2,498 executions because four shared support functions repeat across binaries), 83 Ruby functions (319 assertions, 15 platform skips), and 18 Python tests: **2,567 unique functions**, including those skips. This run adds three Python functions and one Ruby function and expands one Rust CLI test. The 37,085 fixture inputs run inside one Rust test and are not added to the function count.

The parser pair is **tree-sitter 0.27.1 with grammar 26.10.103**; this pair resolves the older percent-literal regression that blocked 0.27.1 with the preceding grammar. The dependency updater installed no new packages under the current age/pinning policy. General parser, write-back, cache and configuration contracts remain documented in [AGENTS.md](AGENTS.md).

## Regexp parser semantics

The recorded parser/target matrix and binding safety decisions remain in [regexp-semantics.md](docs/regexp-semantics.md), the divergence manifest and the [dated conformance archive](docs/conformance/2026-10-22.md#regexp-parser-semantics). Current corpus totals do not expand those configured-target or runtime guarantees.

## Performance

The current [Windows timing record](docs/benchmarks/2026-10-24-windows.json) separates single/parallel, cold/warm and discovery conditions. RuboCop parallel and YJIT are unavailable in this Windows Ruby. Ratios against the newer reference describe different diagnostic work and are not before/after optimization claims. See [README](README.md#performance) for medians and the dated record for every trial, CPU time, cache count, exit and host-activity counter. The[2026-10-22 Linux results](docs/benchmarks/2026-10-22.json) remain historical.
