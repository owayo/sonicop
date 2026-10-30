"""本家の設定cache21条件を各2回採取し、全reportとcache entryの有無を検査する。"""

import argparse
import copy
import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, reference_environment

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/configured_result_cache.json"
DATA_SHA256 = "e81c43b85f17f8ada17c3e160c6b8db7f2c682959167125db00cbc697a54444b"
CLI = LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n"
API = (
    LOADER
    + """
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
names = %w[lib/rubocop/runner.rb lib/rubocop/cli.rb lib/rubocop/options.rb
  lib/rubocop/config_store.rb lib/rubocop/config_loader.rb lib/rubocop/config_validator.rb
  config/default.yml LICENSE.txt]
print JSON.generate(runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[psych parser prism rubocop-ast rubocop rainbow].to_h {|name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]}},
  source_sha256: names.to_h {|name|
    [name, Digest::SHA256.file(File.join(spec.full_gem_path, name)).hexdigest]})
"""
)


def _template():
    return json.loads(FIXTURE.read_bytes())


def _same(actual, expected):
    if type(actual) is not type(expected):
        raise ValueError("参照の型が変わりました")
    if isinstance(actual, dict):
        if actual.keys() != expected.keys():
            raise ValueError("参照のfield集合が変わりました")
        for key in actual:
            _same(actual[key], expected[key])
    elif isinstance(actual, list):
        if len(actual) != len(expected):
            raise ValueError("参照の件数が変わりました")
        for left, right in zip(actual, expected, strict=True):
            _same(left, right)
    elif actual != expected:
        raise ValueError("参照値が変わりました")


def validate_case(row):
    """原参照の全raw・保持・cache有無を固定し、各回の検査完了も要求する。

    Raises:
        ValueError: ID、型、入力、全report又は保持・cache観測が変わった場合。

    """
    expected = next((item for item in _template()["cases"] if item["id"] == row["id"]), None)
    _same(row, expected)
    for result in row["results"]:
        _same(result["exit"], 1)
        _same(result["stderr_hex"], "")
        _same(result["source_config_bytes_mtime_mode_equal"], True)
        _same(result["report_exists"], False)
        stdout = bytes.fromhex(result["stdout_hex"])
        if not stdout.endswith(b"}"):
            raise ValueError("compact reportが完了していません")
        report = json.loads(stdout[stdout.index(b"{") :])
        _same(report["metadata"]["ruby_engine"], "ruby")
        _same(report["metadata"]["rubocop_version"], "1.89.0")
        _same(
            report["summary"],
            {
                "offense_count": sum(len(file["offenses"]) for file in report["files"]),
                "target_file_count": 1,
                "inspected_file_count": 1,
            },
        )
        if not report["summary"]["offense_count"]:
            raise ValueError("診断を一度も観測していません")
        _same(result["cache_regular_files"], int(result["cache_exists"]))


