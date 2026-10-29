"""一覧の実IO bytesを固定19条件のRuboCop CLIだけから再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/disk_listing.json"
IDS = (
    "disk-one",
    "explicit-reverse",
    "unicode",
    "directory",
    "short-flag",
    "format-out",
    "mixed-formats-out",
    "stderr-option",
    "redirect-files",
    "readonly-correction-mode",
    "empty-directory",
    "invalid-cache",
    "json-control",
    "stdin-directory-list",
    "stdin-json-correction",
    "existing-out",
    "stdin-directory-stderr",
    "rejected-disk-stderr",
    "rejected-directory-stderr",
)
LISTED_PATHS = {
    "disk-one": ["good.rb"],
    "explicit-reverse": ["second.rb", "good.rb"],
    "unicode": ["日本語.rb"],
    "directory": ["dir/a.rb", "dir/z.rb"],
    "short-flag": ["good.rb"],
    "format-out": ["good.rb"],
    "mixed-formats-out": ["good.rb"],
    "stderr-option": ["good.rb"],
    "redirect-files": ["good.rb"],
    "readonly-correction-mode": ["good.rb"],
    "empty-directory": [],
    "stdin-directory-list": ["dir/a.rb"],
    "existing-out": ["good.rb"],
    "stdin-directory-stderr": ["dir/a.rb"],
    "rejected-disk-stderr": ["good.rb"],
    "rejected-directory-stderr": ["dir/a.rb"],
}
PRIMARY = {"ruby": "4.0.7", "platform": "x64-mingw-ucrt", "gems": VERSIONS}
SOURCES = {
    "runner.rb": "d9772563cd45015aaff5ad893fce2bb37ce32bd587e82956006ccaa86086f122",
    "cli.rb": "7a097a2918f7bf4b03b71a49d868910f243b78aa26e31035520822709d33e933",
    "cli/command/execute_runner.rb": "57bd0f7c5386465397570b06bcbadd65dbdb8be79900adc70b2a7d9ba4edcdaa",
    "cli/command/list_enabled_cops_for.rb": "f1b4b5c544ad693b4caac80b677f9555ea259de35371f7e980b4f9826046f074",
}
ORIGINAL_FIELDS_SHA256 = "9e533b16d437276e4a3537b8597bfa14bf2c87ebc83fccacbea770e08a384374"
CLI = LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n"
METADATA = (
    LOADER
    + """
require 'json'
require 'digest'
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
names = JSON.parse(ARGV.fetch(0))
print JSON.generate({primary: {
  ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |n| [n, Gem.loaded_specs.fetch(n).version.to_s] }
}, source_sha256: names.to_h { |n| [n, Digest::SHA256.file(File.join(root, 'lib/rubocop', n)).hexdigest] }})
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


def child_environment():
    """ENV4とambient NO_COLORを子copyから除き、色とTERMを測定入力に固定する。

    Returns:
        dict: GEM/PATH/OSと親環境を保つ子環境。

    """
    result = reference_environment()
    for key in list(result):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            result.pop(key)
    result.update(CLICOLOR_FORCE="0", TERM="dumb")
    return result


def split_report(raw):
    """完了した一対象JSONの全fieldと、生のstdin suffixを分離する。

    Returns:
        dict: metadata以外のreportとsuffix_hex。

    Raises:
        ValueError: JSON、実版又は完了件数が不正な場合。

    """
    text = raw.decode("utf8")
    value, end = json.JSONDecoder().raw_decode(text)
    if not isinstance(value, dict):
        raise ValueError("reportはobjectである必要があります")
    metadata = value.get("metadata", {})
    if metadata.get("ruby_engine") != "ruby" or metadata.get("rubocop_version") != "1.89.0":
        raise ValueError("参照の実版/engineが違います")
    summary = value.get("summary", {})
    files = value.get("files", [])
    if (
        not isinstance(files, list)
        or len(files) != 1
        or not isinstance(files[0].get("offenses"), list)
        or files[0].get("path") not in ("good.rb", "virtual.rb")
        or any(
            type(summary.get(key)) is not int
            for key in ("offense_count", "target_file_count", "inspected_file_count")
        )
        or summary["target_file_count"] != 1
        or summary["inspected_file_count"] != 1
        or summary["offense_count"] != len(files[0]["offenses"])
        or summary["offense_count"] != 1
    ):
        raise ValueError("reportの対象/完了件数が不正です")
    value.pop("metadata")
    return {"report": value, "suffix_hex": raw[len(text[:end].encode()) :].hex()}


