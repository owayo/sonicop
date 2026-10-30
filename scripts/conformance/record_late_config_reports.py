"""本家のRunner終了境界38条件を、元rawと原本属性を保って再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/late_config_reports.json"
CASE_IDS = (
    "single",
    "prefix-ambiguous",
    "prefix-unknown",
    "prefix-obsolete",
    "prefix-missing",
    "failed-first",
    "root-directory",
    "discovery-directory",
    "initial",
    "initial-out",
    "list-files",
    "list-directory",
    "failfast-offense",
    "failfast-clean",
    "correct-a",
    "correct-A",
    "correct-parallel",
    "cache-single",
    "cache-parallel",
    "multiple",
    "json-out",
    "stderr-progress",
    "stderr-single",
    "stderr-parallel",
    "stdin-single",
    "stdin-single-files",
    "stdin-directory-a",
    "stdin-directory-A",
    "stdin-directory-files",
    "force-exclusion",
    "ignore-parent",
    "recognized",
    "out-is-directory",
    "normal",
    "normal-correct",
    "color-json",
    "selection-config",
    "unvisited-config",
)
INPUT_SHA256 = "3f44e1f0b094e8e9ec110a8dd6745003552798f30d8859ecb3297e9ed89f6e4f"
PARALLEL = {"correct-parallel", "cache-parallel", "stderr-parallel", "unvisited-config"}
EARLY = {
    "discovery-directory",
    "initial",
    "initial-out",
    "list-directory",
    "ignore-parent",
    "out-is-directory",
}
RAW_KEYS = ("raw_stdout_sha256", "raw_stderr_sha256", "raw_output_sha256")
CLI = LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n'


def _same(actual, expected):
    if type(actual) is not type(expected):
        return False
    if isinstance(actual, dict):
        return actual.keys() == expected.keys() and all(
            _same(actual[k], expected[k]) for k in actual
        )
    if isinstance(actual, list):
        return len(actual) == len(expected) and all(
            _same(a, b) for a, b in zip(actual, expected, strict=True)
        )
    return actual == expected


def _template():
    return json.loads(FIXTURE.read_bytes())


def case_inputs():
    """38入力とscopeを固定し、候補出力や空集合を採取仕様にしない。

    Returns:
        list: 固定ID順の本家入力。

    Raises:
        ValueError: ID・入力・比較scopeが元参照から変わった場合。

    """
    forms = [{k: v for k, v in row.items() if k != "expected"} for row in _template()["cases"]]
    digest = hashlib.sha256(
        json.dumps(forms, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if tuple(row["id"] for row in forms) != CASE_IDS or digest != INPUT_SHA256:
        raise ValueError("changed fixed inputs or coverage")
    return forms


def _fresh_paths(work, output):
    original = (Path(work), Path(output))
    if any(os.path.lexists(path) for path in original):
        raise ValueError("destination already exists")
    resolved = tuple(path.resolve(strict=False) for path in original)
    names = [os.path.normcase(os.path.normpath(str(path))) for path in resolved]
    if names[0] == names[1] or any(os.path.lexists(path) for path in resolved):
        raise ValueError("aliased or occupied destinations")
    if resolved[0] in resolved[1].parents:
        raise ValueError("output would be inside private work")
    return resolved


def _environment(intentional):
    child = reference_environment()
    for key in list(child):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            child.pop(key)
    child.update(intentional)
    return child


def _inventory(work, names):
    return {
        name: (
            (work / name).read_bytes(),
            (work / name).stat().st_mtime_ns,
            (work / name).stat().st_mode,
        )
        for name in names
    }


def _without_metadata(data):
    marker = b'"metadata":{'
    if marker not in data:
        return data
    if data.count(marker) != 1:
        raise ValueError("duplicate report metadata")
    start = data.index(marker)
    end = data.index(b'"files":', start)
    metadata = json.loads(b"{" + data[start:end].removesuffix(b",") + b"}")["metadata"]
    if (
        metadata.get("rubocop_version") != "1.89.0"
        or metadata.get("ruby_engine") != "ruby"
        or metadata.get("ruby_version") != "4.0.7"
    ):
        raise ValueError("wrong reference runtime")
    return data[:start] + data[end:]


def _public(data, work):
    # 実測されたworkの正確なprefixだけを置換し、path一般のcanonical化はしない。
    return _without_metadata(data).replace(work.as_posix().encode() + b"/", b"<WORKDIR>/")


def validate_completion(form, stdout, stderr, outputs):
    """started前と、実際に完了したprefixの全reportを型付きで区別する。

    Raises:
        ValueError: report/metadata/完了prefix/全offenseの件数が実参照と違う場合。

    """
    documents = []
    for raw in (stdout, stderr, *outputs):
        marker = b'{"metadata":'
        if marker in raw:
            _without_metadata(raw)
            document, _ = json.JSONDecoder().raw_decode(raw[raw.index(marker) :].decode())
            documents.append(document)
    if form["id"] in EARLY or form["id"] == "list-files":
        if documents:
            raise ValueError("report before started")
        return
    if len(documents) != 1:
        raise ValueError("missing or duplicate finished report")
    report = documents[0]
    if form["id"] in {"single", "stdin-single", "stdin-single-files"}:
        paths, targets = [], 1
    elif form["id"] == "selection-config":
        paths, targets = [], 2
    elif form["id"] == "failed-first":
        paths, targets = [], 3
    elif form["id"] in {
        "root-directory",
        "stdin-directory-a",
        "stdin-directory-A",
        "stdin-directory-files",
    }:
        paths, targets = ["Gemfile", "good.rb", "last.rb"], 4
    elif form["id"] in {"normal", "normal-correct"}:
        paths, targets = ["good.rb", "last.rb"], 2
    else:
        paths, targets = ["good.rb"], 3
    summary = report["summary"]
    if any(
        type(summary.get(key)) is not int
        for key in ("target_file_count", "inspected_file_count", "offense_count")
    ):
        raise ValueError("non-integer summary")
    if (
        [row["path"] for row in report["files"]] != paths
        or summary["target_file_count"] != targets
        or summary["inspected_file_count"] != len(paths)
    ):
        raise ValueError("wrong completed prefix")
    if summary["offense_count"] != sum(len(row["offenses"]) for row in report["files"]):
        raise ValueError("incomplete offenses")


def _comparable(expected):
    return {k: v for k, v in expected.items() if k not in RAW_KEYS}


def validate_fixture(value):
    """原38の期待rawを保持し、変動する作業path由来のhashだけを別に検証する。

    Raises:
        ValueError: schema・版・source・全ID・期待値・型またはhashが不完全な場合。

    """
    template = _template()
    if type(value.get("schema")) is not int or value["schema"] != 1:
        raise ValueError("wrong schema")
    for key in ("versions", "sources"):
        if not _same(value.get(key), template[key]):
            raise ValueError("wrong primary version/source")
    forms = case_inputs()
    rows = value.get("cases", [])
    if not isinstance(rows, list) or len(rows) != 38:
        raise ValueError("incomplete matrix")
    for row, form, expected in zip(rows, forms, template["cases"], strict=True):
        if not _same({k: v for k, v in row.items() if k != "expected"}, form):
            raise ValueError("changed input/scope")
        if not _same(_comparable(row["expected"]), _comparable(expected["expected"])):
            raise ValueError("changed real reference bytes/fields")
        hashes = [row["expected"][key] for key in RAW_KEYS[:2]]
        output_hashes = row["expected"]["raw_output_sha256"]
        if (
            not isinstance(output_hashes, dict)
            or output_hashes.keys() != expected["expected"]["raw_output_sha256"].keys()
        ):
            raise ValueError("missing output hash")
        hashes.extend(digest for digest in output_hashes.values() if digest is not None)
        if any(
            not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None
            for digest in hashes
        ):
            raise ValueError("invalid raw hash")


def record(ruby, form, work):
    """固定版の実CLIだけを起動し、全stream・原本bytes/mtime/属性を照合する。

    Returns:
        dict: 元の入力と実際の全期待値・raw hashを持つ1行。

    Raises:
        ValueError: 入力・原本・出力・終了値が元の実参照から変わった場合。

    """
    forms = case_inputs()
    index = next((i for i, item in enumerate(forms) if item.get("id") == form.get("id")), None)
    if index is None or not _same(forms[index], form):
        raise ValueError("unknown or modified fixed input")
    work = Path(work)
    if os.path.lexists(work):
        raise ValueError("private case work already exists")
    work.mkdir(parents=True)
    for name in form["directories"]:
        (work / name).mkdir(parents=True)
    for name, text in form["files"].items():
        path = work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode())
    before = _inventory(work, form["files"])
    child = subprocess.run(
        [ruby, "-e", CLI, "--", *form["args"]],
        cwd=work,
        env=_environment(form["env"]),
        input=bytes.fromhex(form["stdin_hex"]),
        capture_output=True,
        check=False,
        timeout=60,
    )
    out = {
        name: (work / name).read_bytes() if (work / name).is_file() else None
        for name in form["outputs"]
    }
    validate_completion(
        form, child.stdout, child.stderr, [value for value in out.values() if value is not None]
    )
    after = _inventory(work, form["files"])
    unchanged = [name for name, (data, _, _) in before.items() if after[name][0] == data]
    if any(after[name] != before[name] for name in unchanged):
        raise ValueError("unchanged bytes were rewritten or permissions changed")
    if any(after[name][2] != before[name][2] for name in before):
        raise ValueError("source permissions changed")
    stderr = _public(child.stderr, work)
    if form.get("stderr_contract") == "open-error":
        stderr = stderr.splitlines(keepends=True)[0]
    expected = {
        "exit": child.returncode,
        "stdout_hex": _public(child.stdout, work).hex(),
        "stderr_hex": stderr.hex(),
        "stderr_contract": form.get("stderr_contract", form["kind"]),
        "files_after": {
            name: state[0].hex() for name, state in after.items() if name not in form["outputs"]
        },
        "unchanged_files": unchanged,
        "outputs": {
            name: _public(data, work).hex() if data is not None else None
            for name, data in out.items()
        },
        "raw_stdout_sha256": hashlib.sha256(child.stdout).hexdigest(),
        "raw_stderr_sha256": hashlib.sha256(child.stderr).hexdigest(),
        "raw_output_sha256": {
            name: hashlib.sha256(data).hexdigest() if data is not None else None
            for name, data in out.items()
        },
    }
    if not _same(_comparable(expected), _comparable(_template()["cases"][index]["expected"])):
        raise ValueError("actual exit/streams/report/source/out differs from real reference")
    with (work / "private-raw.json").open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(
            {
                "exit": child.returncode,
                "stdout_hex": child.stdout.hex(),
                "stderr_hex": child.stderr.hex(),
                "outputs_hex": {
                    name: data.hex() if data is not None else None for name, data in out.items()
                },
                "before": {
                    name: {"hex": state[0].hex(), "mtime_ns": state[1], "mode": state[2]}
                    for name, state in before.items()
                },
                "after": {
                    name: {"hex": state[0].hex(), "mtime_ns": state[1], "mode": state[2]}
                    for name, state in after.items()
                },
            },
            handle,
            indent=2,
        )
        handle.write("\n")
    return {**form, "expected": expected}


def _runtime(ruby, work, expected_sources=None):
    expected_sources = _template()["sources"] if expected_sources is None else expected_sources
    source = (
        LOADER
        + "\nrequire 'json'; require 'digest'\n"
        + "source_files = JSON.parse(ARGV.fetch(0))\n"
        + r"""