def validate_fixture(value):
    """固定21入力・実source8・42回の全観測を一緒に検査する。

    Raises:
        ValueError: 型、出典、入力、順序又は観測の一部が変わった場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定21条件の型・入力・出典・全観測が変わりました")
    _same(value["schema"], 1)
    _same(value["coverage"]["actual_cli_cases"], 21)
    _same(value["coverage"]["actual_cli_invocations"], 42)
    _same(value["coverage"]["native_invocations"], 0)
    _same(value["coverage"]["subset"], False)
    _same(len(value["cases"]), 21)
    for row in value["cases"]:
        _same(len(row["results"]), 2)
        validate_case(row)


def _environment(root):
    child = reference_environment()
    for key in list(child):
        if key.upper() in {"RUBOCOP_CACHE_ROOT", "NO_COLOR", "SONICOP_PROFILE"}:
            child.pop(key)
    child.update({"XDG_CACHE_HOME": str(root / "cache"), "CLICOLOR_FORCE": "0"})
    return child


def _write_json(path, value):
    with path.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def _inventory(root, files):
    result = {}
    for name in files:
        path = root / name
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError("原本が通常fileではありません")
        result[name] = {
            "bytes_hex": path.read_bytes().hex(),
            "mtime_ns": info.st_mtime_ns,
            "mode": info.st_mode,
        }
    return result


def _entry(path):
    if not os.path.lexists(path):
        return None
    info = path.lstat()
    # 原本以外のentryは本文を読まず記録し、dangling junctionも不生成扱いにしない。
    return {
        "mode": info.st_mode,
        "mtime_ns": info.st_mtime_ns,
        "file_attributes": getattr(info, "st_file_attributes", None),
        "reparse_tag": getattr(info, "st_reparse_tag", None),
    }


def _cache_state(root):
    path = root / "cache"
    if not os.path.lexists(path):
        return {"exists": False, "regular_files": 0}
    if not stat.S_ISDIR(path.lstat().st_mode):
        raise ValueError("cache rootがdirectoryではありません")
    stack, count = [path], 0
    while stack:
        current = stack.pop()
        info = current.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError("cacheのlinkをたどれません")
        if stat.S_ISDIR(info.st_mode):
            stack.extend(current.iterdir())
        elif stat.S_ISREG(info.st_mode):
            count += 1
        else:
            raise ValueError("cacheに未対応のentryがあります")
    return {"exists": True, "regular_files": count}


def _capture(ruby, root, files, code, args, repeat):
    before = _inventory(root, files)
    report_before = _entry(root / "report.json")
    if report_before is not None:
        raise ValueError("report entryは最初から存在します")
    cache_before = _cache_state(root)
    done = subprocess.run(
        [str(ruby), "-e", code, "--", *args],
        cwd=root,
        env=_environment(root),
        capture_output=True,
        timeout=60,
        check=False,
    )
    raw = {
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
    }
    _write_json(root / f"private-process-{repeat}.json", raw)
    after = _inventory(root, files)
    report_after = _entry(root / "report.json")
    cache_after = _cache_state(root)
    _write_json(
        root / f"private-run-{repeat}.json",
        {
            **raw,
            "inputs_before": before,
            "inputs_after": after,
            "report_before": report_before,
            "report_after": report_after,
            "cache_before": cache_before,
            "cache_after": cache_after,
        },
    )
    _same(before, after)
    _same(report_after, None)
    return {
        **raw,
        "cache_exists": cache_after["exists"],
        "cache_regular_files": cache_after["regular_files"],
        "source_config_bytes_mtime_mode_equal": True,
        "report_exists": False,
    }


def record_case(ruby, root, row):
    """本家CLIを同じ入力で2回動かし、各回の原本とcache有無を保持する。

    Returns:
        dict: 2回の全rawと、入力・比較scopeを保つ実参照。

    """
    root.mkdir()
    for name, text in row["files"].items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(text.encode())
    actual = copy.deepcopy(row)
    actual["results"] = [
        _capture(ruby, root, row["files"], CLI, row["args"], repeat) for repeat in range(2)
    ]
    validate_case(actual)
    return actual


def collect(ruby, work, output):
    """実source照会1回とCLI42回が完了した後だけ、完成JSONをexclusive保存する。

    Returns:
        dict: 実source、全raw、原本保持とcacheの有無を記録した固定21条件。

    Raises:
        ValueError: 保存先entry/重複、出典、型、保持又は実観測が不正な場合。

    """
    if any(os.path.lexists(path) for path in (work, output)):
        raise ValueError("保存先entryは既に存在します")
    first, second = (Path(os.path.normcase(str(path.resolve()))) for path in (work, output))
    if first == second or first in second.parents or second in first.parents:
        raise ValueError("workとoutの保存先が重なっています")
    if not output.parent.is_dir():
        raise ValueError("outの親directoryがありません")
    value = _template()
    validate_fixture(value)
    work.mkdir(parents=True)
    runtime = work / "runtime"
    runtime.mkdir()
    result = _capture(ruby, runtime, {}, API, [], 0)
    _same(result["exit"], 0)
    _same(result["stderr_hex"], "")
    _same(result["cache_exists"], False)
    _same(json.loads(bytes.fromhex(result["stdout_hex"])), value["primary"])
    value["cases"] = [record_case(ruby, work / row["id"], row) for row in value["cases"]]
    validate_fixture(value)
    _write_json(output, value)
    return value


def main():
    """候補exeを受け取らず、固定Windows参照の全21条件を再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
