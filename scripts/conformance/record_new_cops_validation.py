"""固定版の実RubyからNewCopsの78条件を、全rawと原本属性付きで再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/new_cops_validation.json"
DATA_SHA256 = "8c66c1079c5a378fcde1fbe7a91205121b2d7d51cbf83641eaf11b19c3349ac1"
EXPECTED = {
    "exit",
    "stdout_hex",
    "stderr_hex",
    "out_exists",
    "out_hex",
    "inputs_bytes_mtime_preserved",
}
API = (
    LOADER
    + """
require 'json'
require 'digest'
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
puts JSON.generate(ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |n|
    [n, Gem.loaded_specs.fetch(n).version.to_s] },
  source_sha256: %w[lib/rubocop/config_validator.rb lib/rubocop/config_loader.rb].to_h { |n|
    [n, Digest::SHA256.file(File.join(root,n)).hexdigest] })
"""
)


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


def _template():
    return json.loads(FIXTURE.read_bytes())


def validate_case(row):
    """初期拒否と受理・入れ子の完了reportを分け、空や偽の成功を拒否する。

    Raises:
        ValueError: 終了値、reportのpath/件数/identity又は原本・出力が不正な場合。

    """
    expected_exit = 0 if row["kind"] == "accepted" else 2
    _same(row["exit"], expected_exit)
    _same(row["inputs_bytes_mtime_preserved"], True)
    seed = row["files"].get("report.json")
    _same(row["out_exists"], seed is not None)
    _same(row["out_hex"], None if seed is None else seed.encode().hex())
    stdout, stderr = (bytes.fromhex(row[name + "_hex"]) for name in ("stdout", "stderr"))
    if row["kind"] != "accepted" and not stderr:
        raise ValueError("設定拒否のstderrがありません")
    if row["kind"] == "new-cops-error" and any(
        marker not in stderr
        for marker in (b"Error: invalid ", b"for `NewCops` found in ", b"Valid choices")
    ):
        raise ValueError("別のerrorをNewCops拒否へ数えられません")
    if row["kind"] == "accepted" or row["comparison"] == "full-report":
        report = json.loads(stdout)
        metadata = report["metadata"]
        _same(
            [metadata["ruby_engine"], metadata["ruby_version"], metadata["rubocop_version"]],
            ["ruby", "4.0.7", "1.89.0"],
        )
        inspected = int(row["kind"] == "accepted")
        _same(
            report["summary"],
            {"offense_count": 0, "target_file_count": 1, "inspected_file_count": inspected},
        )
        _same(report["files"], [{"path": "a.rb", "offenses": []}] if inspected else [])
    elif stdout:
        raise ValueError("初期拒否より先にreportを出せません")


def validate_fixture(value):
    """元64のrawと追加14条件・scope・型・出典を固定する。

    Raises:
        ValueError: 固定schema、条件集合、原文又は完了値が変わった場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != DATA_SHA256 or type(value.get("schema")) is not int or value["schema"] != 1:
        raise ValueError("固定78条件の入力・型・raw・scopeが変わりました")
    _same(value["runtime"]["gems"], VERSIONS)
    _same(value["coverage"]["subset"], False)
    if len(value["cases"]) != 78:
        raise ValueError("全78条件が必要です")
    for row in value["cases"]:
        validate_case(row)


def inputs():
    """候補exeを受け取らず、固定78入力の独立copyだけを返す。

    Returns:
        list: 元の順序と比較scopeを保つ入力辞書。

    """
    value = _template()
    validate_fixture(value)
    return [
        {key: copy.deepcopy(item) for key, item in row.items() if key not in EXPECTED}
        for row in value["cases"]
    ]


def child_environment(intentional=None):
    """ENV4とambient NO_COLORを子copyから除き、親・GEM/PATH/OSは保つ。

    Returns:
        dict: TERM=dumbと明示したcase環境を反映した値。

    """
    child = reference_environment()
    for key in list(child):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            child.pop(key)
    child["TERM"] = "dumb"
    child.update(intentional or {})
    return child