puts JSON.generate({versions: %w[parser prism rubocop-ast rubocop rainbow].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] },
 ruby: RUBY_VERSION, sources: source_files.to_h { |name|
 [name, Digest::SHA256.file(File.join(Gem.loaded_specs.fetch('rubocop').full_gem_path, name)).hexdigest] }})
"""
    )
    child = subprocess.run(
        [ruby, "-e", source, json.dumps(list(expected_sources))],
        cwd=work,
        env=_environment({"CLICOLOR_FORCE": "0"}),
        capture_output=True,
        check=False,
        timeout=60,
    )
    if type(child.returncode) is not int or child.returncode != 0 or child.stderr:
        raise ValueError("real runtime/source query failed")
    value = json.loads(child.stdout)
    if (
        type(child.returncode) is not int
        or child.returncode != 0
        or child.stderr
        or value.get("ruby") != "4.0.7"
        or not _same(value.get("versions"), VERSIONS)
        or not _same(value.get("sources"), expected_sources)
    ):
        raise ValueError("wrong real oracle runtime/source")


def main():
    """候補を呼ぶ引数を設けず、固定版の実RuboCop38条件を再採取する。

    Raises:
        ValueError: 保存先・固定入力・実出力が契約と違う場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    options = parser.parse_args()
    work, output = _fresh_paths(options.work_dir, options.output)
    validate_fixture(_template())
    if os.name != "nt":
        raise ValueError("38 raw records require the pinned Windows oracle")
    work.mkdir(parents=True)
    _runtime(options.ruby, work)
    rows = [
        record(options.ruby, form, work / f"{index:02d}-{form['id']}")
        for index, form in enumerate(case_inputs())
    ]
    result = {**copy.deepcopy(_template()), "cases": rows}
    validate_fixture(result)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(result, handle, ensure_ascii=True, indent=2)
        handle.write("\n")


if __name__ == "__main__":
    main()
