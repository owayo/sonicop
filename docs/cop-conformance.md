# Cop conformance

The [Cop conformance](../README.md#cop-conformance) table in the README switches all 609 cops on, on both sides, over the 37,491 cases RuboCop's own specs supply, and counts a cop as an exact match only when its offenses agree completely. This page explains how every cop is reached, what a separate direct sweep found, and how non-default settings are measured. The comparison over five real Ruby projects is in [CONFORMANCE.md](../CONFORMANCE.md).

## Reaching every cop

Every one of the 609 cops fires on the recorded cases, but three of them took a run of their own. `Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod` report nothing without a `rubydex` project index, which needs the gem installed and `AllCops/UseProjectIndex` switched on — and `Lint/DeprecatedReference` needs more than that: the call has to sit inside a class inheriting the one whose method carries the `@deprecated` tag. Upstream's own specs open with an `expect_no_offenses` saying the cop is silent without the index, which is easy to read as "unreachable"; it is not.

Getting *Exact match* to 609 while reducing *Diverging* to zero is the current goal. More real Ruby does not get there: the 56 cops RuboCop ships disabled, and much of what it ships as pending, never fire in a plain run however large the tree. What does reach every one of them is the input its own specs supply — the cases recorded in `tests/fixtures/upstream_spec_capture.jsonl` touch **609 of 609 cops**, measured. Two things about running them are easy to get wrong, and both silently shrink the table rather than failing:

- **`TargetRubyVersion` is part of the input, not a global.** Pinning everything at 2.7 leaves `Style/ArrayIntersect`, `Naming/BlockForwarding`, `Style/ItBlockParameter` and eleven others unable to fire at all. Each case is run at the version its spec asked for.
- **The filename is what several cops inspect.** `Bundler/*` needs a `Gemfile`, `Gemspec/*` a `.gemspec`, and `Naming/FileName` reads the name itself. Writing every case as `.rb` had all 17 of those cops matching nothing.

## A direct oracle sweep

A separate direct oracle sweep on 2026-08-29 examined 11,506 cases extractable from the upstream cop specs. RuboCop could not read 226 of those inputs and crashed on one; among the measurable cases, Sonicop had **zero detection differences and zero correction differences**. This result is not folded into the table in the README: 51 cops had no directly extractable case, and three directive cops cannot be measured under `--only`, so the sweep does not prove that all 609 cops are exact. The direct sweep also uses neutral/default conditions rather than preserving every example's `TargetRubyVersion`. The four Ruby 3.4-sensitive cops changed in this pass were therefore compared separately at 3.4, where their messages and locations matched RuboCop exactly.

## Non-default settings

Configuration is measured separately, because a cop that only matches at its default value is half a cop. Every one of the 111 cops carrying an `Enforced*` setting was switched to a **non-default** value at once and the corpus re-run: **99.995% of 622,317 offenses match**, with 85 of the 96 cops that fired matching exactly. The residue is 10 cops of at most 17 offenses each, plus one where RuboCop crashes and sonicop does not; the list is in [CONFORMANCE.md](../CONFORMANCE.md).

Reproduce either table with `scripts/conformance_table.rb`.
