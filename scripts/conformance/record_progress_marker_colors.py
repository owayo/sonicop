"""Progressの履歴severity色を、元18条件/19実CLIの全bytesから再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import re
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/progress_marker_colors.json"
RUNTIME = {
    "ruby": "4.0.7",
    "platform": "x64-mingw-ucrt",
    "gems": {
        "parser": "3.3.12.0",
        "prism": "1.8.1",
        "rubocop-ast": "1.50.0",
        "rubocop": "1.89.0",
        "rainbow": "3.1.1",
    },
    "source_sha256": {
        "lib/rubocop/formatter/progress_formatter.rb": "b99dd890265457397ce32830375c2fd15038a70447e03a0f519ec0da74595b98",
        "lib/rubocop/formatter/simple_text_formatter.rb": "e2e21303fb0752dfe15a767757b67109511b0b11ddd28f5f1d5c01c85592ae83",
    },
}
IDS = (
    "info",
    "refactor",
    "convention",
    "warning",
    "error",
    "fatal",
    "clean",
    "corrected-C",
    "mixed-corrected-high",
    "mixed-files",
    "no-color",
    "forced",
    "forced-no-color",
    "stderr",
    "cache",
    "shared-simple",
    "separate-simple",
    "plain",
)
ORIGINAL_FIELDS_SHA256 = "a599411f4ba3f13544eb16ea4cb76dc12c86f55945b159c9d75c9212ab7f1cae"
MARK = re.compile(rb"(?:\x1b\[[0-9;]*m)?[.IRCWEF](?:\x1b\[0m)?")
ANSI = re.compile(rb"\x1b\[[0-9;]*m")
CLI = LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n'
METADATA = (
    LOADER
    + """
require 'json'
require 'digest'
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
paths = JSON.parse(ARGV.fetch(0))
print JSON.generate({ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
 gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |n| [n, Gem.loaded_specs.fetch(n).version.to_s] },
 source_sha256: paths.to_h { |p| [p, Digest::SHA256.file(File.join(root,p)).hexdigest] }})
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


def progress_bytes(case, run):
    """測定対象のprogress streamを、stdout/stderr/Fileから選ぶ。

    Returns:
        bytes: 全text modeとANSIを保持した実bytes。

    """
    stream = case["progress_stream"]
    return bytes.fromhex(
        run[stream + "_hex"] if stream in ("stdout", "stderr") else run["artifacts_hex"][stream]
    )


def _marks(case, run):
    raw = progress_bytes(case, run)
    count = len(case["targets"])
    unit = "file" if count == 1 else "files"
    header = f"Inspecting {count} {unit}\r\n".encode()
    if not raw.startswith(header) or f"{count} {unit} inspected".encode() not in ANSI.sub(b"", raw):
        raise ValueError("対象数又は検査完了が不正です")
    rest = raw[len(header) :]
    marks = []
    for _ in case["targets"]:
        match = MARK.match(rest)
        if match is None:
            raise ValueError("progress markerがありません")
        marks.append(match.group())
        rest = rest[match.end() :]
    return b"".join(marks).hex()


