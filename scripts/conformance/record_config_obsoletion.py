"""本家のfatal rename49と初期検証22CLIを、候補を呼ばずに記録する。"""

import argparse
import copy
import hashlib
import json
import os
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

BASE = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n"
OLD = "Layout/TrailingBlankLines:\n  Enabled: false\n"
API = (
    LOADER
    + """
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
obsoletion = RuboCop::ConfigObsoletion.new(RuboCop::Config.new({}))
sources = %w[config/obsoletion.yml lib/rubocop/config_obsoletion.rb lib/rubocop/config_obsoletion/renamed_cop.rb lib/rubocop/config_obsoletion/cop_rule.rb lib/rubocop/config_obsoletion/rule.rb lib/rubocop/config_validator.rb lib/rubocop/config_loader.rb LICENSE.txt].to_h do |file|
  [file, Digest::SHA256.file(File.join(spec.full_gem_path, file)).hexdigest]
end
rules = obsoletion.rules.filter_map do |rule|
  next unless rule.is_a?(RuboCop::ConfigObsoletion::RenamedCop)
  {kind: 'renamed', old_name: rule.old_name,
   short_name: RuboCop::Cop::Badge.parse(rule.old_name).cop_name,
   new_name: rule.new_name, warning: rule.warning?, message: rule.rule_message}
end
puts JSON.generate({ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] },
  sources: sources, rules: rules,
  all_rules_by_class: obsoletion.rules.group_by { |rule| rule.class.name }.transform_values(&:size)})
"""
)


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def case_inputs():
    """初期phaseの同じcauseだけを22条件へ固定する。

    Returns:
        list: 実CLIで採取する固定22条件。

    """
    forms = [
        ("full-disabled", OLD, [], {}, {}, ["a.rb"]),
        ("full-ignore", OLD, ["--ignore-unrecognized-cops"], {}, {}, ["a.rb"]),
        ("bare-disabled", "TrailingBlankLines:\n  Enabled: false\n", [], {}, {}, ["a.rb"]),
        ("moved", "Metrics/LineLength: {}\n", [], {}, {}, ["a.rb"]),
        ("two-reversed", "Style/DeprecatedHashMethods: {}\n" + OLD, [], {}, {}, ["a.rb"]),
        ("parent", "inherit_from: parent.yml\n", [], {}, {"parent.yml": OLD}, ["a.rb"]),
        (
            "parent-overridden",
            "inherit_from: parent.yml\nLayout/TrailingBlankLines: null\n",
            [],
            {},
            {"parent.yml": OLD},
            ["a.rb"],
        ),
        ("full-null", "Layout/TrailingBlankLines: null\n", [], {}, {}, ["a.rb"]),
        ("bare-null", "TrailingBlankLines: null\n", [], {}, {}, ["a.rb"]),
        ("early-missing-target", OLD, ["--out", "report.json"], {}, {}, ["missing.rb"]),
        ("version", OLD, ["--version"], {}, {}, ["a.rb"]),
        ("verbose-version", OLD, ["--verbose-version"], {}, {}, ["a.rb"]),
        ("force-default", OLD, ["--force-default-config"], {}, {}, ["a.rb"]),
        (
            "out-sentinel",
            OLD,
            ["--out", "report.json"],
            {},
            {"report.json": "UNCHANGED\n"},
            ["a.rb"],
        ),
        ("color-flag", OLD, ["--color"], {}, {}, ["a.rb"]),
        ("force-color", OLD, [], {"CLICOLOR_FORCE": "1"}, {}, ["a.rb"]),
        ("force-no-color", OLD, ["--no-color"], {"CLICOLOR_FORCE": "1"}, {}, ["a.rb"]),
        ("namespace-first", "SpaceAroundOperators: {}\n" + OLD, [], {}, {}, ["a.rb"]),
        (
            "parameter-suppressed",
            "Layout/SpaceAroundOperators:\n  Bogus: true\n" + OLD,
            [],
            {},
            {},
            ["a.rb"],
        ),
        (
            "canonical-negative",
            "Layout/TrailingEmptyLines:\n  Enabled: false\n",
            [],
            {},
            {},
            ["a.rb"],
        ),
        ("full-enabled", "Layout/TrailingBlankLines:\n  Enabled: true\n", [], {}, {}, ["a.rb"]),
        ("old-scalar", "Layout/TrailingBlankLines: value\n", [], {}, {}, ["a.rb"]),
    ]
    return [
        {
            "id": name,
            "files": {"Gemfile": "", "a.rb": "x=1\n", "config.yml": BASE + tail, **extra},
            "args": [
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--format",
                "json",
                *flags,
                *targets,
            ],
            "env": {"CLICOLOR_FORCE": "0", **env},
        }
        for name, tail, flags, env, extra, targets in forms
    ]


