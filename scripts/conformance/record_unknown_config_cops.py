"""設定節の未知cop判定とignore時の警告を、固定した実RuboCopから記録する。"""

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from contextlib import contextmanager
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

BOOT = LOADER + "\nabort 'Ruby 4.0.7 required' unless RUBY_VERSION == '4.0.7'\n"
API = (
    BOOT
    + """
require "json"
require "digest"
spec = Gem::Specification.find_by_name("rubocop", "1.89.0")
files = %w[config_validator.rb config_loader.rb cop/registry.rb cop/badge.rb name_similarity.rb]
source_sha256 = files.to_h do |file|
  [file, Digest::SHA256.file(File.join(spec.full_gem_path, "lib/rubocop", file)).hexdigest]
end
gems = %w[parser prism rubocop-ast rubocop rainbow].to_h do |name|
  [name, Gem.loaded_specs.fetch(name).version.to_s]
end
STDOUT.binmode.write JSON.generate({ruby: RUBY_VERSION, gems: gems,
  registry: RuboCop::Cop::Registry.global.map(&:cop_name),
  departments: RuboCop::Cop::Registry.global.departments,
  legacy: RuboCop::ConfigObsoletion.legacy_cop_names,
  default_keys: RuboCop::ConfigLoader.default_configuration.keys,
  source_sha256: source_sha256})
"""
)
CLI = BOOT + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")'
BASE = (
    "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
    "  SuggestExtensions: false\n  DisabledByDefault: true\n"
)


def scenarios():
    """既知の入力条件だけを作り、候補や保存済みexpectedは読まない。

    Returns:
        list: 仕様版で有限検証した42条件。

    """
    cases = []

    def add(name, tail, args=None, files=None, base=BASE, env=None, comparison="strict"):
        cases.append(
            {
                "id": name,
                "files": {
                    "Gemfile": "",
                    "a.rb": "x=1\n",
                    "config.yml": base + tail,
                    **(files or {}),
                },
                "args": [
                    "--config",
                    "config.yml",
                    "--cache",
                    "false",
                    "--format",
                    "json",
                    *(args or []),
                    "a.rb",
                ],
                "env": {"CLICOLOR_FORCE": "0", **(env or {})},
                "comparison": comparison,
                "coverage": "strict",
            }
        )

    for name, value in [
        ("map", "\n  Enabled: true"),
        ("disabled", "\n  Enabled: false"),
        ("empty-map", " {}"),
        ("null", ""),
        ("scalar", " value"),
    ]:
        add(f"unknown-{name}", f"NoSuchCop:{value}\n")
    add("unknown-full", "Unknown/Cop:\n  Enabled: true\n")
    add("two-unknown", "UnknownOmega: {}\nUnknownAlpha: {}\n")
    add("unknown-lower", "no_such_cop: {}\n")
    add("unknown-unicode", "存在しないCop: {}\n")
    add("typo-department", "Layoat: {}\n")
    add("typo-full", "Layout/SpaceAroundOperatorz: {}\n")
    add("root-layout", "Layout:\n  Enabled: false\n")
    add("known-short", "SpaceAroundOperators:\n  Enabled: true\n")
    add("known-full", "Layout/SpaceAroundOperators:\n  Enabled: true\n")
    add("known-snake-short", "space_around_operators:\n  Enabled: true\n")
    add("known-snake-full", "Layout/space_around_operators:\n  Enabled: true\n")
    add("reserved-inherit-mode", "inherit_mode:\n  merge: [Exclude]\n")
    add("reserved-inherit-from", "inherit_from: []\n")
    add("reserved-inherit-gem", "inherit_gem: {}\n")
    add("reserved-require", "require: []\n")
    add("reserved-plugins", "plugins: []\n")
    add("all-cops-parameter", "", base=BASE + "  NoSuchCop: true\n")
    add("all-cops-case", "all_cops: {}\n")
    add("wrong-namespace", "Style/SpaceAroundOperators:\n  Enabled: false\n")
    add("unknown-with-short", "NoSuchCop: {}\nSpaceAroundOperators:\n  Enabled: true\n")
    add(
        "unknown-with-bad-parameter", "NoSuchCop: {}\nLayout/SpaceAroundOperators:\n  Bogus: true\n"
    )
    add("ignore", "NoSuchCop: {}\n", ["--ignore-unrecognized-cops"])
    add("ignore-color", "NoSuchCop: {}\n", ["--ignore-unrecognized-cops", "--color"])
    add(
        "ignore-two-color",
        "UnknownOmega: {}\nUnknownAlpha: {}\n",
        ["--color", "--ignore-unrecognized-cops"],
    )
    add(
        "ignore-with-bad-parameter",
        "NoSuchCop: {}\nLayout/SpaceAroundOperators:\n  Bogus: true\n",
        ["--ignore-unrecognized-cops"],
    )
    add("reject-color", "NoSuchCop: {}\n", ["--color"])
    add("reject-out", "NoSuchCop: {}\n", ["--out", "report.json"])
    add("reject-version", "NoSuchCop: {}\n", ["--version"])
    add("reject-verbose-version", "NoSuchCop: {}\n", ["--verbose-version"])
    add("reject-force-default", "NoSuchCop: {}\n", ["--force-default-config"])
    add(
        "inherited-unknown",
        "inherit_from: parent.yml\n",
        files={"parent.yml": "UnknownOmega: {}\n"},
    )
    add(
        "inherited-overridden",
        "inherit_from: parent.yml\nUnknownOmega: null\n",
        files={"parent.yml": "UnknownOmega: {}\n"},
    )
    add(
        "ignore-force",
        "NoSuchCop: {}\n",
        ["--ignore-unrecognized-cops"],
        env={"CLICOLOR_FORCE": "1"},
    )
    add(
        "ignore-force-no-color",
        "NoSuchCop: {}\n",
        ["--ignore-unrecognized-cops", "--no-color"],
        env={"CLICOLOR_FORCE": "1"},
    )
    add("reject-force", "NoSuchCop: {}\n", env={"CLICOLOR_FORCE": "1"})
    for name, args in [("legacy-disabled", []), ("legacy-ignore", ["--ignore-unrecognized-cops"])]:
        add(
            name,
            "Layout/TrailingBlankLines:\n  Enabled: false\n",
            args,
        )
    return cases


