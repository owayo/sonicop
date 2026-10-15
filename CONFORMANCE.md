# RuboCop conformance

Measured: 2026-10-15. The behavioral reference is **RuboCop 1.89.0 on Ruby 4.0.7**; Sonicop's vendored `config/default.yml` comes from that release. Both tools used `--force-default-config`. The current upstream gem may have more cops; a newer release is used only for the separate speed comparison in the README.

The Rust and Ruby suite passed 2,312 Rust tests and 77 Ruby test runs; 15 measurement-tool tests also passed (**2,404 total, including 58 test functions added for this maintenance**). The new project-index test binary also repeats four shared support tests. The upstream fixture suite checks 37,085 recorded cases inside one Rust test; those case counts are separate from the test-function total.

## Scope and method

Sonicop implements **609 / 609** RuboCop 1.89.0 cop names, with the same names and no extras: 394 enabled, 159 pending, and 56 disabled by default. These are registry counts. A default run exercises only the enabled group. `Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod` now support the project-index path. The separate [cop conformance table](README.md#cop-conformance) records which cops actually fired on upstream spec and constructed cases; a cop that stayed silent is never counted as an exact match.

The five public corpora below have **18,251** target files in total. RuboCop and Sonicop resolve identical target *path sets* on all five. Each corpus is pinned to a commit; numbers from another revision are not interchangeable. For each common offense position `(cop, path, line, column)`, we also compare end line, end column, length, message, severity and correctability. Offense records are compared as multisets, so duplicates are retained. Equal total offense counts alone do not establish a match.

| Corpus | Revision | Target files | RuboCop offenses | Sonicop offenses |
|---|---|---:|---:|---:|
| rubocop/rubocop | `f009b33` | 1,765 | 5,766 | 5,766 |
| rails/rails | `62b5458` | 3,551 | 167,760 | 167,760 |
| ruby/ruby | `3349f41` | 7,466 | 761,578 | 761,185 |
| Homebrew/brew | `38ee325` | 2,179 | 49,920 | 49,326 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,286 | 15,286 |

Ruby's reference was assembled in chunks. RuboCop raises an invalid UTF-8 error on `test/ruby/test_regexp.rb`, and its JSON formatter cannot serialize an offense from `test/ruby/test_file_exhaustive.rb`. Those **two files are excluded from the Ruby offense comparison**, which covers 7,464 files; both tools still discover the same 7,466 target paths. A single RuboCop invocation can emit well-formed but incomplete JSON after a crash. Every comparison checks that its inspected count reached its target count or records the excluded files explicitly.

## Detection results

| Corpus | Sonicop-only positions | RuboCop-only positions | Shared-position field differences | Interpretation |
|---|---:|---:|---:|---|
| rubocop/rubocop | 0 | 0 | 0 | Exact |
| rails/rails | 0 | 0 | 0 | Exact |
| ruby/ruby | 165 | 558 | 4 | Parser recovery, grammar and remaining cop differences |
| Homebrew/brew | 330 | 924 | 0 | All positions differ in `Lint/Syntax` |
| mastodon/mastodon | 0 | 0 | 0 | Exact |

Homebrew has the same **569 syntax-error files** on both sides. The different `Lint/Syntax` positions are produced after error recovery; they do not represent different accepted-file sets there. Ruby has both parser and cop residue. Ruby's four shared-position differences are two `correctable` flags, one range length, and one syntax message. The full-record multiset has 169 Sonicop-only records and 562 RuboCop-only records after including those four differing positions. These differences remain open and are not described as exact matches.

Command-style receivers in spaced index assignments are now diagnosed at their assignment operator, including constants and argumentless calls. Recovery follows the reference's scope: ordinary blocks and a missing right-hand side stop the remaining diagnostics, while recoverable method, class, conditional and lambda bodies still report later errors. These constructed cases are tested against actual RuboCop output; the broader parser recovery gap remains open.

A separate CRLF probe prepared 1,749 `.rb` files and converted **1,748 files with line endings** in a copy of the pinned `rubocop/rubocop` revision `f009b33`. Both tools inspected the same 1,765 target paths. The full-record comparison covers **7,514 offenses**, including range ends, length, message, severity and correctability. The current candidate matches all 7,514 full records. On 2026-10-08 this gate caught a 405-record end-column regression before publication; counts alone would have missed it.

The default configuration is the required baseline. Native project configurations were also attempted. Their missing plugin requirements stop RuboCop before inspection on four corpora: `rubocop-performance` on RuboCop, `rubocop-minitest` on Rails, `rubocop-capybara` on Mastodon and `rubocop-md` on Homebrew. No native-config agreement is claimed for those runs. For the Ruby corpus, both tools discovered the same 7,466 native-config target paths. A nested native-config run over `spec/ruby` inspected 4,435 files and matched all four offenses, including their fields. Directory target discovery uses the configuration of the starting directory; the cop configuration is resolved for each inspected file.

## Autocorrect and safety

`-a` and `-A` rewrite files, so each tool runs on its own copy of the same clean tree. Corrected trees are compared byte for byte, including non-`.rb` files. The known differences are pinned by file and output hash in `tests/conformance/known_divergences.yml`; a changed hash or an unregistered difference fails the gate.

| Corpus | `-a` corrected-tree differences | `-A` corrected-tree differences |
|---|---:|---:|
| rubocop/rubocop | 0 | 1 known safety difference |
| rails/rails | 4 known safety differences | 3 known safety differences |
| mastodon/mastodon | 3 known safety differences | 3 known safety differences |
| Homebrew/brew | 0 | 0 |
| ruby/ruby | Reference aborts | Reference aborts |

RuboCop's own autocorrect can produce invalid Ruby in cases that Sonicop's syntax guard leaves untouched. Other recorded exceptions prevent an assignment-order change that loses the old buffer and a guard-clause rewrite that turns a local variable read into a method call raising `NameError`. Sonicop also preserves the source encoding on write-back; RuboCop can write UTF-8 bytes under a non-UTF-8 magic comment. Each intentional incompatibility needs a minimal disk-level test and an entry in the divergence manifest. These safety exceptions are monitored differences, never omitted files.

The final correction guard also parses output as a complete script. Diagnostic parsing remains unchanged; its partial-script mode can accept a top-level `break` that a file cannot execute. The supplementary check uses the configured target, with Ruby 3.3 as Prism's lower bound, and only judges new failures when the original parses under the same setting. It restores CRLF before comparison and parses the original bytes against the exact bytes prepared for write-back. Source declarations, aliases and BOMs are interpreted by Prism, so encoding errors introduced by removing interpolation are detected in US-ASCII, Shift_JIS and other encodings. Diagnostic parsing retains its UTF-8 setting. This is not proof of compatibility with pre-3.3 grammar or general semantic preservation. A separate narrow check prevents removal of standalone `exec` launcher lines before a polyglot script's Ruby entry; arbitrary shell expressions and changes inside a retained launcher line are outside that check.

Constructed regressions cover required parentheses around multiple assignments, block-parameter reflow, `defined?` modifier loops, removed post-loops containing `break`, the polyglot entry, empty lambda parameters, empty regexp character classes and operator calls reinterpreted as commands. RuboCop's actual destructive bytes and Claude's safety decisions are registered in the divergence manifest. `Lint/ArrayLiteralInRegexp` diverges only under `-A`; safe correction is checked separately. The Sonicop heredoc-delimiter defect is fixed to match the reference's valid correction, rather than hidden by a rollback. Single-quoted and `%q` byte-escape text inside interpolation is also corrected to the reference's string value; these escapes must not be decoded as double-quoted non-UTF-8 bytes. Disk-level regressions check the resulting bytes under both correction modes.

For Ruby, both `-a` and `-A` exit unsuccessfully in RuboCop 1.89.0 on the source that also blocks its lint run. There is no complete corrected reference tree, so full-corpus autocorrect equality is unmeasured rather than assumed.

In the [2026-10-15 correction gate](https://github.com/owayo/sonicop/actions/runs/38730852034), all four public corpora with complete corrected trees passed both modes. A second Sonicop `-A` pass changed **0 files** in those four corpora. Syntax checks of changed `.rb` files from either tool found **0 candidate-only invalid Ruby files**; RuboCop alone produced invalid Ruby in 1 file of its own tree, 2 Rails files and 3 Mastodon files.

A separate candidate-only Ruby run inspected all 7,466 files. Syntax checks covered 6,812 changed `.rb` files under `-a` and 7,227 under `-A`, with no newly invalid candidate files. However, its second `-A` changed files, so the overall workflow failed the idempotence gate. This is not an all-corpus safety pass or a complete reference comparison.

That failure exposed repeated frozen-comment insertion after a BOM-prefixed encoding declaration. The cop now follows the reference's token boundary while preserving the original line for magic-comment interpretation. Thirty-six recorded reference cases cover BOMs, shebangs, empty input, indentation and multiple declarations; disk tests also cover CRLF and both correction modes.

The Ruby-wide idempotence condition remains unmet independently of that fix. `benchmark/app_lc_fizzbuzz.rb` reaches the 200-pass correction limit; both Sonicop and the reference write partial output, and both change it on a second invocation. Their corrected bytes also differ, so this is not recorded as an exact match. A separate 205-level nesting control confirms the shared limit behavior: both write 86,261 bytes on exit 1 with an infinite-loop diagnostic, then converge to the same 86,311 bytes on the next invocation. The limit is retained for compatibility. No general semantic safety or successful convergence is claimed for a run that reaches it.

## How to reproduce

```bash
# 固定した各コーパスで、先に対象パスの集合を比較する。
rubocop --force-default-config --cache false -L
sonicop --force-default-config -L

# 検査数を確認してから、JSON の位置と全項目を比較する。
rubocop --force-default-config --cache false -f json . > rubocop.json
sonicop --force-default-config --cache false -f json . > sonicop.json
```

`make corpus-check` and the **Corpus conformance** workflow run the correction and safety gates using the tools in `scripts/conformance/`. They preserve each linter's exit code. An exit 2 is accepted for comparison only when every reported rollback path is registered for that corpus and correction mode; the corrected-tree gate must still match both output hashes exactly. Other failures remain incomplete measurements. Measurement-tool I/O errors use exit 2, distinct from a classified syntax difference. Ruby's lint comparison needs the chunked `rcfull.py` reference from the `migrate-rubocop` measurement tools. RuboCop 1.89.0 is the oracle even when a newer RuboCop is installed for timing.

The real-code corpora cannot cover every branch: some cops are disabled by default, and some syntax shapes do not occur. The upstream spec fixture suite supplies deliberately constructed cases; README's *Exercised* column keeps coverage separate from correctness. The 2026-10-15 full-field CLI sweep covered 42,433 measurable inputs across 1,543 configuration groups, plus 174 project-index cases and two filename-dependent cases. **Cops = Exercised = Exact match = 609** now holds in every department for these measured inputs. Six other groups (161 inputs) were rejected by the reference CLI and are unmeasured. This result does not resolve the real-code corpus differences above. The table is regenerated by `scripts/conformance_table.rb` from measured JSON, never edited by hand.