def renamed_data(primary):
    """実APIのmessageからactionを照合し、移行先やseverityを推測しない。

    Returns:
        list: 登録順を保つ51行の埋め込み表。

    Raises:
        ValueError: APIの名前・severity・messageが不完全な場合。

    """
    rules = primary["rules"]
    if len(rules) != 51 or len({row["old_name"] for row in rules}) != 51:
        raise ValueError("rename表が不完全")
    _require(sum(row["warning"] for row in rules) == 2, "severity件数が不一致")
    _require(
        all(row["kind"] == "renamed" and isinstance(row["warning"], bool) for row in rules),
        "rule型が不一致",
    )
    result = []
    for row in rules:
        actions = [
            action
            for action in ("renamed", "moved")
            if row["message"]
            == f"The `{row['old_name']}` cop has been {action} to `{row['new_name']}`."
        ]
        _require(len(actions) == 1, "本家messageのactionが不一致")
        result.append(
            {
                "old_name": row["old_name"],
                "short_name": row["short_name"],
                "new_name": row["new_name"],
                "severity": "warning" if row["warning"] else "error",
                "action": actions[0],
            }
        )
    return result


CASE_IDS = (
    "full-disabled",
    "full-ignore",
    "bare-disabled",
    "moved",
    "two-reversed",
    "parent",
    "parent-overridden",
    "full-null",
    "bare-null",
    "early-missing-target",
    "version",
    "verbose-version",
    "force-default",
    "out-sentinel",
    "color-flag",
    "force-color",
    "force-no-color",
    "namespace-first",
    "parameter-suppressed",
    "canonical-negative",
    "full-enabled",
    "old-scalar",
)
ORDER_IDS = (
    "fatal-before-unknown-force-0",
    "fatal-before-unknown-force-1",
    "completed-warning-before-fatal-and-missing-force-0",
    "completed-warning-before-fatal-and-missing-force-1",
)


def _order_inputs():
    forms = []
    for kind in ("fatal-before-unknown", "completed-warning-before-fatal-and-missing"):
        for force in ("0", "1"):
            files = {"Gemfile": "", "a.rb": "x=1\n"}
            flags = []
            if kind == "fatal-before-unknown":
                files["config.yml"] = BASE + "NoSuchCop: {}\nLayout/TrailingBlankLines: {}\n"
            else:
                files.update(
                    {
                        "config.yml": BASE
                        + "inherit_from: [unknown.yml, obsolete.yml, missing.yml]\n",
                        "unknown.yml": "NoSuchCop: {}\n",
                        "obsolete.yml": "Layout/TrailingBlankLines: {}\n",
                        "report.json": "UNCHANGED\n",
                    }
                )
                flags = ["--ignore-unrecognized-cops"]
            forms.append(
                {
                    "id": f"{kind}-force-{force}",
                    "files": files,
                    "args": [
                        "--config",
                        "config.yml",
                        "--cache",
                        "false",
                        "--format",
                        "json",
                        "--out",
                        "report.json",
                        *flags,
                        "a.rb",
                    ],
                    "env": {"TERM": "dumb", "CLICOLOR_FORCE": force},
                }
            )
    return forms


def _selected_inputs(only=None, order=False):
    forms = _order_inputs() if order else case_inputs()
    expected = ORDER_IDS if order else CASE_IDS
    _require(tuple(form["id"] for form in forms) == expected, "有限入力の欠落・重複・順序差")
    _require(
        only is None or (only and len(set(only)) == len(only) and not set(only) - set(expected)),
        "空・未知・重複したcase選択",
    )
    return [form for form in forms if only is None or form["id"] in only]


