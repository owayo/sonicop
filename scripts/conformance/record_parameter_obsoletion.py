"""固定版RuboCopの廃止parameter60API/420probeとCLI41だけを記録する。"""

import argparse
import json
import os
import re
import subprocess
import tempfile
from contextlib import contextmanager
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

CASE_IDS = (
    "fatal-alternative",
    "fatal-present-false",
    "fatal-present-null",
    "fatal-reason",
    "fatal-rule-order",
    "fatal-suppresses-warnings",
    "warning-present-false",
    "warning-present-null",
    "warning-alternatives",
    "warning-style",
    "warning-rule-order",
    "minimum-low27",
    "minimum-low31",
    "minimum-high32",
    "minimum-high40",
    "minimum-false32",
    "minimum-null32",
    "minimum-absent32",
    "minimum-environment32",
    "canonical-style",
    "absent-parameter",
    "namespace-before-error",
    "namespace-before-warning",
    "parent-warning",
    "parent-warning-replayed",
    "child-lower-target",
    "out-fatal-preserved",
    "out-warning",
    "force-default-error",
    "force-default-warning",
    "warning-force-color",
    "warning-multiple-force-color",
    "fatal-force-color",
    "ordinary-warning-control",
    "ordinary-warning-color-control",
    "warning-color-flag",
    "warning-force-no-color",
    "minimum-inferred-gemspec",
    "minimum-inferred-version-file",
    "minimum-gemspec-precedes-version-file",
    "force-default-warning-pending-enabled",
)

FATAL_IDS = frozenset(
    (
        "fatal-alternative",
        "fatal-present-false",
        "fatal-present-null",
        "fatal-reason",
        "fatal-rule-order",
        "fatal-suppresses-warnings",
        "namespace-before-error",
        "out-fatal-preserved",
        "force-default-error",
        "fatal-force-color",
    )
)

SOURCES = (
    "config/obsoletion.yml",
    "lib/rubocop/config_obsoletion.rb",
    "lib/rubocop/config_obsoletion/parameter_rule.rb",
    "lib/rubocop/config_obsoletion/changed_parameter.rb",
    "lib/rubocop/config_obsoletion/changed_enforced_styles.rb",
    "lib/rubocop/config_obsoletion/rule.rb",
    "lib/rubocop/config_validator.rb",
    "lib/rubocop/config_loader.rb",
    "LICENSE.txt",
)

RULE_IDS = (
    ("Layout/SpaceAroundOperators", "MultiSpaceAllowedForOperators"),
    ("Style/SpaceAroundOperators", "MultiSpaceAllowedForOperators"),
    ("Style/Encoding", "EnforcedStyle"),
    ("Style/Encoding", "SupportedStyles"),
    ("Style/Encoding", "AutoCorrectEncodingComment"),
    ("Style/IfUnlessModifier", "MaxLineLength"),
    ("Style/WhileUntilModifier", "MaxLineLength"),
    ("AllCops", "RunRailsCops"),
    ("Layout/CaseIndentation", "IndentWhenRelativeTo"),
    ("Layout/LineLength", "IgnoreCopDirectives"),
    ("Lint/BlockAlignment", "AlignWith"),
    ("Layout/BlockAlignment", "AlignWith"),
    ("Lint/EndAlignment", "AlignWith"),
    ("Layout/EndAlignment", "AlignWith"),
    ("Lint/DefEndAlignment", "AlignWith"),
    ("Layout/DefEndAlignment", "AlignWith"),
    ("Rails/UniqBeforePluck", "EnforcedMode"),
    ("Style/MethodCallWithArgsParentheses", "IgnoredMethodPatterns"),
    ("Performance/Count", "SafeMode"),
    ("Performance/Detect", "SafeMode"),
    ("Bundler/GemComment", "Whitelist"),
    ("Lint/SafeNavigationChain", "Whitelist"),
    ("Lint/SafeNavigationConsistency", "Whitelist"),
    ("Style/NestedParenthesizedCalls", "Whitelist"),
    ("Style/SafeNavigation", "Whitelist"),
    ("Style/TrivialAccessors", "Whitelist"),
    ("Style/IpAddresses", "Whitelist"),
    ("Naming/HeredocDelimiterNaming", "Blacklist"),
    ("Naming/PredicatePrefix", "NamePrefixBlacklist"),
    ("Naming/PredicatePrefix", "NameWhitelist"),
    ("Metrics/BlockLength", "ExcludedMethods"),
    ("Metrics/MethodLength", "ExcludedMethods"),
    ("Lint/Debugger", "DebuggerReceivers"),
    ("Layout/IndentationWidth", "IgnoredPatterns"),
    ("Layout/LineLength", "IgnoredPatterns"),
    ("Lint/UnreachableLoop", "IgnoredPatterns"),
    ("Naming/MethodName", "IgnoredPatterns"),
    ("Style/MethodCallWithArgsParentheses", "IgnoredPatterns"),
    ("Lint/AmbiguousBlockAssociation", "IgnoredMethods"),
    ("Lint/NumberConversion", "IgnoredMethods"),
    ("Metrics/AbcSize", "IgnoredMethods"),
    ("Metrics/BlockLength", "IgnoredMethods"),
    ("Metrics/CyclomaticComplexity", "IgnoredMethods"),
    ("Metrics/MethodLength", "IgnoredMethods"),
    ("Metrics/PerceivedComplexity", "IgnoredMethods"),
    ("Style/BlockDelimiters", "IgnoredMethods"),
    ("Style/ClassEqualityComparison", "IgnoredMethods"),
    ("Style/FormatStringToken", "IgnoredMethods"),
    ("Style/MethodCallWithArgsParentheses", "IgnoredMethods"),
    ("Style/MethodCallWithoutArgsParentheses", "IgnoredMethods"),
    ("Style/NumericPredicate", "IgnoredMethods"),
    ("Style/SymbolLiteral", "IgnoredMethods"),
    ("Style/ArgumentsForwarding", "AllowOnlyRestArgument"),
    ("Bundler/GemComment", "IgnoredGems"),
    ("Lint/NumberConversion", "IgnoredClasses"),
    ("Lint/MissingCopEnableDirective", "MaximumRangeSize"),
    ("Metrics/CollectionLiteralLength", "LengthThreshold"),
    ("Style/FetchEnvVar", "AllowedVars"),
    ("Layout/IndentationConsistency", "EnforcedStyle"),
    ("Style/EmptyClassDefinition", "EnforcedStyle"),
)

