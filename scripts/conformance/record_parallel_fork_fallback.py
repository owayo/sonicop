"""Windowsのfork通知25条件を、固定版の実Rubyから全streamと属性付きで再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER as RUBOCOP_LOADER
from record_config_cop_namespaces import VERSIONS as RUBOCOP_VERSIONS
from record_config_cop_namespaces import reference_environment

VERSIONS = {**RUBOCOP_VERSIONS, "parallel": "2.3.0"}
LOADER = "gem 'parallel', '2.3.0'\n" + RUBOCOP_LOADER
LOADER += "\nraise 'unexpected parallel' unless Gem.loaded_specs.fetch('parallel').version.to_s == '2.3.0'\n"
NOTICE = b"parallel: Process.fork is not supported by this Ruby\r\n"
FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/parallel_fork_fallback.json"
DATA_SHA256 = "3d5a922983de728c7f488bea668bdb4a93f3879c94be353971318fb921f7a37b"
IO_IDS = {"openfail-active", "multi-out-openfail"}

API = (
    LOADER
    + """
require 'json'
require 'digest'
puts JSON.generate(runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  fork_supported: Process.respond_to?(:fork),
  gems: %w[parser prism rubocop-ast rubocop rainbow parallel].to_h {|name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]}},
  source_sha256: {
    'rubocop/runner.rb' => Digest::SHA256.file(File.join(Gem.loaded_specs.fetch('rubocop').full_gem_path, 'lib/rubocop/runner.rb')).hexdigest,
    'parallel.rb' => Digest::SHA256.file(File.join(Gem.loaded_specs.fetch('parallel').full_gem_path, 'lib/parallel.rb')).hexdigest,
    'parallel/MIT-LICENSE.txt' => Digest::SHA256.file(File.join(Gem.loaded_specs.fetch('parallel').full_gem_path, 'MIT-LICENSE.txt')).hexdigest})
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


def _fresh(work, output):
    for path in (work, output):
        if os.path.lexists(path):
            raise ValueError("保存先entryは既に存在します")
    first, second = (os.path.normcase(str(path.resolve(strict=False))) for path in (work, output))
    if (
        first == second
        or Path(first) in Path(second).parents
        or Path(second) in Path(first).parents
    ):
        raise ValueError("workとoutputの保存先が重なっています")


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


def _template():
    return json.loads(FIXTURE.read_bytes())


def child_environment(intentional=None):
    """ENV4とambient NO_COLORを子copyで除去し、GEM/PATH/OSは保つ。

    Returns:
        dict: TERM=dumbとcase固有の色/cacheを反映した環境。

    """
    child = reference_environment()
    for key in list(child):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            child.pop(key)
    child["TERM"] = "dumb"
    child.update(intentional or {})
    return child


def _reports(row):
    expected = row["expected"]
    streams = [expected["stdout_hex"], expected["stderr_hex"], *expected["outputs_hex"].values()]
    reports = []
    for raw in streams:
        if raw is None:
            continue
        text = bytes.fromhex(raw).decode()
        position = text.find('{"metadata":')
        if position >= 0:
            value, _ = json.JSONDecoder().raw_decode(text[position:])
            reports.append(value)
    return reports