def _fresh_paths(*paths):
    _require(len({path.absolute() for path in paths}) == len(paths), "出力先が重複")
    _require(
        not any(os.path.lexists(path) for path in paths),
        "既存entryやdangling symlinkを上書きしない",
    )


def _primary(ruby, work):
    done = subprocess.run(
        [ruby, "-e", API],
        cwd=work,
        env=reference_environment(),
        capture_output=True,
        timeout=60,
        check=False,
    )
    _require(done.returncode == 0 and not done.stderr, "実API照会が失敗")
    primary = json.loads(done.stdout)
    _require(
        primary["ruby"] == "4.0.7"
        and primary["gems"] == VERSIONS
        and "mingw" in primary["platform"],
        "実runtimeが不一致",
    )
    _require(
        all(type(count) is int for count in primary["all_rules_by_class"].values()),
        "API件数は整数だけを認める",
    )
    _require(
        primary["all_rules_by_class"]
        == {
            "RuboCop::ConfigObsoletion::RenamedCop": 51,
            "RuboCop::ConfigObsoletion::RemovedCop": 13,
            "RuboCop::ConfigObsoletion::SplitCop": 1,
            "RuboCop::ConfigObsoletion::ExtractedCop": 2,
            "RuboCop::ConfigObsoletion::ChangedParameter": 58,
            "RuboCop::ConfigObsoletion::ChangedEnforcedStyles": 2,
        },
        "カテゴリAPIが不一致",
    )
    expected_sources = (
        "config/obsoletion.yml",
        "lib/rubocop/config_obsoletion.rb",
        "lib/rubocop/config_obsoletion/renamed_cop.rb",
        "lib/rubocop/config_obsoletion/cop_rule.rb",
        "lib/rubocop/config_obsoletion/rule.rb",
        "lib/rubocop/config_validator.rb",
        "lib/rubocop/config_loader.rb",
        "LICENSE.txt",
    )
    _require(tuple(primary["sources"]) == expected_sources, "source集合が不一致")
    _require(
        all(
            len(value) == 64 and all(char in "0123456789abcdef" for char in value)
            for value in primary["sources"].values()
        ),
        "source SHA256が不完全",
    )
    renamed_data(primary)
    return primary


def _inventory(directory, names):
    return {
        name: ((directory / name).read_bytes(), (directory / name).stat().st_mtime_ns)
        for name in names
    }


def _report(raw):
    value = json.loads(raw)
    identity = value["metadata"]
    _require(
        identity["rubocop_version"] == "1.89.0"
        and identity["ruby_engine"] == "ruby"
        and identity["ruby_version"] == "4.0.7",
        "本家JSON identityが不一致",
    )
    _require([file["path"] for file in value["files"]] == ["a.rb"], "対象pathが不一致")
    _require(
        all(
            type(value["summary"].get(key)) is int
            for key in ("offense_count", "target_file_count", "inspected_file_count")
        ),
        "完了件数は整数だけを認める",
    )
    _require(
        value["summary"]
        == {
            "offense_count": 0,
            "target_file_count": 1,
            "inspected_file_count": 1,
        }
        and value["files"][0]["offenses"] == [],
        "正常対照が未完了または全offenseと不一致",
    )


