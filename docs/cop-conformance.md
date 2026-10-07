# Cop conformance

The [README table](../README.md#cop-conformance) was regenerated on 2026-10-08 from full-field CLI comparisons against RuboCop 1.89.0 on Ruby 4.0.7. It records **609 cop names, 606 exercised cops and 603 exact matches**. Registration, coverage and correctness are separate measurements. Real-project results are in [CONFORMANCE.md](../CONFORMANCE.md).

## Inputs and comparison

The upstream spec capture supplies 42,596 inputs in 1,551 configuration groups for the 609-name registry. Both CLIs completed 42,435 inputs in 1,545 groups. The reference rejects six groups (161 inputs) whose null `EnforcedStyle` values are accepted by the upstream spec helper; those groups are unmeasured.

Each group retains its `TargetRubyVersion`, file type and Ruby configuration value types, including Regexp, Symbol and binary strings. Gemspec and Bundler inputs use their required filename types. `Bundler/GemFilename` and `Lint/RequireRelativeSelfPath` also need explicit filename cases. The three directive cops that cannot be compared with `--only` are run with all cops, then only their own offense records are compared.

Before comparison, every run must have matching nonempty target path sets and complete inspection counts. Records are compared as multisets within each run, including all location fields, length, message, severity and correctability. A duplicate record is not discarded, and differences in separate runs cannot cancel each other out.

The committed fixture test is another gate: 37,085 recorded cases across 593 reached cops. Its passing result does not replace the broader CLI measurement above.

## Remaining gaps

Three cops need a project index: `Lint/DeprecatedReference`, `Lint/NameTypo` and `Lint/UnusedPrivateMethod`. Sonicop currently has no project-wide index, so only their index-disabled behavior is supported. They do not fire in this sweep and are not counted as exact matches. The earlier claim that all 609 fired was incorrect.

Three measurable cops still differ:

| Cop | Input condition | Remaining difference |
|---|---|---|
| `Lint/Syntax` | Ruby 3.3 targets and invalid syntax | Parser diagnostics and recovery |
| `Style/FormatStringToken` | A Ruby Symbol used for `Mode` | Two extra offenses |
| `Style/RedundantArgument` | A binary string configured as a method's default argument | One missing offense |

Getting **Cops = Exercised = Exact match** to 609 is still the goal. Adding ordinary application code cannot exercise project-index behavior or the many disabled cops by itself.

## Non-default settings

The grouped CLI sweep preserves each captured configuration rather than replacing it with defaults. In this pass it found and led to fixes for Ruby Regexp tags, legacy ignored-method patterns, numeric pattern anchoring, method-name exemptions, Ruby truthiness, namespace wrapper styles and hash value omission in parenthesized `yield` calls.

YAML scalar types are also tested directly. Ruby's YAML 1.1 plain `yes`/`no`/`on`/`off` values differ from quoted strings; serializing a test configuration with a YAML 1.2 writer can silently remove the quotes and change the test input. Such tests write raw YAML to disk. Quoted values, explicit tags, anchors, Unicode and invalid input have positive and negative cases.

Reproduce the table with `scripts/conformance_table.rb` and a JSON specification containing the 609-name registry and paired reports for each run. It fails if any cop is unexercised or differs, and rejects incomplete or malformed reports. Table counts are generated from reports rather than entered by hand.