BASE = (
    "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
    "  SuggestExtensions: false\n  DisabledByDefault: true\n"
)

STYLE = "Style/EmptyClassDefinition:\n  EnforcedStyle: class_definition\n"

ARGS = "Style/ArgumentsForwarding:\n  AllowOnlyRestArgument: true\n"

ERROR = "Layout/CaseIndentation:\n  IndentWhenRelativeTo: case\n"

MULTI = STYLE + ARGS

API = (
    LOADER
    + r"""
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
sources = %w[config/obsoletion.yml lib/rubocop/config_obsoletion.rb
 lib/rubocop/config_obsoletion/parameter_rule.rb
 lib/rubocop/config_obsoletion/changed_parameter.rb
 lib/rubocop/config_obsoletion/changed_enforced_styles.rb
 lib/rubocop/config_obsoletion/rule.rb lib/rubocop/config_validator.rb
 lib/rubocop/config_loader.rb LICENSE.txt].to_h do |file|
  [file, Digest::SHA256.file(File.join(spec.full_gem_path, file)).hexdigest]
end
standard = RuboCop::ConfigObsoletion.new(RuboCop::Config.new({})).rules
rows = standard.each_with_index.filter_map do |rule, index|
  next unless rule.parameter_rule?
  metadata = rule.metadata
  matched = metadata.fetch('value', true)
  probes = [
    ['absent', false, nil, 3.2], ['false', true, false, 3.2],
    ['null', true, nil, 3.2], ['matching', true, matched, 3.2],
    ['other', true, 'other_style', 3.2],
    ['minimum_low', true, matched, 3.1], ['minimum_high', true, matched, 3.2]
  ].map do |name, present, value, target|
    hash = {'AllCops' => {'TargetRubyVersion' => target},
            rule.cop => present ? {rule.parameter => value} : {}}
    config = RuboCop::Config.new(hash, 'config.yml')
    actual = rule.class.new(config, rule.cop, rule.parameter, metadata)
    {id: name, present: present, value: value, target: target, violated: !!actual.violated?}
  end
  config = RuboCop::Config.new({'AllCops' => {'TargetRubyVersion' => 3.2}}, 'config.yml')
  actual = rule.class.new(config, rule.cop, rule.parameter, metadata)
  {index: index, kind: rule.class.name.split('::').last, cop: rule.cop,
   parameter: rule.parameter, warning: rule.warning?, metadata: metadata,
   message: actual.message, probes: probes}
end
puts JSON.generate({ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
 gems: %w[parser prism rubocop-ast rubocop rainbow].to_h do |name|
   [name, Gem.loaded_specs.fetch(name).version.to_s]
 end,
 sources: sources, rules: rows})
"""
)