def _record(index, form, ruby, launcher, work, order=False):
    directory = work / f"{index:02}-{form['id']}"
    directory.mkdir(exist_ok=False)
    for name, text in form["files"].items():
        (directory / name).write_bytes(text.encode())
    before = _inventory(directory, form["files"])
    environment = reference_environment()
    environment.update(form["env"])
    done = subprocess.run(
        [ruby, str(launcher), *form["args"]],
        cwd=directory,
        env=environment,
        capture_output=True,
        timeout=60,
        check=False,
    )
    _require(
        before == _inventory(directory, form["files"]), "source/config/outのbytesまたはmtime変更"
    )
    if form["id"] == "canonical-negative":
        _require(done.returncode == 0 and not done.stderr, "正常対照が失敗")
        _report(done.stdout)
    else:
        _require(done.returncode == 2 and not done.stdout and done.stderr, "初期拒否が不完全")
        _require(
            b"The `" in done.stderr and b"obsolete configuration found in " in done.stderr,
            "rename案内以外の失敗",
        )
        _require(
            b"Configuration file not found" not in done.stderr, "先行fatalよりMissingが優先された"
        )
    exists = (directory / "report.json").exists()
    _require(exists == ("report.json" in form["files"]), "初期拒否でoutが作られた")
    row = {
        **copy.deepcopy(form),
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
        "out_exists": exists,
    }
    if order:
        row.update(
            {
                "after_hex": {
                    name: (directory / name).read_bytes().hex() for name in form["files"]
                },
                "original_mtimes_unchanged": True,
                "raw_stderr_sha256": hashlib.sha256(done.stderr).hexdigest(),
            }
        )
    with (directory / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(row, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    return row


def _fixture(primary, cases, selected=None):
    expected = list(CASE_IDS) if selected is None else [case["id"] for case in selected]
    _require([row["id"] for row in cases] == expected and cases, "有限行列が不完全")
    return {
        "schema": 1,
        "primary": primary,
        "cases": cases,
        "coverage": {
            "actual_cli_cases": len(cases),
            "actual_cli_rejections": sum(row["exit"] == 2 for row in cases),
            "actual_cli_canonical_negative": sum(
                row["id"] == "canonical-negative" for row in cases
            ),
            "renamed_api_rules": 51,
            "fatal_api_rules": 49,
            "warning_api_only_rules": 2,
            "warning_merge_implemented": False,
            "other_cop_categories_api_only": 16,
            "parameter_rules_api_only": 60,
        },
    }


def _order_runtime(ruby, work):
    query = (
        LOADER
        + "\nrequire 'json'; puts JSON.generate(ruby:RUBY_VERSION,revision:RUBY_REVISION.to_s,platform:RUBY_PLATFORM,gems:%w[parser prism rubocop-ast rubocop rainbow].to_h{|n|[n,Gem.loaded_specs.fetch(n).version.to_s]})"
    )
    done = subprocess.run(
        [ruby, "-e", query],
        cwd=work,
        env=reference_environment(),
        capture_output=True,
        timeout=60,
        check=False,
    )
    _require(done.returncode == 0 and not done.stderr, "追補runtimeが失敗")
    runtime = json.loads(done.stdout)
    _require(
        runtime["ruby"] == "4.0.7"
        and runtime["gems"] == VERSIONS
        and "mingw" in runtime["platform"]
        and runtime["revision"],
        "追補runtimeが不一致",
    )
    return runtime


def main():
    """候補を呼ばず、全件完了した実RuboCopの記録とAPI表を新規保存する。

    Raises:
        ValueError: 既存path・不完全な入力・本家記録を検出した場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--rules-output", required=True, type=Path)
    parser.add_argument("--case", nargs="*")
    parser.add_argument("--order", action="store_true")
    args = parser.parse_args()
    work, output, rules_output = (
        path.absolute() for path in (args.work_dir, args.output, args.rules_output)
    )
    _fresh_paths(work, output, rules_output)
    selected = _selected_inputs(args.case, args.order)
    work.mkdir(parents=True, exist_ok=False)
    primary = _primary(args.ruby, work)
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf-8", newline="\n") as handle:
        handle.write(LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n')
    cases = [
        _record(index, form, args.ruby, launcher, work, args.order)
        for index, form in enumerate(selected)
    ]
    if [row["id"] for row in cases] != [form["id"] for form in selected]:
        raise ValueError("完了したcase集合が入力と異なる")
    fixture = (
        {"schema": 1, "runtime": _order_runtime(args.ruby, work), "cases": cases}
        if args.order
        else _fixture(primary, cases, selected)
    )
    for path, value in [(output, fixture), (rules_output, renamed_data(primary))]:
        with path.open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")


if __name__ == "__main__":
    main()