CASE_IDS = (
    "unknown-map",
    "unknown-disabled",
    "unknown-empty-map",
    "unknown-null",
    "unknown-scalar",
    "unknown-full",
    "two-unknown",
    "unknown-lower",
    "unknown-unicode",
    "typo-department",
    "typo-full",
    "root-layout",
    "known-short",
    "known-full",
    "known-snake-short",
    "known-snake-full",
    "reserved-inherit-mode",
    "reserved-inherit-from",
    "reserved-inherit-gem",
    "reserved-require",
    "reserved-plugins",
    "all-cops-parameter",
    "all-cops-case",
    "wrong-namespace",
    "unknown-with-short",
    "unknown-with-bad-parameter",
    "ignore",
    "ignore-color",
    "ignore-two-color",
    "ignore-with-bad-parameter",
    "reject-color",
    "reject-out",
    "reject-version",
    "reject-verbose-version",
    "reject-force-default",
    "inherited-unknown",
    "inherited-overridden",
    "ignore-force",
    "ignore-force-no-color",
    "reject-force",
    "legacy-disabled",
    "legacy-ignore",
)


def _selected_inputs(only):
    forms = scenarios()
    if [case["id"] for case in forms] != list(CASE_IDS):
        raise ValueError("incomplete input matrix")
    if only is not None and (not only or len(set(only)) != len(only) or set(only) - set(CASE_IDS)):
        raise ValueError("empty, duplicate or unknown selected cases")
    return [case for case in forms if only is None or case["id"] in only]


@contextmanager
def _workspace(work):
    if work is None:
        with tempfile.TemporaryDirectory(prefix="sonicop-config-name-") as temporary:
            yield Path(temporary)
    else:
        work.mkdir(parents=True, exist_ok=False)
        yield work