def case_inputs():
    """存在・値・Ruby境界・集約とphaseだけの有限代表を固定する。

    Returns:
        list: 元のargs/files/envを保つ41条件。

    Raises:
        ValueError: 固定41件のID順が欠けた場合。

    """
    cases = []

    def add(name, tail, target="2.7", flags=(), env=None, extra=None, filename="config.yml"):
        config = (
            BASE.replace(
                "  TargetRubyVersion: 2.7\n",
                "" if target is None else f"  TargetRubyVersion: {target}\n",
            )
            + tail
        )
        cases.append(
            {
                "id": name,
                "files": {"Gemfile": "", "a.rb": "x=1\n", filename: config, **(extra or {})},
                "args": [
                    "--config",
                    filename,
                    "--cache",
                    "false",
                    "--format",
                    "json",
                    *flags,
                    "a.rb",
                ],
                "env": {"CLICOLOR_FORCE": "0", **(env or {})},
            }
        )

    add("fatal-alternative", ERROR)
    add("fatal-present-false", ERROR.replace("case", "false"))
    add("fatal-present-null", ERROR.replace("case", "null"))
    add("fatal-reason", "Layout/SpaceAroundOperators:\n  MultiSpaceAllowedForOperators: false\n")
    add(
        "fatal-rule-order",
        ERROR + "Layout/SpaceAroundOperators:\n  MultiSpaceAllowedForOperators: false\n",
    )
    add("fatal-suppresses-warnings", MULTI + ERROR + "Layout/TrailingBlankLines: {}\n", "3.2")
    add("warning-present-false", "Layout/LineLength:\n  IgnoreCopDirectives: false\n")
    add("warning-present-null", "Layout/LineLength:\n  IgnoreCopDirectives: null\n")
    add("warning-alternatives", "Metrics/BlockLength:\n  IgnoredMethods: [foo]\n")
    add("warning-style", STYLE)
    add("warning-rule-order", MULTI, "3.2")
    add("minimum-low27", ARGS)
    add("minimum-low31", ARGS, "3.1")
    add("minimum-high32", ARGS, "3.2")
    add("minimum-high40", ARGS, "4.0")
    add("minimum-false32", ARGS.replace("true", "false"), "3.2")
    add("minimum-null32", ARGS.replace("true", "null"), "3.2")
    add("minimum-absent32", "Style/ArgumentsForwarding: {}\n", "3.2")
    add("minimum-environment32", ARGS, env={"RUBOCOP_TARGET_RUBY_VERSION": "3.2"})
    add(
        "canonical-style",
        "Layout/IndentationConsistency:\n  EnforcedStyle: indented_internal_methods\n",
    )
    add("absent-parameter", "Layout/CaseIndentation:\n  EnforcedStyle: case\n")
    add("namespace-before-error", ERROR.replace("Layout/CaseIndentation", "CaseIndentation"))
    add(
        "namespace-before-warning",
        STYLE.replace("Style/EmptyClassDefinition", "EmptyClassDefinition"),
    )
    add("parent-warning", "inherit_from: parent.yml\n", extra={"parent.yml": STYLE})
    add(
        "parent-warning-replayed",
        "inherit_from: [parent.yml, parent.yml]\n",
        extra={"parent.yml": STYLE},
    )
    add(
        "child-lower-target",
        "inherit_from: parent.yml\n",
        extra={"parent.yml": BASE.replace("2.7", "3.2") + ARGS},
    )
    add(
        "out-fatal-preserved",
        ERROR,
        flags=["--out", "report.json"],
        extra={"report.json": "UNCHANGED\n"},
    )
    add("out-warning", STYLE, flags=["--out", "report.json"])
    add("force-default-error", ERROR, flags=["--force-default-config"])
    add(
        "force-default-warning",
        STYLE,
        flags=["--force-default-config", "--only", "Lint/Syntax"],
    )
    # #157の通知実装後は、旧prefixと後続通知の全rawを比較する。
    cases[-1]["comparison"] = "all_streams"
    add("warning-force-color", STYLE, env={"CLICOLOR_FORCE": "1"})
    add("warning-multiple-force-color", MULTI, "3.2", env={"CLICOLOR_FORCE": "1"})
    add(
        "fatal-force-color",
        MULTI + ERROR + "Layout/TrailingBlankLines: {}\n",
        "3.2",
        env={"CLICOLOR_FORCE": "1"},
    )
    add("ordinary-warning-control", "Layout/LineLength:\n  Bogus149: true\n")
    add(
        "ordinary-warning-color-control",
        "Layout/LineLength:\n  Bogus149: true\n",
        env={"CLICOLOR_FORCE": "1"},
    )
    add("warning-color-flag", STYLE, flags=["--color"])
    add("warning-force-no-color", STYLE, flags=["--no-color"], env={"CLICOLOR_FORCE": "1"})
    add(
        "minimum-inferred-gemspec",
        ARGS,
        target=None,
        extra={
            "probe.gemspec": (
                "Gem::Specification.new do |s|\n  s.name = 'probe'\n  s.version = '0.1.0'\n"
                "  s.required_ruby_version = '>= 3.2'\nend\n"
            )
        },
    )
    # 最初のgemspecに版が無ければ次sourceへ進む。採取器を置く親repoへ依存させない。
    add(
        "minimum-inferred-version-file",
        ARGS,
        target=None,
        extra={".ruby-version": "3.2\n", "probe.gemspec": "Gem::Specification.new {}\n"},
    )
    add(
        "minimum-gemspec-precedes-version-file",
        ARGS,
        target=None,
        extra={
            ".ruby-version": "3.2\n",
            "probe.gemspec": (
                "Gem::Specification.new do |s|\n  s.required_ruby_version = '>= 2.6'\nend\n"
            ),
        },
    )
    add(
        "force-default-warning-pending-enabled",
        STYLE,
        flags=["--force-default-config", "--enable-pending-cops", "--only", "Lint/Syntax"],
    )
    for case in cases:
        case["coverage"] = "strict"
    if tuple(case["id"] for case in cases) != CASE_IDS:
        raise ValueError("incomplete fixed scenario identity")
    return cases