def validate_fixture(value):
    """元18条件/19実行の型・全入力/rawと実版/source集合を固定する。

    Raises:
        ValueError: schema、出典、条件集合、run又は元rawが変わった場合。

    """
    if (
        set(value) != {"schema", "runtime", "cases"}
        or type(value.get("schema")) is not int
        or value["schema"] != 1
    ):
        raise ValueError("unsupported schema")
    _same(value.get("runtime"), RUNTIME)
    _same(value["runtime"]["gems"], VERSIONS)
    rows = value.get("cases", [])
    if not isinstance(rows, list) or tuple(row.get("id") for row in rows) != IDS:
        raise ValueError("固定18ID/順序が必要です")
    core = {"runtime": value["runtime"], "cases": []}
    for row in rows:
        count = 2 if row["id"] == "cache" else 1
        if (
            row.get("coverage") != "full_bytes"
            or type(row.get("run_count")) is not int
            or row["run_count"] != count
            or len(row["runs"]) != count
        ):
            raise ValueError("比較範囲又は19実行が変わりました")
        for run in row["runs"]:
            if type(run.get("exit")) is not int or run["exit"] not in (0, 1):
                raise ValueError("終了値の型又は値が不正です")
            _same(_marks(row, run), run["marks_hex"])
        core["cases"].append({k: v for k, v in row.items() if k != "coverage"})
    digest = hashlib.sha256(
        json.dumps(core, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != ORIGINAL_FIELDS_SHA256:
        raise ValueError("元参照の入力/rawが変わりました")


def _artifacts(root):
    path = root / "progress.txt"
    if not os.path.lexists(path):
        return {}
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or path.is_symlink():
        raise ValueError("出力先に通常file以外のentryができました")
    return _inventory(root, ["progress.txt"])


def record(ruby, case, root, launcher):
    """実runを順次観測し、補正bytesと未変更入力statを分けて保持する。

    Returns:
        dict: 元全fieldを保持した一条件の観測。

    Raises:
        ValueError: 未知入力、保持、出力先又は参照の契約を満たさない場合。

    """
    fixture = _template()
    validate_fixture(fixture)
    expected = [row for row in fixture["cases"] if row["id"] == case.get("id")]
    if len(expected) != 1:
        raise ValueError("unknown case")
    _same(case, expected[0])
    if os.path.lexists(root):
        raise ValueError("case directoryはfreshである必要があります")
    root.mkdir(parents=True)
    for name, text in case["files"].items():
        (root / name).write_bytes(text.encode())
    environment = child_environment({**case["env"], "XDG_CACHE_HOME": str(root / "cache")})
    runs = []
    for index, expected_run in enumerate(case["runs"]):
        before = _inventory(root, case["files"])
        artifacts_before = _artifacts(root)
        done = subprocess.run(
            [str(ruby), str(launcher), *case["args"]],
            cwd=root,
            env=environment,
            capture_output=True,
            timeout=60,
            check=False,
        )
        after = _inventory(root, case["files"])
        artifacts = _artifacts(root)
        run = {
            "exit": done.returncode,
            "stdout_hex": done.stdout.hex(),
            "stderr_hex": done.stderr.hex(),
            "artifacts_hex": {name: info["bytes_hex"] for name, info in artifacts.items()},
            "after_hex": {name: info["bytes_hex"] for name, info in after.items()},
        }
        _write_json(
            root / f"private-run{index}.json",
            {
                **run,
                "inputs_before": before,
                "inputs_after": after,
                "artifacts_before": artifacts_before,
                "artifacts_after": artifacts,
            },
        )
        for name in before:
            if before[name]["mode"] != after[name]["mode"]:
                raise ValueError("原本modeが変わりました")
            if before[name]["bytes_hex"] == expected_run["after_hex"][name]:
                _same(before[name], after[name])
        run["marks_hex"] = _marks(case, run)
        _same(run, expected_run)
        runs.append(run)
    row = copy.deepcopy(case)
    row["runs"] = runs
    return row


def collect(ruby, work, output):
    """固定19実CLIとruntimeの完了後だけ、公開先をexclusive作成する。

    Raises:
        ValueError: 保存先、実版、出典又は観測が契約を満たさない場合。

    """
    _fresh(work, output)
    value = _template()
    validate_fixture(value)
    work.mkdir(parents=True)
    done = subprocess.run(
        [str(ruby), "-e", METADATA, json.dumps(list(RUNTIME["source_sha256"]))],
        cwd=work,
        env=child_environment(),
        capture_output=True,
        timeout=60,
        check=False,
    )
    _write_json(
        work / "private-runtime.json",
        {"exit": done.returncode, "stdout_hex": done.stdout.hex(), "stderr_hex": done.stderr.hex()},
    )
    if done.returncode != 0 or done.stderr:
        raise ValueError("runtime照会に失敗しました")
    _same(json.loads(done.stdout), RUNTIME)
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(CLI)
    value["cases"] = [
        record(ruby, row, work / row["id"], launcher.absolute()) for row in value["cases"]
    ]
    _write_json(output, value)


def main():
    """候補を受け取らず、版を固定した本家だけを採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