def _fresh_paths(work, output):
    if work.absolute() == output.absolute() or any(
        os.path.lexists(path) for path in (work, output)
    ):
        raise ValueError("既存work/outputを上書きしない")


def validate_api(api):
    """別版・不足したRegistryの記録を拒む。

    Raises:
        ValueError: 対象版または登録集合が不完全な場合。

    """
    if api.get("ruby") != "4.0.7" or api.get("gems") != VERSIONS:
        raise ValueError("unexpected reference versions")
    for key, count in [
        ("registry", 609),
        ("departments", 9),
        ("legacy", 67),
        ("default_keys", 610),
    ]:
        names = api.get(key)
        if not isinstance(names, list) or len(names) != count or len(set(names)) != count:
            raise ValueError(f"incomplete reference {key}")


def completed_report(stdout):
    """終了0/1を、空や途中のJSONだけで成功にしない。

    Returns:
        dict: metadata以外の診断全fields。

    Raises:
        ValueError: reportの対象/件数が不完全な場合。

    """
    report = json.loads(stdout)
    metadata = report.get("metadata", {})
    if (
        metadata.get("rubocop_version") != "1.89.0"
        or metadata.get("ruby_engine") != "ruby"
        or metadata.get("ruby_version") != "4.0.7"
    ):
        raise ValueError("unexpected reference identity")
    files = report.get("files")
    summary = report.get("summary")
    if not isinstance(files, list) or len(files) != 1 or files[0].get("path") != "a.rb":
        raise ValueError("unexpected reference targets")
    if not isinstance(summary, dict) or any(
        type(summary.get(key)) is not int or summary[key] != 1
        for key in ["target_file_count", "inspected_file_count"]
    ):
        raise ValueError("incomplete reference inspection")
    offenses = files[0].get("offenses")
    if (
        not isinstance(offenses, list)
        or type(summary.get("offense_count")) is not int
        or summary["offense_count"] != len(offenses)
    ):
        raise ValueError("reference offense count mismatch")
    report.pop("metadata", None)
    return report


def record(ruby, case, work=None):
    """原本を保った本家のbytesと完了情報だけを返す。

    Returns:
        dict: 有限入力に対応する参照結果。

    Raises:
        ValueError: CLIが途中終了したか原本や拒否時のartifactを変更した場合。

    """
    with _workspace(work) as root:
        for path, text in case["files"].items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_bytes(text.encode())
        before_times = {path: (root / path).stat().st_mtime_ns for path in case["files"]}
        child = subprocess.run(
            [ruby, "-e", CLI, "--", *case["args"]],
            cwd=root,
            env={**reference_environment(), **case["env"]},
            capture_output=True,
            timeout=90,
        )
        if child.returncode not in (0, 1, 2):
            raise ValueError("unexpected reference exit")
        out_exists = (root / "report.json").exists()
        if child.returncode == 2 and (child.stdout or not child.stderr or out_exists):
            raise ValueError("configuration rejection happened after output creation")
        result = {
            **case,
            "exit": child.returncode,
            "stdout_hex": child.stdout.hex(),
            "stderr_hex": child.stderr.hex(),
            "out_exists": out_exists,
            "raw_stdout_sha256": hashlib.sha256(child.stdout).hexdigest(),
            "raw_stderr_sha256": hashlib.sha256(child.stderr).hexdigest(),
        }
        if child.returncode != 2:
            result["report"] = completed_report(child.stdout)
        after = {path: (root / path).read_bytes() for path in case["files"]}
        if after != {path: text.encode() for path, text in case["files"].items()}:
            raise ValueError("reference changed an inspection input")
        if before_times != {path: (root / path).stat().st_mtime_ns for path in case["files"]}:
            raise ValueError("reference changed an input mtime")
        with (root / "raw.json").open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
        return result


