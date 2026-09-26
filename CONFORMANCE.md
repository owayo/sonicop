# RuboCop conformance

Measured: 2026-09-27. The behavioral reference is **RuboCop 1.89.0 on Ruby 4.0.6**; Sonicop's vendored `config/default.yml` comes from that release. Both tools used `--force-default-config`. The current upstream gem may have more cops; a newer release is used only for the separate speed comparison in the README.

## Scope and method

Sonicop implements **609 / 609** RuboCop 1.89.0 cops, with the same names and no extras: 394 enabled, 159 pending, and 56 disabled by default. These are registry counts. A default run exercises only the enabled group. The separate [cop conformance table](README.md#cop-conformance) records which cops actually fired on upstream spec cases; a cop that stayed silent is never counted as an exact match.

The five public corpora below have **18,251** target files in total. RuboCop and Sonicop resolve identical target *path sets* on all five. Each corpus is pinned to a commit; numbers from another revision are not interchangeable. For each common offense position `(cop, path, line, column)`, we also compare end line, end column, length, message, severity and correctability. Equal total offense counts alone do not establish a match.

| Corpus | Revision | Target files | RuboCop offenses | Sonicop offenses |
|---|---|---:|---:|---:|
| rubocop/rubocop | `f009b33` | 1,765 | 5,766 | 5,766 |
| rails/rails | `62b5458` | 3,551 | 167,760 | 167,760 |
| ruby/ruby | `3349f41` | 7,466 | 761,578 | 761,188 |
| Homebrew/brew | `38ee325` | 2,179 | 49,920 | 49,326 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,286 | 15,286 |

Ruby's reference was assembled in chunks. RuboCop raises an invalid UTF-8 error on `test/ruby/test_regexp.rb`, and its JSON formatter cannot serialize an offense from `test/ruby/test_file_exhaustive.rb`. Those **two files are excluded from the Ruby offense comparison**, which covers 7,464 files; both tools still discover the same 7,466 target paths. A single RuboCop invocation can emit well-formed but incomplete JSON after a crash. Every comparison checks that its inspected count reached its target count or records the excluded files explicitly.

## Detection results

| Corpus | Sonicop-only positions | RuboCop-only positions | Shared-position field differences | Interpretation |
|---|---:|---:|---:|---|
| rubocop/rubocop | 0 | 0 | 0 | Exact |
| rails/rails | 0 | 0 | 0 | Exact |
| ruby/ruby | 168 | 558 | 4 | Parser recovery, grammar and remaining cop differences |
| Homebrew/brew | 330 | 924 | 0 | All positions differ in `Lint/Syntax` |
| mastodon/mastodon | 0 | 0 | 0 | Exact |

Homebrew has the same **569 syntax-error files** on both sides. The different `Lint/Syntax` positions are produced after error recovery; they do not represent different accepted-file sets there. Ruby has both parser and cop residue. Its four shared-position differences are two `correctable` flags, one range length, and one syntax message. These differences remain open and are not described as exact matches.

A separate CRLF probe used the pinned `rubocop/rubocop` revision `f009b33`, converted 1,748 Ruby files in a copy, and inspected the same 1,765 target paths on both sides. The **7,514 offense records** matched in full, including range, message, severity and correctability; neither side had an extra record.

The default configuration is the required baseline. Native project configurations were also attempted. Their plugin requirements stop RuboCop before inspection on four corpora, so no native-config agreement is claimed for them. For the Ruby corpus, a nested native-config run over `spec/ruby` inspected 4,435 files and matched offense by offense after a directory-discovery fix. Directory target discovery uses the configuration of the starting directory; the cop configuration is resolved for each inspected file.

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

For Ruby, both `-a` and `-A` exit unsuccessfully in RuboCop 1.89.0 on the source that also blocks its lint run. There is no complete corrected reference tree, so full-corpus autocorrect equality is unmeasured rather than assumed.

On the four public corpora with complete corrected trees, a second Sonicop `-A` pass changed **0 files**. Syntax checks of files changed by either tool found **0 candidate-only invalid Ruby files**; RuboCop alone produced invalid Ruby in 1 file of its own tree, 2 Rails files and 3 Mastodon files. A partial Sonicop `-a` run over Ruby was stopped during a large conversion table; among the 206 changed Ruby files that were valid before correction, **0 became invalid**. This partial result does not stand in for a complete Ruby autocorrect comparison.

## How to reproduce

```bash
# 固定した各コーパスで、先に対象パスの集合を比較する。
rubocop --force-default-config --cache false -L
sonicop --force-default-config -L

# 検査数を確認してから、JSON の位置と全項目を比較する。
rubocop --force-default-config --cache false -f json . > rubocop.json
sonicop --force-default-config --cache false -f json . > sonicop.json
```

Use the `migrate-rubocop` measurement scripts for the actual comparison. Ruby needs their chunked `rcfull.py` reference. `fullfmt.sh` checks a clean source and compares copies, including both safe and unsafe correction. RuboCop 1.89.0 is the oracle for these conformance results even when a newer RuboCop is installed for timing.

The real-code corpora cannot cover every branch: some cops are disabled by default, and some syntax shapes do not occur. The upstream spec fixture suite supplies deliberately constructed cases; README's *Exercised* column keeps coverage separate from correctness. The goal remains **Cops = Exercised = Exact match** in every department. The last complete spec sweep (2026-08-28) exercised 609 and matched 579 exactly, so that goal is still open. The table must be regenerated by `scripts/conformance_table.rb` from measured JSON, never edited by hand.