def validate(row):
    """通知と途中終了の許可集合を固定し、空・重複・不正な完了を拒否する。

    Raises:
        ValueError: 終了値、通知、identity、path集合又は完了数が実録と異なる場合。

    """
    expected = row["expected"]
    if type(expected["exit"]) is not int or expected["exit"] != row["expected_exit"]:
        raise ValueError("終了値の型又は値が不正です")
    stdout = bytes.fromhex(expected["stdout_hex"])
    stderr = bytes.fromhex(expected["stderr_hex"] or "")
    if stderr.count(NOTICE) != int(row["expect_notice"]) or NOTICE in stdout:
        raise ValueError("fork通知の回数又はchannelが異なります")
    if row["kind"] == "progress" and not stderr.startswith(
        b"Inspecting 2 files\r\n" + NOTICE + b"CC\r\n"
    ):
        raise ValueError("started/通知/fileの順序が異なります")
    if row["kind"] == "notice-json":
        flag = b"-F/--fail-fast" if row["id"] == "fail-fast" else b"--cache false"
        prefix = (
            b"-P/--parallel is being ignored because it is not compatible with " + flag + b".\r\n"
        )
        if not stdout.startswith(prefix) or stdout.count(prefix) != 1:
            raise ValueError("既存135通知が先頭に一度だけ必要です")
    errors = {
        "unknown-selection": NOTICE + b"Unrecognized cop or department: NoSuchCop.\r\n",
        "root-config-error": b"Error: unrecognized cop or department NoSuchCop found in .rubocop.yml\r\n",
        "late-config-error": NOTICE
        + b"Error: unrecognized cop or department NoSuchCop found in sub/.rubocop.yml\r\n",
    }
    if row["id"] in errors and stderr != errors[row["id"]]:
        raise ValueError("通知と設定拒否の段階が異なります")
    reports = _reports(row)
    if row["id"] in {"root-config-error", "list-target-files", "progress-stderr", *IO_IDS}:
        if reports:
            raise ValueError("reportのないphaseへJSONを混ぜられません")
        return
    if len(reports) != 1:
        raise ValueError("一つの完了reportが必要です")
    report = reports[0]
    metadata = report["metadata"]
    if (metadata["ruby_engine"], metadata["ruby_version"], metadata["rubocop_version"]) != (
        "ruby",
        "4.0.7",
        "1.89.0",
    ):
        raise ValueError("本家identityが異なります")
    paths, count, offenses = ["good.rb", "last.rb"], 2, 2
    if row["id"] == "zero-targets":
        paths, count, offenses = [], 0, 0
    elif row["id"] in {"one-target", "duplicate-target", "stdin"}:
        paths, count, offenses = ["good.rb"], 1, 1
    elif row["id"] in {"fail-fast", "late-config-error"}:
        paths, offenses = ["good.rb"], 1
    elif row["id"] == "unknown-selection":
        paths, offenses = [], 0
    elif row["id"] == "only-syntax":
        offenses = 0
    if [file["path"] for file in report["files"]] != paths:
        raise ValueError("実検査prefix又は対象pathが異なります")
    summary = report["summary"]
    if any(
        type(summary.get(key)) is not int
        for key in ("offense_count", "target_file_count", "inspected_file_count")
    ):
        raise ValueError("summaryの件数は整数である必要があります")
    _same(
        summary,
        {"offense_count": offenses, "target_file_count": count, "inspected_file_count": len(paths)},
    )
    if sum(len(file["offenses"]) for file in report["files"]) != offenses:
        raise ValueError("offense集合が完了数と異なります")