def _fresh(work, output):
    if any(os.path.lexists(path) for path in (work, output)):
        raise ValueError("保存先entryは既に存在します")
    first, second = (os.path.normcase(str(path.resolve(strict=False))) for path in (work, output))
    if (
        first == second
        or Path(first) in Path(second).parents
        or Path(second) in Path(first).parents
    ):
        raise ValueError("workとoutの保存先が重なっています")
    if not output.parent.is_dir():
        raise ValueError("outの親directoryがありません")


def _write_json(path, value):
    with path.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def _inventory(root, names):
    result = {}
    for name in names:
        path = root / name
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("原本が通常fileではありません")
        result[name] = {
            "bytes_hex": path.read_bytes().hex(),
            "mtime_ns": info.st_mtime_ns,
            "mode": info.st_mode,
        }
    return result


def _report(root):
    path = root / "report.json"
    if not os.path.lexists(path):
        return None
    return _inventory(root, ["report.json"])["report.json"]


def _prepare(root, files):
    if os.path.lexists(root):
        raise ValueError("child directoryが既に存在します")
    root.mkdir()
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode())


def _capture(ruby, args, root, files, expected_exit, environment=None):
    before = _inventory(root, files)
    report_before = _report(root)
    if report_before is not None and "report.json" not in files:
        raise ValueError("許可していないreport.jsonが存在します")
    done = subprocess.run(
        [str(ruby), *args],
        cwd=root,
        env=child_environment(
            {"CLICOLOR_FORCE": "0", "XDG_CACHE_HOME": str(root / "cache"), **(environment or {})}
        ),
        capture_output=True,
        timeout=60,
        check=False,
    )
    raw = {
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
    }
    _write_json(root / "private-process.json", raw)
    after = _inventory(root, files)
    report_after = _report(root)
    _write_json(
        root / "private-run0.json",
        {
            **raw,
            "inputs_before": before,
            "inputs_after": after,
            "report_before": report_before,
            "report_after": report_after,
        },
    )
    _same(before, after)
    _same(report_before, report_after)
    _same(done.returncode, expected_exit)
    return done, report_after


def record_case(ruby, row, launcher, root):
    """原本と既存sentinelを保持した単一実CLIの全rawを記録する。

    Returns:
        dict: 元fieldsを保ち、実観測で照合した一条件。

    """
    _prepare(root, row["files"])
    done, report = _capture(
        ruby, [str(launcher), *row["args"]], root, row["files"], row["exit"], row["env"]
    )
    actual = {
        **copy.deepcopy(row),
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
        "out_exists": report is not None,
        "out_hex": None if report is None else report["bytes_hex"],
        "inputs_bytes_mtime_preserved": True,
    }
    validate_case(actual)
    _same(actual, row)
    return actual


def collect(ruby, work, output):
    """実API一件とCLI78件の全完了後だけ、新規outをexclusive保存する。

    Returns:
        dict: 型と全rawを保持した完成参照。

    Raises:
        ValueError: 保存先、出典、runtime又は実processの保持検査が異なる場合。

    """
    _fresh(work, output)
    value = _template()
    validate_fixture(value)
    work.mkdir(parents=True)
    root = work / "api-runtime"
    _prepare(root, {})
    done, _ = _capture(ruby, ["-e", API], root, {}, 0)
    if done.stderr:
        raise ValueError("本家APIがstderrへ書きました")
    _same(json.loads(done.stdout), value["runtime"])
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n')
    value["cases"] = [
        record_case(ruby, row, launcher.absolute(), work / row["id"]) for row in value["cases"]
    ]
    validate_fixture(value)
    _write_json(output, value)
    return value


def main():
    """固定版の実Rubyだけを使い、78条件を逐次再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