def parameter_data(primary):
    """本家metadata/messageだけから60規則を作り、不完全なAPI記録を拒む。

    Returns:
        list: 実登録順の埋込み表。

    Raises:
        ValueError: 版・順序・probe・型・message形式が契約と異なる場合。

    """
    if not (primary.get("ruby") == "4.0.7" and primary.get("gems") == VERSIONS):
        raise ValueError("参照版が不一致")
    if not (primary.get("platform") == "x64-mingw-ucrt"):
        raise ValueError("実測したWindows版ではない")
    sources = primary.get("sources", {})
    if not (set(sources) == set(SOURCES)):
        raise ValueError("参照source集合が不一致")
    if not (
        all(
            isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value)
            for value in sources.values()
        )
    ):
        raise ValueError("参照source SHAが不正")
    rules = primary.get("rules", [])
    if not (isinstance(rules, list) and len(rules) == 60):
        raise ValueError("parameter表が不完全")
    if not (
        all(type(row.get("index")) is int and type(row.get("warning")) is bool for row in rules)
    ):
        raise ValueError("index/severity型が不正")
    if not ([row["index"] for row in rules] == list(range(67, 127))):
        raise ValueError("実API順が不一致")
    if not (tuple((row.get("cop"), row.get("parameter")) for row in rules) == RULE_IDS):
        raise ValueError("規則identity順が不一致")
    if not (sum(row["warning"] for row in rules) == 30):
        raise ValueError("severity件数が不一致")
    if not (
        [row.get("kind") for row in rules]
        == ["ChangedParameter"] * 58 + ["ChangedEnforcedStyles"] * 2
    ):
        raise ValueError("category順が不一致")
    result = []
    for row in rules:
        metadata = row.get("metadata")
        if not (isinstance(metadata, dict) and metadata):
            raise ValueError("metadataが空")
        minimum = metadata.get("minimum_ruby_version")
        if not (
            minimum is None or type(minimum) is float and minimum == 3.2 and row["index"] == 119
        ):
            raise ValueError("最低版が未記録の型/条件")
        kind = "parameter" if row["kind"] == "ChangedParameter" else "enforced_style"
        value = metadata.get("value")
        if not (
            (kind == "parameter" and value is None)
            or (kind == "enforced_style" and isinstance(value, str))
        ):
            raise ValueError("未記録のstyle型")
        probes = row.get("probes", [])
        if not (isinstance(probes, list) and len(probes) == 7):
            raise ValueError("probe件数が不正")
        matched = metadata.get("value", True)
        forms = [
            ("absent", False, None, 3.2),
            ("false", True, False, 3.2),
            ("null", True, None, 3.2),
            ("matching", True, matched, 3.2),
            ("other", True, "other_style", 3.2),
            ("minimum_low", True, matched, 3.1),
            ("minimum_high", True, matched, 3.2),
        ]
        for probe, (name, present, probe_value, target) in zip(probes, forms, strict=True):
            if not (set(probe) == {"id", "present", "value", "target", "violated"}):
                raise ValueError("probe fieldsが不一致")
            if not (
                type(probe["present"]) is bool
                and type(probe["violated"]) is bool
                and type(probe["target"]) is float
            ):
                raise ValueError("probe型が不正")
            if not (
                probe["id"] == name
                and probe["present"] is present
                and type(probe["value"]) is type(probe_value)
                and probe["value"] == probe_value
                and probe["target"] == target
            ):
                raise ValueError("probe入力順が不一致")
        if not (isinstance(row.get("message"), str) and "\n" in row["message"]):
            raise ValueError("messageが不完全")
        head, detail = row["message"].split("\n", 1)
        subject = (
            f"parameter `{row['parameter']}`"
            if kind == "parameter"
            else f"`{row['parameter']}: {value}`"
        )
        if not (
            head == f"obsolete {subject} (for `{row['cop']}`) found in config.yml"
            and detail
            and not detail.endswith("\n")
        ):
            raise ValueError("本家message形式が不一致")
        result.append(
            {
                "kind": kind,
                "cop": row["cop"],
                "parameter": row["parameter"],
                "severity": "warning" if row["warning"] else "error",
                "minimum_ruby_version": None if minimum is None else str(minimum),
                "value": value,
                "detail": detail,
            }
        )
    return result