def validate_fixture(value):
    """元19入力/rawとschema/source集合を固定し、欠落や別条件への差替えを拒む。

    Raises:
        ValueError: 型、件数、入力、raw又は出典が変わった場合。

    """
    if set(value) != {"schema", "primary", "source_sha256", "cases"}:
        raise ValueError("schemaのfield集合が変わりました")
    if type(value.get("schema")) is not int or value["schema"] != 1:
        raise ValueError("unsupported schema")
    _same(value.get("primary"), PRIMARY)
    _same(value.get("source_sha256"), SOURCES)
    rows = value.get("cases", [])
    if not isinstance(rows, list):
        raise ValueError("casesはlistである必要があります")
    if tuple(row.get("id") for row in rows) != IDS:
        raise ValueError("固定19ID/順序が必要です")
    original = {"primary": value["primary"], "cases": []}
    for row in rows:
        if row.get("coverage") != "full_bytes" or type(row.get("exit")) is not int:
            raise ValueError("比較scope又は終了値が不正です")
        original["cases"].append({k: v for k, v in row.items() if k != "coverage"})
        if "report" in row:
            _same(
                split_report(bytes.fromhex(row["stdout_hex"])),
                {key: row[key] for key in ("report", "suffix_hex")},
            )
    digest = hashlib.sha256(
        json.dumps(original, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != ORIGINAL_FIELDS_SHA256:
        raise ValueError("元参照の入力/rawが変わりました")


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


def _outputs(root):
    result = {}
    for name in ("report.json", "simple.txt"):
        path = root / name
        if not os.path.lexists(path):
            result[name] = None
        else:
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or path.is_symlink():
                raise ValueError("出力先に通常file以外のentryができました")
            result[name] = _inventory(root, [name])[name]
    return result


def record(ruby, case, root):
    """実CLIのchannel、原本と既存outのbytes/mtime/modeを独立に記録する。

    Returns:
        dict: 元全fieldを保持した一条件の実観測。

    Raises:
        ValueError: 未知入力、未完了、原本/out変更又は参照不一致の場合。

    """
    template = _template()
    validate_fixture(template)
    rows = [row for row in template["cases"] if row["id"] == case.get("id")]
    if len(rows) != 1:
        raise ValueError("unknown case")
    _same(case, rows[0])
    if os.path.lexists(root):
        raise ValueError("case directoryはfreshである必要があります")
    root.mkdir(parents=True)
    for name, text in case["files"].items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode())
    for name in case.get("directories", []):
        (root / name).mkdir(parents=True, exist_ok=True)
    restored = []
    for name in case.get("readonly", []):
        path = root / name
        info = path.stat()
        restored.append((path, info))
        path.chmod(stat.S_IMODE(info.st_mode) & ~0o222)
    before = _inventory(root, case["files"])
    out_before = _outputs(root)
    try:
        kwargs = {
            "cwd": root,
            "env": child_environment(),
            "input": bytes.fromhex(case.get("stdin_hex", "")),
            "timeout": 90,
            "check": False,
        }
        command = [str(ruby), "-e", CLI, "--", *case["args"]]
        if case.get("redirect"):
            with (
                (root / "captured.stdout").open("xb") as stdout,
                (root / "captured.stderr").open("xb") as stderr,
            ):
                done = subprocess.run(command, stdout=stdout, stderr=stderr, **kwargs)
            stdout, stderr = (
                (root / name).read_bytes() for name in ("captured.stdout", "captured.stderr")
            )
        else:
            done = subprocess.run(command, capture_output=True, **kwargs)
            stdout, stderr = done.stdout, done.stderr
        after = _inventory(root, case["files"])
        out_after = _outputs(root)
        _write_json(
            root / "private-cli.json",
            {
                "exit": done.returncode,
                "stdout_hex": stdout.hex(),
                "stderr_hex": stderr.hex(),
                "inputs_before": before,
                "inputs_after": after,
                "out_before": out_before,
                "out_after": out_after,
            },
        )
        _same(before, after)
        _same(out_before, out_after)
        if case["id"] in LISTED_PATHS:
            listed = "".join(name + "\r\n" for name in LISTED_PATHS[case["id"]]).encode()
            channels = (b"", listed) if "--stderr" in case["args"] else (listed, b"")
            _same([done.returncode, stdout, stderr], [0, *channels])
        row = copy.deepcopy(case)
        row.update(
            exit=done.returncode,
            stdout_hex=stdout.hex(),
            stderr_hex=stderr.hex(),
            outputs={name: info["bytes_hex"] if info else None for name, info in out_after.items()},
        )
        if "report" in case:
            row.update(split_report(stdout))
        _same(row, case)
        return row
    finally:
        # readonlyの観測後、専用root内の同じ実fileだけ元属性へ戻す。
        for path, before_info in restored:
            now = path.lstat()
            if stat.S_ISREG(now.st_mode) and (now.st_dev, now.st_ino) == (
                before_info.st_dev,
                before_info.st_ino,
            ):
                path.chmod(stat.S_IMODE(before_info.st_mode))


def collect(ruby, work, output):
    """固定19全件の完了後だけ、公開先をexclusive作成する。

    Raises:
        ValueError: 保存先、実版、出典又は観測の契約を満たさない場合。

    """
    _fresh(work, output)
    value = _template()
    validate_fixture(value)
    work.mkdir(parents=True)
    done = subprocess.run(
        [str(ruby), "-e", METADATA, json.dumps(list(SOURCES))],
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
    _same(json.loads(done.stdout), {"primary": PRIMARY, "source_sha256": SOURCES})
    value["cases"] = [record(ruby, row, work / row["id"]) for row in value["cases"]]
    _write_json(output, value)


def main():
    """候補を受け取らず、固定版の実RuboCopだけを採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.output)


if __name__ == "__main__":
    main()
