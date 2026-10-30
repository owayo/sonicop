"""固定版Rubyからcache/parallelの全34条件を再採取し、未完了の記録を拒否する。"""

import argparse
import copy
import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/cache_parallel_preflight.json"
DATA_SHA256 = "7e2d03f16e5b6506fc417cd798826b351d76025e00d907e492b34474115e563d"
EXPECTED = {"exit", "stdout_hex", "stderr_hex", "source_config_bytes_mtime_equal", "out_exists"}
MESSAGE = (
    b"-P/--parallel uses caching to speed up execution, so combining with "
    b"AllCops: UseCache: false is not allowed."
)
REFERENCE_LOADER = "gem 'parallel', '2.3.0'\n" + LOADER
API = (
    REFERENCE_LOADER
    + """
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
parallel = Gem.loaded_specs.fetch('parallel')
puts JSON.generate(runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h {|name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]}},
  source_sha256: %w[lib/rubocop/cli.rb lib/rubocop/options.rb LICENSE.txt].to_h {|name|
    [name, Digest::SHA256.file(File.join(spec.full_gem_path, name)).hexdigest]},
  parallel: {version: parallel.version.to_s, source_sha256:
    %w[lib/parallel.rb MIT-LICENSE.txt].to_h {|name|
      [name, Digest::SHA256.file(File.join(parallel.full_gem_path, name)).hexdigest]}})
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


def _reference_row(name):
    return next(row for row in _template()["cases"] if row["id"] == name)


def validate_case(row):
    """初期拒否・既存通知・早期終了と、非空の完了reportを区別する。

    Raises:
        ValueError: 型、phase、全raw、report又は原本・出力状態が不正な場合。

    """
    _same(row["source_config_bytes_mtime_equal"], True)
    _same(row["out_exists"], "report.json" in row["files"])
    stdout, stderr = (bytes.fromhex(row[name + "_hex"]) for name in ("stdout", "stderr"))
    kind = row["kind"]
    if kind in ("rejection", "invalid-cache", "unknown-config"):
        _same(row["exit"], 2)
        _same(stdout, b"")
        if kind == "rejection":
            message = MESSAGE
            if row["id"] in ("color", "force-color"):
                message = b"\x1b[31m" + message + b"\x1b[0m"
        elif kind == "invalid-cache":
            message = b"-C/--cache argument must be true or false"
        else:
            message = b"Error: unrecognized cop or department Unknown/Cop found in .rubocop.yml"
        _same(stderr, message + b"\r\n")
        return
    _same(row["exit"], 1 if kind in ("report", "notice-report") else 0)
    if kind == "pending-report":
        _same(stderr, bytes.fromhex(_reference_row("pending-true")["stderr_hex"]))
    else:
        _same(stderr, b"")
    if kind == "notice-report":
        flag = b"--cache false" if row["id"] == "cache-false-cancels" else b"-F/--fail-fast"
        notice = (
            b"-P/--parallel is being ignored because it is not compatible with " + flag + b".\r\n"
        )
        _same(stdout[: len(notice)], notice)
        stdout = stdout[len(notice) :]
    if kind in ("report", "notice-report", "pending-report"):
        if not stdout.endswith(b"}"):
            raise ValueError("compact JSONが完了していないか、余分な終端改行があります")
        report = json.loads(stdout)
        _same(
            report["metadata"],
            {
                "rubocop_version": "1.89.0",
                "ruby_engine": "ruby",
                "ruby_version": "4.0.7",
                "ruby_patchlevel": "0",
                "ruby_platform": "x64-mingw-ucrt",
            },
        )
        pending = kind == "pending-report"
        _same(
            report["summary"],
            {
                "offense_count": 0 if pending else 1,
                "target_file_count": 2 if pending else 1,
                "inspected_file_count": 2 if pending else 1,
            },
        )
        # 本家の実1件の全offense fieldsを使い、件数だけの偽成功を許さない。
        expected = json.loads(bytes.fromhex(_reference_row("true-parallel-one")["stdout_hex"]))
        _same(
            report["files"],
            [{"path": name, "offenses": []} for name in ("good.rb", "last.rb")]
            if pending
            else expected["files"],
        )
    elif kind == "version":
        _same(stdout, b"1.89.0\r\n")
    elif kind in ("verbose-version", "text"):
        _same(stdout, bytes.fromhex(_reference_row(row["id"])["stdout_hex"]))
    else:
        raise ValueError("未知の比較scopeです")


def validate_fixture(value):
    """元28条件の全rawと、追加したpending順序・truthy六条件を固定する。

    Raises:
        ValueError: schema、型、条件集合、出典又は完了値が変わった場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != DATA_SHA256 or type(value.get("schema")) is not int or value["schema"] != 1:
        raise ValueError("固定34条件の入力・型・全raw・scopeが変わりました")
    _same(value["runtime"]["gems"], VERSIONS)
    _same(value["coverage"]["subset"], False)
    _same(value["coverage"]["actual_cli_cases"], 34)
    if len(value["cases"]) != 34:
        raise ValueError("全34条件が必要です")
    for row in value["cases"]:
        validate_case(row)


def inputs():
    """候補exeを受け取らず、固定34入力の独立copyを返す。

    Returns:
        list: 元の順序を保つ実入力と比較scope。

    """
    value = _template()
    validate_fixture(value)
    return [
        {key: copy.deepcopy(item) for key, item in row.items() if key not in EXPECTED}
        for row in value["cases"]
    ]


def child_environment(intentional=None):
    """ENV4とambient NO_COLORだけを子copyから除き、親・GEM/PATH/OSを保つ。

    Returns:
        dict: TERMと意図した条件を明示した子環境。

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
    if not os.path.lexists(root / "report.json"):
        return None
    return _inventory(root, ["report.json"])["report.json"]


def _prepare(root, files):
    if os.path.lexists(root):
        raise ValueError("child directoryが既に存在します")
    root.mkdir()
    for name, text in files.items():
        (root / name).write_bytes(text.encode())


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
    """実CLI一件の全rawと、原本・既存sentinelのbytes/mtime/modeを記録する。

    Returns:
        dict: 元fieldsを保ち、実観測で照合した一条件。

    """
    _prepare(root, row["files"])
    for name in ("empty", "outdir"):
        (root / name).mkdir()
    done, report = _capture(
        ruby, [str(launcher), *row["args"]], root, row["files"], row["exit"], row["env"]
    )
    actual = {
        **copy.deepcopy(row),
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
        "out_exists": report is not None,
        "source_config_bytes_mtime_equal": True,
    }
    validate_case(actual)
    _same(actual, row)
    return actual


def collect(ruby, work, output):
    """実API一件とCLI34件の全完了後だけ、完成outをexclusive保存する。

    Returns:
        dict: 全raw、型、出典と有限scopeを保つ参照。

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
    _same(
        json.loads(done.stdout),
        {key: value[key] for key in ("runtime", "source_sha256", "parallel")},
    )
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(REFERENCE_LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n')
    value["cases"] = [
        record_case(ruby, row, launcher.absolute(), work / row["id"]) for row in value["cases"]
    ]
    validate_fixture(value)
    _write_json(output, value)
    return value


def main():
    """候補を使わず、固定Windows参照の全34条件を逐次再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