def completed_report(raw):
    """正常31条件の全reportを保持し、空/重複pathや不正なsummaryを拒む。

    Returns:
        dict: identity以外の全report fields。

    Raises:
        ValueError: 版・完了集合・offense・件数型が契約と異なる場合。

    """
    report = json.loads(raw)
    identity = report.get("metadata", {})
    if not (
        identity.get("rubocop_version") == "1.89.0"
        and identity.get("ruby_engine") == "ruby"
        and identity.get("ruby_version") == "4.0.7"
    ):
        raise ValueError("本家report identityが不一致")
    summary = report.get("summary", {})
    if not (
        all(
            type(summary.get(key)) is int
            for key in ("offense_count", "target_file_count", "inspected_file_count")
        )
    ):
        raise ValueError("件数が整数でない")
    if not (summary == {"offense_count": 0, "target_file_count": 1, "inspected_file_count": 1}):
        raise ValueError("本家の正常JSONが未完了")
    files = report.get("files", [])
    if not (
        isinstance(files, list)
        and len(files) == 1
        and files[0].get("path") == "a.rb"
        and files[0].get("offenses") == []
    ):
        raise ValueError("本家の完了path/offenseが不一致")
    report.pop("metadata")
    return report


def _validate_selection(only):
    if not (only is None or only and len(set(only)) == len(only) and not set(only) - set(CASE_IDS)):
        raise ValueError("空/重複/未知case指定")


def _selected_inputs(only=None):
    _validate_selection(only)
    forms = case_inputs()
    if not (tuple(row["id"] for row in forms) == CASE_IDS):
        raise ValueError("固定41入力が不完全")
    return [case for case in forms if only is None or case["id"] in only]


def _fresh_paths(*paths):
    if not (not any(os.path.lexists(path) for path in paths)):
        raise ValueError("既存entry/danglingを上書きしない")
    resolved = tuple(path.resolve(strict=False) for path in paths)
    identities = {os.path.normcase(os.path.normpath(str(path))) for path in resolved}
    if not (len(identities) == len(paths) and not any(os.path.lexists(path) for path in resolved)):
        raise ValueError("保存先alias/既存正規化path")
    return resolved


@contextmanager
def _workspace(work):
    if work is None:
        with tempfile.TemporaryDirectory(prefix="sonicop-parameter-obsoletion-") as temporary:
            yield Path(temporary)
    else:
        work.mkdir(parents=True, exist_ok=False)
        yield work


def _inventory(directory, files):
    return {
        name: ((directory / name).read_bytes(), (directory / name).stat().st_mtime_ns)
        for name in files
    }


