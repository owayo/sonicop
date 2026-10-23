# RuboCop conformance

Measured: 2026-10-22, Sonicop code revision `4408950`. The behavioral reference is **RuboCop 1.89.0 on Ruby 4.0.8**; Sonicop's vendored `config/default.yml` comes from that release. Both tools used `--force-default-config`. The current upstream gem may have more cops; a newer release is used only for the separate speed comparison in the README.

The validation suite contains 2,328 Rust tests, 82 Ruby test runs and 15 measurement-tool tests (**2,425 total, including 15 test functions added for this maintenance**). The project-index test binary also repeats four shared support tests. The upstream fixture suite checks 37,085 recorded cases inside one Rust test; those case counts are separate from the test-function total.

## Scope and method

Sonicop implements **609 / 609** RuboCop 1.89.0 cop names, with the same names and no extras: 394 enabled, 159 pending, and 56 disabled by default. These are registry counts. A default run exercises only the enabled group. `Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod` now support the project-index path. The separate [cop conformance table](README.md#cop-conformance) records which cops actually fired on upstream spec and constructed cases; a cop that stayed silent is never counted as an exact match.

CLI syntax probes confirmed the same effective target on both tools: Ruby 3.3 for Rails, 4.0 for Mastodon and 2.7 for the other three public corpora.

The five public corpora below have **18,251** target files in total. RuboCop and Sonicop resolve identical target *path sets* on all five. Each corpus is pinned to a commit; numbers from another revision are not interchangeable. For each common offense position `(cop, path, line, column)`, we also compare end line, end column, length, message, severity and correctability. Offense records are compared as multisets, so duplicates are retained. Equal total offense counts alone do not establish a match.

| Corpus | Revision | Target files | RuboCop offenses | Sonicop offenses |
|---|---|---:|---:|---:|
| rubocop/rubocop | `f009b33` | 1,765 | 5,766 | 5,766 |
| rails/rails | `62b5458` | 3,551 | 167,760 | 167,760 |
| ruby/ruby | `3349f41` | 7,466 | 761,579 | 761,186 |
| Homebrew/brew | `38ee325` | 2,179 | 49,920 | 49,326 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,286 | 15,286 |

Ruby's reference was assembled in chunks. RuboCop raises an invalid UTF-8 error on `test/ruby/test_regexp.rb`, and its JSON formatter cannot serialize an offense from `test/ruby/test_file_exhaustive.rb`. Those **two files are excluded from the Ruby offense comparison**, which covers 7,464 files; both tools still discover the same 7,466 target paths. The full Sonicop run reports 763,578 offenses; the table above uses only the 7,464 comparable files. A single RuboCop invocation can emit well-formed but incomplete JSON after a crash. Every comparison checks that its inspected count reached its target count or records the excluded files explicitly.

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

Nested heredocs have an intentional parser-dependent difference ([#20](https://github.com/owayo/sonicop/issues/20)). Whitequark/parser repeats an inner heredoc body inside the outer string; three levels repeat the innermost interpolation four times, and an outer string can reinterpret the contents of an inner single-quoted heredoc as interpolation. Sonicop keeps the physical Ruby evaluation order, which agrees with RuboCop's Prism engine for these inputs. For a conditional interpolation, whitequark reports ABC `<0, 6, 2>` and cyclomatic/perceived complexity `3`; Prism and Sonicop report `<0, 4, 1>` and `2`. The three-level control has ABC `<0, 12, 4>` versus `<0, 5, 1>`. These are recorded differences, not exact agreement or an unimplemented parser feature.

The same duplicated AST assignment makes whitequark's `Lint/UselessAssignment` delete a declaration that a later heredoc reads. Both reference correction modes write valid Ruby that subsequently raises `NameError`; the original runs successfully under Ruby 4.0.8. Sonicop retains the declaration and the original bytes and intentionally omits that false diagnosis. This decision preserves the physical binding; it is independent of the syntax guard, which cannot detect the runtime change. The reference records, including syntax checks and execution results, are in [nested_heredoc_divergences.json](tests/fixtures/nested_heredoc_divergences.json), with [recording provenance](tests/fixtures/nested_heredoc_divergences.json.PROVENANCE).

The recording covers 390 actual reference CLI runs and shares 32 identical outcomes: seven shapes, LF/CRLF, inspection/`-a`/`-A`, and Ruby 2.7/3.3/3.4 with default and explicit parser engines. The four-cop matrix has 342 runs; another 48 isolate `Lint/UselessAssignment` so remaining Metrics offenses cannot mask its exit-code difference. In the isolated reference, inspection exits 1 and both corrections exit 0 despite introducing `NameError`; Sonicop exits 0 and keeps the working source. Ruby 2.7 plus Prism is rejected. Default selects whitequark for 2.7 and Prism from 3.3; explicit whitequark still duplicates at 3.3/3.4. The conformance gate monitors 13 intentional differences against the whitequark recordings, and the CLI regression checks full diagnostic records and unchanged disk bytes against the physical-source Prism recordings. Runtime evidence uses the installed Ruby 4.0.8, rather than claiming execution on all configured target interpreters.

For Ruby, both `-a` and `-A` exit unsuccessfully in RuboCop 1.89.0 on the source that also blocks its lint run. There is no complete corrected reference tree, so full-corpus autocorrect equality is unmeasured rather than assumed.

In the [2026-10-22 correction gate](https://github.com/owayo/sonicop/actions/runs/39778325785), all four public corpora with complete corrected trees passed both modes. A second Sonicop `-A` pass changed **0 files** in those four corpora. Syntax checks of changed `.rb` files from either tool found **0 candidate-only invalid Ruby files**; RuboCop alone produced invalid Ruby in 1 file of its own tree, 2 Rails files and 3 Mastodon files.

A separate candidate-only Ruby run inspected all 7,466 files. Syntax checks covered 6,812 changed `.rb` files under `-a` and 7,227 under `-A`, with no newly invalid candidate files. Both candidate modes retained exit 2 for rejected writes. Under `-A`, stderr records 40 syntax-guard refusals and two encoding refusals; those source files are left unchanged. The syntax check only establishes that the completed writes did not introduce invalid Ruby. Its second `-A` changed one file, so the overall workflow failed the idempotence gate. This is not an all-corpus safety pass or a complete reference comparison.

An earlier failure exposed repeated frozen-comment insertion after a BOM-prefixed encoding declaration. The cop now follows the reference's token boundary while preserving the original line for magic-comment interpretation. Its 93 stable reference cases compare diagnostic fields, correction bytes and a second invocation. A further 57 cases compare bytes when both tools stop at the 200-pass limit. These include decoded string tokens, invalid UTF-8 token bytes, whitespace, line continuations and comments at EOF. A separate native token-order recording checks 9,426 combinations across whitequark targets 2.3/3.3, Prism 3.3 and both final-newline states. These bounded cases are not counted as successful convergence.

The final correction gate lists exactly one file changed by a second `-A`: `benchmark/app_lc_fizzbuzz.rb`. It reaches the 200-pass limit on both tools. Fixing the empty insertion anchor used by indentation corrections makes its first and second output match RuboCop byte for byte: 472,197 bytes (`6d7fbe0d0127eda5e5e4a3dec18a216b3b8d7d5d8018cc551d5407065c1e2ef0`) and 628,707 bytes (`5de828d63fd6b4e09a7ad2365f9528cd48c93bec2f3add57ada9438ddc9fd08e`). The isolated reference comparison exits 1 and reports an infinite loop on both invocations; the whole-corpus candidate runs retain exit 2 for other rejected writes. The Ruby-wide idempotence condition therefore remains unmet, even though this file's correction bytes now match. The gate continues to fail; the path is not excluded.

A separate 205-level nesting control confirms the shared limit behavior: both tools write 86,261 bytes on exit 1, then converge to the same 86,311 bytes on the next invocation. The limit is retained for compatibility. No general semantic safety or successful convergence is claimed for a run that reaches it.

## Maintenance regressions

Block comments include the newline following `=end` in diagnostic and correction ranges. Whitequark extends a comment closed at EOF without a final newline two characters beyond the source; Prism stops at EOF. Bounded text ranges are kept separate from reported ranges, and block-comment fences use normalized character positions mapped back to UTF-8 bytes, including CRLF and Japanese trailing text. Three reference internal-error cases are recorded under `block_comment_error_divergences` in `tests/conformance/known_divergences.yml`: the reference reports no offenses and leaves the source unchanged but exits 1 (`IndexError` for whole-comment replacement beyond EOF, or `ArgumentError` for the shortest Prism EOF block). Sonicop preserves those diagnostics and bytes while exiting 0. In the directive cases, removing the former false offense changes inspection from exit 1 to 0; this is an intentional CLI difference, not full compatibility. A CLI regression checks all three records with LF/CRLF and inspection/`-a`/`-A` (18 combinations).

Inspection preserves target order through parallel collection and cache hits. Sorting the inspected paths by components previously placed `foo/bar.rb` before `foo.rb` and overrode explicit argument order. The reference expands a directory in string order and keeps the order between explicit arguments. CLI runs over two files establish these expectations; the regression covers directory, forward and reverse arguments in cached/uncached and parallel/single modes, including repeated cache hits. The native Ruby run also matches the complete 4,435-file array order.

Correction merging now uses the reference Registry order already recorded in `config/autocorrect_support.json`. Alphabetical merging changed the result of nested `if` inside `unless !a`, including safe correction. Six actual RuboCop CLI recordings check the reports, exit codes and bytes on disk. They explicitly exclude `Layout/EndOfLine` on both tools so Windows' native newline setting does not change the recorded condition.

Loading a nested configuration now preserves the invocation's working directory for diagnostic paths. Windows reference probes compare absolute, long-name and relative stdin paths, each with and without an offense, plus file discovery. All seven paired reports match. Ruby's `File.realpath` can preserve a DOS short name that Rust expands; matching the reference does not require treating these spellings as interchangeable for display.

Write-back checks whether the original can be opened for writing before attempting replacement. Read-only source files therefore remain unchanged, while a writable file in a read-only directory can still be corrected. Hard links are retained on Windows as well as Unix. Unix ownership, extended attributes and macOS ACLs are preserved by writing through the existing file when replacement would lose them. Ordinary replacements use a temporary file; writes through existing files are not atomic. Tests inspect contents, links and attributes on disk, including an ownership check run as root in CI.

Cache hits also verify the source digest when file size and timestamps match. A same-length edit followed by timestamp restoration previously returned a stale offense; the regression checks edits in both directions. Source-gem installation obtains Cargo's actual target directory rather than assuming a directory inside the unpacked gem. Isolated real builds cover default, absolute, relative and configured target directories, followed by installation and execution.

Dependency updates retain tree-sitter **0.27.0**: version 0.27.1 adds an incorrect `tSTAR` diagnostic to valid percent-literal Ruby with the same grammar. The [upstream report](https://github.com/owayo/tree-sitter-ruby/issues/6) includes a minimal reproduction. Its regression rejects that additional diagnostic without requiring the existing false EOF diagnostic, so a complete parser fix can pass. `ruby-prism` remains at 1.9.0 because the pinned project-index dependency requires that native library version. These constraints are deliberate, rather than untested upgrades. The toolchain uses Rust 1.99.1 and Ruby 4.0.8 on Linux/macOS; Windows uses Ruby 4.0.7 because RubyInstaller 4.0.8 was unavailable during validation.

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

The real-code corpora cannot cover every branch: some cops are disabled by default, and some syntax shapes do not occur. The upstream spec fixture suite supplies deliberately constructed cases; README's *Exercised* column keeps coverage separate from correctness. The 2026-10-22 full-field CLI sweep covered 42,433 measurable inputs across 1,543 configuration groups, plus 174 project-index cases and two filename-dependent cases. **Cops = Exercised = Exact match = 609** now holds in every department for these measured inputs. Six other groups (161 inputs) were rejected by the reference CLI and are unmeasured. This result does not resolve the real-code corpus differences above. The table is regenerated by `scripts/conformance_table.rb` from measured JSON, never edited by hand.

The `--show-cops` listing is also parsed as YAML. Descriptions previously appeared without quoting, so a description beginning with a backtick made the entire listing invalid. The regression checks all 609 entries, the reference's `Lint/BigDecimalNew` description and configured strings through an output/configuration round trip. Ruby's YAML 1.1 boolean words remain strings; Unicode line separators and control characters are escaped so they are not normalized or rejected.
