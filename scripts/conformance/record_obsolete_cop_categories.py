"""Removed/Split/Extractedの実APIと有限40CLIを、候補を使わず記録する。"""

import argparse
import json
import os
import subprocess
import tempfile
from contextlib import contextmanager
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

BASE = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n"
OLD = "Gemspec/DateAssignment:\n  Enabled: false\n"
API = (
    LOADER
    + """
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
rules = RuboCop::ConfigObsoletion.new(RuboCop::Config.new({}, 'config.yml')).rules
selected = rules.each_with_index.filter_map do |rule, index|
  kind = rule.class.name.split('::').last
  next unless %w[RemovedCop SplitCop ExtractedCop].include?(kind)
  row = {index: index, kind: kind, old_name: rule.old_name,
         short_name: RuboCop::Cop::Badge.parse(rule.old_name).cop_name, warning: rule.warning?}
  if kind == 'ExtractedCop'
    row.merge(department: rule.department, gem: rule.gem)
  else
    row.merge(metadata: rule.metadata, message: rule.rule_message)
  end
end
sources = %w[config/obsoletion.yml lib/rubocop/config_obsoletion.rb lib/rubocop/config_obsoletion/cop_rule.rb lib/rubocop/config_obsoletion/rule.rb lib/rubocop/config_obsoletion/removed_cop.rb lib/rubocop/config_obsoletion/split_cop.rb lib/rubocop/config_obsoletion/extracted_cop.rb lib/rubocop/config_validator.rb lib/rubocop/config_loader.rb LICENSE.txt]
puts JSON.generate({ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] },
  sources: sources.to_h { |path| [path, Digest::SHA256.file(File.join(spec.full_gem_path, path)).hexdigest] },
  counts: rules.group_by { |rule| rule.class.name.split('::').last }.transform_values(&:size), rules: selected})
"""
)
CLI = LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n"


def validate_primary(primary):
    """件数だけの空採取や版違いを成功にしない。

    Raises:
        ValueError: 版・集合・順序・primary hashが不完全な場合。

    """
    rules = primary.get("rules", [])
    expected = ["RemovedCop"] * 13 + ["SplitCop"] + ["ExtractedCop"] * 2
    if (
        primary.get("ruby") != "4.0.7"
        or primary.get("gems") != VERSIONS
        or "mingw" not in primary.get("platform", "")
        or [rule.get("kind") for rule in rules] != expected
        or [rule.get("index") for rule in rules] != list(range(51, 67))
        or len({rule.get("old_name") for rule in rules}) != 16
        or any(rule.get("warning") is not False for rule in rules)
        or primary.get("counts")
        != {
            "RenamedCop": 51,
            "RemovedCop": 13,
            "SplitCop": 1,
            "ExtractedCop": 2,
            "ChangedParameter": 58,
            "ChangedEnforcedStyles": 2,
        }
        or any(type(count) is not int for count in primary.get("counts", {}).values())
        or any(type(rule.get("index")) is not int for rule in rules)
        or set(primary.get("sources", {})) != set(SOURCES)
        or any(
            not isinstance(value, str)
            or len(value) != 64
            or any(char not in "0123456789abcdef" for char in value)
            for value in primary.get("sources", {}).values()
        )
    ):
        raise ValueError("incomplete or wrong-version obsoletion API")

    for rule in rules:
        fields = (
            ["old_name", "short_name", "department", "gem"]
            if rule["kind"] == "ExtractedCop"
            else ["old_name", "short_name", "message"]
        )
        if any(not isinstance(rule.get(key), str) or not rule[key] for key in fields):
            raise ValueError("incomplete API rule fields")
        if rule["kind"] != "ExtractedCop" and not isinstance(rule.get("metadata"), dict):
            raise ValueError("incomplete API rule metadata")