def record(ruby, form, work=None):
    """廃止項目の拒否/正常段階と原本・既存out保存を満たすCLIだけを記録する。

    Returns:
        dict: 元入力・終了値・全rawと明示的coverage。

    Raises:
        ValueError: 未知入力・異常終了・不完全JSON・原本変更がある場合。

    """
    expected = next((case for case in _selected_inputs() if case["id"] == form.get("id")), None)
    if not (expected is not None and form == expected):
        raise ValueError("固定入力と不一致")
    with _workspace(work) as directory:
        for name, text in form["files"].items():
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode())
        before = _inventory(directory, form["files"])
        done = subprocess.run(
            [
                ruby,
                "-e",
                LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n',
                "--",
                *form["args"],
            ],
            cwd=directory,
            env={**reference_environment(), **form["env"]},
            capture_output=True,
            check=False,
            timeout=60,
        )
        if not (before == _inventory(directory, form["files"])):
            raise ValueError("原本/config/Gemfile/既存outのbytesまたはmtimeが変化")
        error = form["id"] in FATAL_IDS
        if not (type(done.returncode) is int and done.returncode == (2 if error else 0)):
            raise ValueError("拒否/正常phaseが不一致")
        out = directory / "report.json"
        if not (out.exists() == ("--out" in form["args"])):
            raise ValueError("想定外のout生成/欠落")
        if error:
            if not (not done.stdout and b"Error:" in done.stderr and b"obsolete " in done.stderr):
                raise ValueError("廃止設定以外の失敗")
        else:
            if "--out" in form["args"]:
                if not (not done.stdout):
                    raise ValueError("out時にstdoutへreportを重複出力")
            completed_report(out.read_bytes() if out.exists() else done.stdout)
        row = {
            **form,
            "exit": done.returncode,
            "stdout_hex": done.stdout.hex(),
            "stderr_hex": done.stderr.hex(),
            "out_exists": out.exists(),
            "out_hex": out.read_bytes().hex() if out.exists() else None,
            "inputs_bytes_mtime_unchanged": True,
        }
        if form["id"] == "force-default-warning":
            prefix, marker, _ = done.stderr.partition(b"The following cops were added to RuboCop,")
            if not (marker and prefix.startswith(b"Warning: obsolete ")):
                raise ValueError("既知通知境界が不一致")
            row["obsoletion_prefix_hex"] = prefix.hex()
        row["coverage"] = row.pop("coverage")
        with (directory / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(row, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        return row


def _fixture(primary, rows, only=None):
    parameter_data(primary)
    forms = _selected_inputs(only)
    if not (rows and [row["id"] for row in rows] == [form["id"] for form in forms]):
        raise ValueError("採取case集合が不完全")
    for row, form in zip(rows, forms, strict=True):
        if not ({key: row[key] for key in form} == form):
            raise ValueError("採取入力/coverageが不一致")
    return {
        "schema": 1,
        "primary": primary,
        "cases": rows,
        "coverage": {
            "parameter_api": 60,
            "changed_parameter_api": 58,
            "changed_enforced_styles_api": 2,
            "error_api": 30,
            "warning_api": 30,
            "actual_cli_cases": len(rows),
            "strict_cli_cases": sum(row["coverage"] == "strict" for row in rows),
            "report_only_cli_cases": sum(row["coverage"] == "prefix_157" for row in rows),
            "all_60_distinct_cli_claimed": False,
        },
    }


def main():
    """候補pathを受け取らず、固定版の実RuboCopだけをfresh保存先へ再採取する。

    Raises:
        ValueError: metadataがstderrを書いた場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rules-output", type=Path, required=True)
    parser.add_argument("--case", nargs="*")
    options = parser.parse_args()
    work, output, rules_output = _fresh_paths(
        options.work_dir, options.output, options.rules_output
    )
    _validate_selection(options.case)
    work.mkdir(parents=True, exist_ok=False)
    meta = subprocess.run(
        [options.ruby, "-e", API],
        cwd=work,
        env=reference_environment(),
        capture_output=True,
        timeout=60,
        check=True,
    )
    if not (not meta.stderr):
        raise ValueError("metadataがstderrを出力")
    primary = json.loads(meta.stdout)
    parameter_data(primary)
    rows = []
    for form in _selected_inputs(options.case):
        rows.append(record(options.ruby, form, work / form["id"]))
        print(f"recorded {len(rows)} {form['id']}", flush=True)
    result = _fixture(primary, rows, options.case)
    for path, value in [(output, result), (rules_output, parameter_data(primary))]:
        with path.open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")


if __name__ == "__main__":
    main()