def validate_fixture(value):
    """元25条件の入力・全raw・出典・比較scopeを固定する。

    Raises:
        ValueError: 固定schema、条件集合、型、出典又は完了値が変わった場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != DATA_SHA256 or type(value.get("schema")) is not int or value["schema"] != 1:
        raise ValueError("固定25条件の入力・型・rawが変わりました")
    _same(value["runtime"]["gems"], VERSIONS)
    if (
        value["runtime"]["fork_supported"] is not False
        or value["coverage"]["subset"] is not False
        or len(value["cases"]) != 25
    ):
        raise ValueError("Windows fork不可の全25条件が必要です")
    for row in value["cases"]:
        validate(row)


def inputs():
    """固定25入力を独立copyで返し、候補exeを受け取らない。

    Returns:
        list: 元の順序を保つ入力辞書。

    """
    value = _template()
    validate_fixture(value)
    return [
        {key: copy.deepcopy(item) for key, item in row.items() if key != "expected"}
        for row in value["cases"]
    ]


def _artifacts(root, names):
    result = {}
    for name in names:
        path = root / name
        result[name] = _inventory(root, [name])[name] if os.path.lexists(path) else None
    return result


def _unexpected_report(root):
    path = root / "report.json"
    if not os.path.lexists(path):
        return {"exists": False}
    info = path.lstat()
    return {"exists": True, "mode": info.st_mode, "size": info.st_size}


def _prepare(root, files):
    if os.path.lexists(root):
        raise ValueError("child directoryが既に存在します")
    root.mkdir()
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode())


def _capture(ruby, arguments, root, files, names, expected_exit, intentional=None, stdin=None):
    before = _inventory(root, files)
    artifacts_before = _artifacts(root, names)
    out_before = _unexpected_report(root)
    done = subprocess.run(
        [str(ruby), *arguments],
        cwd=root,
        env=child_environment(
            {"CLICOLOR_FORCE": "0", "XDG_CACHE_HOME": str(root / "cache"), **(intentional or {})}
        ),
        capture_output=True,
        timeout=60,
        check=False,
        input=stdin,
    )
    raw = {
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
    }
    _write_json(root / "private-process.json", raw)
    after = _inventory(root, files)
    artifacts_after = _artifacts(root, names)
    out_after = _unexpected_report(root)
    _write_json(
        root / "private-run0.json",
        {
            **raw,
            "inputs_before": before,
            "inputs_after": after,
            "artifacts_before": artifacts_before,
            "artifacts_after": artifacts_after,
            "out_before": out_before,
            "out_after": out_after,
        },
    )
    _same(before, after)
    _same(out_before, {"exists": False})
    _same(out_after, {"exists": False})
    _same(done.returncode, expected_exit)
    return done, artifacts_after


def record_case(ruby, row, launcher, root):
    """単一の実CLIを記録し、IO二件のbacktraceは私用rawだけへ保持する。

    Returns:
        dict: 元の全fieldsとrawを保持した一条件。

    Raises:
        ValueError: 原本、出力、実終了値又は通知phaseが異なる場合。

    """
    _prepare(root, row["files"])
    for name in ("empty", "outdir"):
        (root / name).mkdir()
    done, artifacts = _capture(
        ruby,
        [str(launcher), *row["args"]],
        root,
        row["files"],
        row["outputs"],
        row["expected_exit"],
        row["env"],
        None if row["stdin_hex"] is None else bytes.fromhex(row["stdin_hex"]),
    )
    if row["id"] in IO_IDS and (
        not done.stderr.startswith(b"Is a directory @ rb_sysopen - outdir\r\n")
        or NOTICE in done.stderr
    ):
        raise ValueError("出力open以外の例外をIO対照へ混ぜられません")
    actual = copy.deepcopy(row)
    actual["expected"] = {
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": None if row["id"] in IO_IDS else done.stderr.hex(),
        "outputs_hex": {
            name: state["bytes_hex"] for name, state in artifacts.items() if state is not None
        },
        "source_bytes_mtime_unchanged": True,
    }
    validate(actual)
    _same(actual, row)
    return actual


def record(ruby, output, work):
    """固定API一件とCLI25件の全完了後だけ、新規outをexclusive保存する。

    Returns:
        dict: 元のscopeを保持した全条件。

    Raises:
        ValueError: 保存先、型、出典、実process又は保持検査が異なる場合。

    """
    _fresh(work, output)
    if not output.parent.is_dir():
        raise ValueError("outの親directoryがありません")
    value = _template()
    validate_fixture(value)
    work.mkdir(parents=True)
    root = work / "api-runtime"
    _prepare(root, {})
    done, _ = _capture(ruby, ["-e", API], root, {}, (), 0)
    if done.stderr:
        raise ValueError("本家APIがstderrへ書きました")
    _same(
        json.loads(done.stdout),
        {"runtime": value["runtime"], "source_sha256": value["source_sha256"]},
    )
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n")
    value["cases"] = [
        record_case(ruby, row, launcher.absolute(), work / row["id"]) for row in value["cases"]
    ]
    validate_fixture(value)
    _write_json(output, value)
    return value


def main():
    """候補を実行せず、固定版Windows Rubyの全条件だけを再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    record(args.ruby, args.output, args.work_dir)


if __name__ == "__main__":
    main()