TRACE_IDS = (
    "diamond-ignore-force-0",
    "diamond-ignore-force-1",
    "warning-before-missing-force-0",
    "warning-before-missing-force-1",
)
TRACE_API = (
    BOOT
    + """
require 'json'
STDOUT.binmode.write JSON.generate({ruby: RUBY_VERSION, revision: RUBY_REVISION.to_s, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] }})
"""
)


def _trace_inputs(only=None):
    forms = []
    for kind in ("diamond-ignore", "warning-before-missing"):
        for force in ("0", "1"):
            files = {"Gemfile": "", "a.rb": "x=1\n", "common.yml": "NoSuchCop: {}\n"}
            if kind == "diamond-ignore":
                files.update(
                    {
                        "config.yml": BASE + "inherit_from: [left.yml, right.yml]\n",
                        "left.yml": "inherit_from: common.yml\n",
                        "right.yml": "inherit_from: common.yml\n",
                    }
                )
            else:
                files["config.yml"] = BASE + "inherit_from: [common.yml, missing.yml]\n"
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
                        "--ignore-unrecognized-cops",
                        "a.rb",
                    ],
                    "env": {"TERM": "dumb", "CLICOLOR_FORCE": force},
                }
            )
    if [case["id"] for case in forms] != list(TRACE_IDS):
        raise ValueError("incomplete trace matrix")
    if only is not None and (not only or len(set(only)) != len(only) or set(only) - set(TRACE_IDS)):
        raise ValueError("empty, duplicate or unknown trace selection")
    return [case for case in forms if only is None or case["id"] in only]


def _record_trace(ruby, selected, work):
    meta = subprocess.run(
        [ruby, "-e", TRACE_API],
        env=reference_environment(),
        capture_output=True,
        check=True,
        timeout=60,
    )
    if meta.stderr:
        raise ValueError("trace metadata wrote stderr")
    runtime = json.loads(meta.stdout)
    if (
        runtime["ruby"] != "4.0.7"
        or runtime["gems"] != VERSIONS
        or "mingw" not in runtime["platform"]
    ):
        raise ValueError("unexpected trace runtime")
    rows = []
    for case in selected:
        directory = work / case["id"]
        raw = record(ruby, case, directory)
        stderr = bytes.fromhex(raw["stderr_hex"])
        projected = stderr.replace(directory.as_posix().encode(), b"<ROOT>")
        rows.append(
            {
                **case,
                "exit": raw["exit"],
                "stdout_hex": raw["stdout_hex"],
                "stderr_template_hex": projected.hex(),
                "raw_stderr_sha256": raw["raw_stderr_sha256"],
                "report": json.loads(bytes.fromhex(raw["stdout_hex"]))
                if raw["exit"] != 2
                else None,
                "after_hex": {
                    name: (directory / name).read_bytes().hex() for name in case["files"]
                },
                "original_mtimes_unchanged": True,
            }
        )
    return {"schema": 1, "runtime": runtime, "cases": rows}


def main():
    """候補への引数を受け取らず、固定版の実RuboCopだけを再採取する。

    Raises:
        ValueError: 仕様版metadataの採取がstderrを出した場合。

    """
    parser = argparse.ArgumentParser()
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--case", action="append")
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument(
        "--trace", action="store_true", help="継承DAGとMissing前の警告4条件を別採取"
    )
    options = parser.parse_args()
    work, output = options.work_dir.absolute(), options.output.absolute()
    _fresh_paths(work, output)
    selected = _trace_inputs(options.case) if options.trace else _selected_inputs(options.case)
    work.mkdir(parents=True, exist_ok=False)
    meta = subprocess.run(
        [options.ruby, "-e", API],
        env=reference_environment(),
        capture_output=True,
        check=True,
        timeout=60,
    )
    if meta.stderr:
        raise ValueError("reference metadata wrote stderr")
    api = json.loads(meta.stdout)
    validate_api(api)
    result = (
        _record_trace(options.ruby, selected, work)
        if options.trace
        else {
            "schema": 1,
            "api": api,
            "cases": [record(options.ruby, case, work / case["id"]) for case in selected],
        }
    )
    with output.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(json.dumps(result, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