def catalogue(primary):
    """静的messageを実APIから保存し、空alternativesの特殊文面も推測しない。

    Returns:
        list: 判定用16ルールを本家の順序で並べたdata。

    """
    validate_primary(primary)
    result = []
    for rule in primary["rules"]:
        if rule["kind"] == "ExtractedCop":
            result.append(
                {
                    "kind": "extracted",
                    **{key: rule[key] for key in ["old_name", "department", "gem"]},
                }
            )
        else:
            result.append(
                {
                    "kind": "removed" if rule["kind"] == "RemovedCop" else "split",
                    **{key: rule[key] for key in ["old_name", "short_name", "message"]},
                }
            )
    return result


def cases(primary):
    """plugin無し16種類と、phase/存在判定/色/originの有限40条件に固定する。

    Returns:
        list: 実Oracleへ渡す入力・引数・明示環境の40条件。

    Raises:
        ValueError: 固定した40入力のIDが重複または欠落した場合。

    """
    validate_primary(primary)
    result = []

    def add(
        name,
        tail,
        flags=None,
        extra=None,
        env=None,
        args=None,
        root_config="config.yml",
        phase="initial_or_completed",
    ):
        result.append(
            {
                "id": name,
                "files": {
                    "Gemfile": "",
                    "a.rb": "x=1\n",
                    root_config: BASE + tail,
                    **(extra or {}),
                },
                "args": args
                or [
                    "--config",
                    root_config,
                    "--cache",
                    "false",
                    "--format",
                    "json",
                    *(flags or []),
                    "a.rb",
                ],
                "env": {"CLICOLOR_FORCE": "0", **(env or {})},
                "phase": phase,
            }
        )

    for index, rule in enumerate(primary["rules"]):
        add(f"catalogue-{index:02}", f"{rule['old_name']}:\n  Enabled: false\n")
    add("removed-bare", "DateAssignment:\n  Enabled: false\n")
    add("split-bare", "MethodMissing: {}\n")
    add("removed-null", "Gemspec/DateAssignment: null\n")
    add("split-null", "Style/MethodMissing: null\n")
    add("extracted-null", "Performance: null\n")
    add("removed-ignore", OLD, ["--ignore-unrecognized-cops"])
    add("split-ignore", "Style/MethodMissing: {}\n", ["--ignore-unrecognized-cops"])
    add("extracted-ignore", "Rails/FindEach: {}\n", ["--ignore-unrecognized-cops"])
    add("extracted-one", "Performance/Count: {}\n")
    add("extracted-many", "Performance/Detect: {}\nPerformance/Count: {}\n")
    add("extracted-department", "Performance:\n  Enabled: false\n")
    add(
        "rules-order",
        "Rails/FindEach: {}\nStyle/MethodMissing: {}\nGemspec/DateAssignment: {}\nLayout/TrailingBlankLines: {}\n",
    )
    add("namespace-first", "SpaceAroundOperators: {}\n" + OLD)
    add("parameter-suppressed", "Layout/SpaceAroundOperators:\n  Bogus: true\n" + OLD)
    add("force-color", OLD, env={"CLICOLOR_FORCE": "1"})
    add("color-flag", OLD, ["--color"])
    add("force-no-color", OLD, ["--no-color"], env={"CLICOLOR_FORCE": "1"})
    add("inherit-parent", "inherit_from: parent.yml\n", extra={"parent.yml": OLD})
    add(
        "inherit-overridden",
        "inherit_from: parent.yml\nGemspec/DateAssignment: null\n",
        extra={"parent.yml": OLD},
    )
    add(
        "nested-discovery",
        "",
        extra={"sub/.rubocop.yml": BASE + OLD, "sub/case.rb": "x=1\n"},
        root_config=".rubocop.yml",
        args=["--cache", "false", "--format", "json", "--color", "sub/case.rb"],
        phase="late_rejection",
    )
    add(
        "inherited-namespace",
        "inherit_from: parent.yml\n" + OLD,
        extra={"parent.yml": "SpaceAroundOperators: {}\n"},
    )
    add("out-sentinel", OLD, ["--out", "report.json"], extra={"report.json": "UNCHANGED\n"})
    add("canonical-negative", "Gemspec/DeprecatedAttributeAssignment:\n  Enabled: false\n")
    add("unaffected-department", "Layout:\n  Enabled: false\n")
    if len(result) != 40 or len({case["id"] for case in result}) != 40:
        raise ValueError("invalid finite scenario identity")
    for case in result:
        case["coverage"] = "strict"
    return result


