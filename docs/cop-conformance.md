# Cop conformance

The [README table](../README.md#cop-conformance) was regenerated on 2026-10-22 from full-field CLI comparisons against RuboCop 1.89.0 on Ruby 4.0.8, with rubydex 0.4.1 as the project-index reference. It records **609 cop names, 609 exercised cops and 609 exact matches**. Registration, coverage and correctness are separate measurements. Real-project results are in [CONFORMANCE.md](../CONFORMANCE.md).

## Inputs and comparison

The upstream spec capture supplies 42,594 inputs in 1,549 configuration groups for the 609-name registry. Both CLIs completed 42,433 inputs in 1,543 groups. The reference rejects six groups (161 inputs) whose null `EnforcedStyle` values are accepted by the upstream spec helper; those groups are unmeasured. Another 174 project-index cases and two filename-dependent cases use the committed outputs recorded from the actual reference CLI. Each case is a separate run, and index cases can contain several source files.

Each group retains its `TargetRubyVersion`, file type and Ruby configuration value types, including Regexp, Symbol and binary strings. Gemspec and Bundler inputs use their required filename types. `Bundler/GemFilename` and `Lint/RequireRelativeSelfPath` also need explicit filename cases. The three directive cops that cannot be compared with `--only` are run with all cops, then only their own offense records are compared.

Before comparison, every run must have matching nonempty target path sets and complete inspection counts. Records are compared as multisets within each run, including all location fields, length, message, severity and correctability. A duplicate record is not discarded, and differences in separate runs cannot cancel each other out.

The committed fixture test is another gate: 37,085 recorded cases across 593 reached cops. Its passing result does not replace the broader CLI measurement above.

## Project-index coverage and limits

`Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod` now use a shared project index when `AllCops: UseProjectIndex: true`. The additional cases cover all three cops and the indexed branches of 12 existing cops, including inherited methods, reopened classes, visibility, documentation and references from another file. Cache invalidation and optional bundled gem sources have separate tests.

**Cops = Exercised = Exact match = 609** holds for the measured inputs. Real-code `Lint/Syntax` recovery differences and other Ruby corpus residue remain open; passing these constructed cases does not prove agreement on every program. Adding ordinary application code alone cannot exercise project-index behavior or the many disabled cops.

## Non-default settings

The grouped CLI sweep preserves each captured configuration rather than replacing it with defaults. In this pass it found and led to fixes for Ruby Regexp tags, legacy ignored-method patterns, numeric pattern anchoring, method-name exemptions, Ruby truthiness, namespace wrapper styles and hash value omission in parenthesized `yield` calls.

YAML scalar types are also tested directly. Ruby's YAML 1.1 plain `yes`/`no`/`on`/`off` values differ from quoted strings; serializing a test configuration with a YAML 1.2 writer can silently remove the quotes and change the test input. Such tests write raw YAML to disk. Quoted values, explicit tags, anchors, Unicode and invalid input have positive and negative cases.

Reproduce the table with `scripts/conformance_table.rb` and a JSON specification containing the 609-name registry and paired reports for each run. It fails if any cop is unexercised or differs, and rejects incomplete or malformed reports. Table counts are generated from reports rather than entered by hand.

## Real-source update on 2026-10-24

The [Windows corpus run](maintenance/2026-10-24.md) refreshes real-source diagnostic, correction and timing evidence. It confirms the specification registry names/groups and retains Ruby residuals. It does not re-run or change the 2026-10-22 generated 609-cop sweep above. Newer-reference JSON fields and changed cop sets are recorded separately from specification conformance.