def completed_report(raw):
    """正常対照の空検査・別path・不整合なsummaryを拒む。

    Returns:
        dict: identity以外の全report fields。

    Raises:
        ValueError: 正の完了集合とsummaryが不整合な場合。

    """
    report = json.loads(raw)
    _report_identity(report)
    summary = report.get("summary", {})
    files = report.get("files", [])
    if (
        not isinstance(files, list)
        or len(files) != 1
        or files[0].get("path") != "a.rb"
        or any(
            type(summary.get(key)) is not int or summary[key] != 1
            for key in ["target_file_count", "inspected_file_count"]
        )
        or not isinstance(files[0].get("offenses"), list)
        or type(summary.get("offense_count")) is not int
        or summary["offense_count"] != 0
        or files[0]["offenses"] != []
    ):
        raise ValueError("incomplete reference inspection")
    report.pop("metadata", None)
    return report


def late_rejection_report(raw):
    """後段エラーの完全なJSON文書を保存する。正常完了の証拠には使用しない。

    Returns:
        dict: 未完了0件を含むidentity以外の全report fields。

    Raises:
        ValueError: 固定した後段エラーの完了状態と異なる場合。

    """
    report = json.loads(raw)
    _report_identity(report)
    report.pop("metadata", None)
    if report != {
        "files": [],
        "summary": {"offense_count": 0, "target_file_count": 1, "inspected_file_count": 0},
    } or any(type(value) is not int for value in report.get("summary", {}).values()):
        raise ValueError("unexpected late rejection report")
    return report


CASE_IDS = (
    "catalogue-00",
    "catalogue-01",
    "catalogue-02",
    "catalogue-03",
    "catalogue-04",
    "catalogue-05",
    "catalogue-06",
    "catalogue-07",
    "catalogue-08",
    "catalogue-09",
    "catalogue-10",
    "catalogue-11",
    "catalogue-12",
    "catalogue-13",
    "catalogue-14",
    "catalogue-15",
    "removed-bare",
    "split-bare",
    "removed-null",
    "split-null",
    "extracted-null",
    "removed-ignore",
    "split-ignore",
    "extracted-ignore",
    "extracted-one",
    "extracted-many",
    "extracted-department",
    "rules-order",
    "namespace-first",
    "parameter-suppressed",
    "force-color",
    "color-flag",
    "force-no-color",
    "inherit-parent",
    "inherit-overridden",
    "nested-discovery",
    "inherited-namespace",
    "out-sentinel",
    "canonical-negative",
    "unaffected-department",
)
SOURCES = (
    "config/obsoletion.yml",
    "lib/rubocop/config_obsoletion.rb",
    "lib/rubocop/config_obsoletion/cop_rule.rb",
    "lib/rubocop/config_obsoletion/rule.rb",
    "lib/rubocop/config_obsoletion/removed_cop.rb",
    "lib/rubocop/config_obsoletion/split_cop.rb",
    "lib/rubocop/config_obsoletion/extracted_cop.rb",
    "lib/rubocop/config_validator.rb",
    "lib/rubocop/config_loader.rb",
    "LICENSE.txt",
)


def _report_identity(report):
    identity = report.get("metadata", {})
    if (
        identity.get("ruby_engine") != "ruby"
        or identity.get("ruby_version") != "4.0.7"
        or identity.get("rubocop_version") != "1.89.0"
    ):
        raise ValueError("unexpected reference report identity")


def _validate_selection(only):
    if only is not None and (not only or len(set(only)) != len(only) or set(only) - set(CASE_IDS)):
        raise ValueError("empty, duplicate or unknown scenario selection")


def _selected_inputs(primary, only=None):
    _validate_selection(only)
    forms = cases(primary)
    if tuple(case["id"] for case in forms) != CASE_IDS:
        raise ValueError("incomplete fixed input matrix")
    return [case for case in forms if only is None or case["id"] in only]


def _fresh_paths(*paths):
    if any(os.path.lexists(path) for path in paths):
        raise ValueError("existing entry or dangling symlink")
    resolved = tuple(path.resolve(strict=False) for path in paths)
    identities = {os.path.normcase(os.path.normpath(str(path))) for path in resolved}
    if len(identities) != len(paths) or any(os.path.lexists(path) for path in resolved):
        raise ValueError("output aliases or existing normalized destination")
    return resolved


@contextmanager
def _workspace(work):
    if work is None:
        with tempfile.TemporaryDirectory(prefix="sonicop-obsolete-category-") as temporary:
            yield Path(temporary)
    else:
        work.mkdir(parents=True, exist_ok=False)
        yield work


def _inventory(root, names):
    return {name: ((root / name).read_bytes(), (root / name).stat().st_mtime_ns) for name in names}


def record(ruby, case, work=None):
    """原本と既存outのbytes/mtimeを保った実CLIだけを採用する。

    Returns:
        dict: 入力条件と本家の全raw・終了値・report。

    Raises:
        ValueError: 出力段階・完了状態・原本保存が契約に反する場合。

    """
    with _workspace(work) as root:
        for name, text in case["files"].items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode())
        before = _inventory(root, case["files"])
        child = subprocess.run(
            [ruby, "-e", CLI, "--", *case["args"]],
            cwd=root,
            env={**reference_environment(), **case["env"]},
            capture_output=True,
            timeout=90,
            check=False,
        )
        completed = case["id"] in {"canonical-negative", "unaffected-department"}
        if type(child.returncode) is not int or child.returncode != (0 if completed else 2):
            raise ValueError("unexpected reference exit")
        out = root / "report.json"
        late = case["phase"] == "late_rejection"
        if case["coverage"] != "strict":
            raise ValueError("wrong finite comparison coverage")
        if not completed and (
            (child.stdout and not late) or b"obsolete configuration found in " not in child.stderr
        ):
            raise ValueError("unexpected refusal or formatter output before initial failure")
        if completed and child.stderr:
            raise ValueError("normal control wrote stderr")
        if out.exists() != ("report.json" in case["files"]):
            raise ValueError("unexpected report artifact")
        if before != _inventory(root, case["files"]):
            raise ValueError("reference changed input or existing output")
        result = {
            **case,
            "exit": child.returncode,
            "stdout_hex": child.stdout.hex(),
            "stderr_hex": child.stderr.hex(),
            "out_exists": out.exists(),
        }
        if completed:
            result["report"] = completed_report(child.stdout)
        elif late:
            result["rejection_report"] = late_rejection_report(child.stdout)
        # 元参照の既存field順を保ち、追加coverageだけを末尾へ置く。
        result["coverage"] = result.pop("coverage")
        with (root / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(result, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        return result


def _fixture(primary, rows, only=None):
    forms = _selected_inputs(primary, only)
    if not rows or [row["id"] for row in rows] != [case["id"] for case in forms]:
        raise ValueError("incomplete recorded matrix")
    for row, form in zip(rows, forms, strict=True):
        if {key: row[key] for key in form} != form:
            raise ValueError("recorded input differs from fixed args/files/env/phase")
    return {"schema": 1, "primary": primary, "cases": rows}


def main():
    """候補pathを受け取らず、固定した実RuboCopのAPIとCLIだけを再採取する。

    Raises:
        ValueError: 実APIがstderrを書いた場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rules-output", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
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
    if meta.stderr:
        raise ValueError("metadata wrote stderr")
    primary = json.loads(meta.stdout)
    validate_primary(primary)
    selected = _selected_inputs(primary, options.case)
    rows = [record(options.ruby, case, work / case["id"]) for case in selected]
    result = _fixture(primary, rows, options.case)
    for path, value in [(output, result), (rules_output, catalogue(primary))]:
        with path.open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")


if __name__ == "__main__":
    main()
