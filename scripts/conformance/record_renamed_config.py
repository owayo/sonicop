"""固定RuboCopのwarning rename36CLI/4APIを、原本と限定投影を保って再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import subprocess
import tempfile
from contextlib import contextmanager
from pathlib import Path

from record_config_cop_namespaces import LOADER, reference_environment
from record_config_obsoletion import _primary

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/renamed_config.json"
CASE_IDS = (
    "canonical-enabled",
    "old-naming-enabled",
    "old-style-enabled",
    "naming-disabled-over-new",
    "style-disabled-over-new",
    "old-first-enabled-over-new",
    "new-first-enabled-over-new",
    "both-style-last-enabled",
    "both-reverse-key-order",
    "both-style-last-disabled",
    "old-hash-allowed-method",
    "old-empty-hash",
    "old-ignore",
    "old-explicit-color",
    "old-force-color",
    "version",
    "verbose-version",
    "only-syntax",
    "show-new",
    "show-other",
    "list-enabled",
    "list-targets",
    "multi-file",
    "parent-old-child-new",
    "parent-old-child-old",
    "pending-inspect",
    "pending-show",
    "enabled-null",
    "except-new",
    "safe-false",
    "empty-targets",
    "old-false",
    "old-empty-array-replaces-new-allow",
    "fatal-suppresses-warning",
    "parameter-order",
    "stderr-progress",
)
API_IDS = ("old-naming-enabled", "old-empty-hash", "pending-inspect", "enabled-null")
INPUT_SHA256 = "2329b064228d965f5e065462282eb8b320a5c16db17040b227a4e8dbd9eb9ba4"
ORDER_INPUT_SHA256 = "41f9dd2e3ecfa4ef6449133b4a0b345e2a1188288412f9d4d583873f3869f30e"
RUNTIME_API = (
    LOADER
    + r"""
require 'json'
require 'digest'
old = 'Naming/PredicateName'
name = 'Naming/PredicatePrefix'
klass = RuboCop::Cop::Naming::PredicatePrefix
def config_for
  RuboCop::ConfigLoader.merge_with_default(RuboCop::ConfigLoader.load_file('config.yml'), 'config.yml')
end
config = config_for
before = config.to_h.slice(old, name)
first = config.for_cop(name).slice('Enabled', 'AllowedMethods')
repeat = config.for_cop(name).slice('Enabled', 'AllowedMethods')
catalog = config.for_cop(klass).slice('Enabled', 'AllowedMethods')
after_class = config.for_cop(name).slice('Enabled', 'AllowedMethods')
fresh = config_for
class_first = fresh.for_cop(klass).slice('Enabled', 'AllowedMethods')
string_after_class_first = fresh.for_cop(name).slice('Enabled', 'AllowedMethods')
puts JSON.generate({raw_before: before, raw_after: config.to_h.slice(old, name), first: first,
  repeat: repeat, catalog: catalog, after_class: after_class, class_first: class_first,
  string_after_class_first: string_after_class_first,
  deprecated_names: RuboCop::ConfigObsoletion.deprecated_names_for(name),
  sources: %w[lib/rubocop/config.rb lib/rubocop/config_loader_resolver.rb].to_h { |file|
    [file, Digest::SHA256.file(File.join(Gem.loaded_specs.fetch('rubocop').full_gem_path, file)).hexdigest] }})
"""
)


def _same(actual, expected):
    if type(actual) is not type(expected):
        return False
    if isinstance(actual, dict):
        return actual.keys() == expected.keys() and all(
            _same(actual[key], expected[key]) for key in actual
        )
    if isinstance(actual, list):
        return len(actual) == len(expected) and all(
            _same(a, b) for a, b in zip(actual, expected, strict=True)
        )
    return actual == expected


def _template():
    data = json.loads(FIXTURE.read_bytes())
    if (
        type(data.get("schema")) is not int
        or data["schema"] != 1
        or tuple(row["id"] for row in data["cases"]) != CASE_IDS
        or tuple(row["id"] for row in data["runtime_api"]) != API_IDS
    ):
        raise ValueError("incomplete fixed CLI/API matrix")
    return data


def case_inputs():
    """公開済み実参照の入力だけを再使用し、固定36条件以外を拒む。

    Returns:
        list: 候補に依存しないargs/files/env/phaseの入力。

    Raises:
        ValueError: 入力集合または有限比較範囲が変更された場合。

    """
    forms = [
        {
            key: row[key]
            for key in ("id", "files", "args", "env", "directories", "comparison", "expected_exit")
        }
        for row in _template()["cases"]
    ]
    digest = hashlib.sha256(
        json.dumps(forms, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != INPUT_SHA256 or any(type(form["expected_exit"]) is not int for form in forms):
        raise ValueError("fixed input fields/coverage changed")
    return forms


def _environment(intentional=None):
    child = reference_environment()
    for key in list(child):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            child.pop(key)
    child.update(intentional or {})
    return child


def _inventory(root, names):
    return {name: ((root / name).read_bytes(), (root / name).stat().st_mtime_ns) for name in names}


@contextmanager
def _workspace(work):
    if work is None:
        with tempfile.TemporaryDirectory(prefix="sonicop-renamed-config-") as folder:
            yield Path(folder)
    else:
        work.mkdir(parents=True, exist_ok=False)
        yield work


def _fresh_paths(work, output):
    if any(os.path.lexists(path) for path in (work, output)):
        raise ValueError("existing entry/dangling")
    paths = tuple(path.resolve(strict=False) for path in (work, output))
    if len({os.path.normcase(os.path.normpath(str(path))) for path in paths}) != 2 or any(
        os.path.lexists(path) for path in paths
    ):
        raise ValueError("normalized alias/existing destination")
    return paths


def _report(raw, form):
    report = json.loads(raw)
    metadata = report.get("metadata", {})
    paths = (
        []
        if form["id"] == "empty-targets"
        else [name for name in form["files"] if name.endswith(".rb")]
    )
    summary = report.get("summary", {})
    files = report.get("files", [])
    if (
        metadata.get("ruby_engine") != "ruby"
        or metadata.get("ruby_version") != "4.0.7"
        or metadata.get("rubocop_version") != "1.89.0"
        or not isinstance(files, list)
        or [file["path"] for file in files] != paths
        or any(
            type(summary.get(key)) is not int
            for key in ("offense_count", "target_file_count", "inspected_file_count")
        )
        or summary["target_file_count"] != len(paths)
        or summary["inspected_file_count"] != len(paths)
        or any(not isinstance(file.get("offenses"), list) for file in files)
        or summary["offense_count"] != sum(len(file["offenses"]) for file in files)
    ):
        raise ValueError("reference JSON identity/completion/offenses differ")
    return report


def _public_cli(raw, form, root):
    if form["id"] not in {"show-new", "pending-show"}:
        return raw
    # 元CLIにあるcatalog Excludeの一箇所だけを保持済みplaceholderへ戻す。
    needle = f"  Exclude:\r\n  - {root.as_posix()}/spec/**/*\r\n".encode()
    if raw.count(needle) != 1:
        raise ValueError("unexpected catalog Exclude origin")
    return raw.replace(needle, b"  Exclude:\r\n  - {cwd}/spec/**/*\r\n")


def _validate_row(row, form, expected):
    if (
        type(row.get("exit")) is not int
        or type(row.get("bytes_mtime_unchanged")) is not bool
        or row["bytes_mtime_unchanged"] is not True
    ):
        raise ValueError("CLI output field types differ")
    if row["exit"] != form["expected_exit"] or not _same({key: row.get(key) for key in form}, form):
        raise ValueError("fixed CLI input/exit differs")
    stdout, stderr = bytes.fromhex(row["stdout_hex"]), bytes.fromhex(row["stderr_hex"])
    if form["comparison"] == "json":
        _report(stdout, form)
    elif form["comparison"] == "early_error" and (stdout or b"Warning:" in stderr):
        raise ValueError("warning/report before fatal")
    if not _same(row, expected):
        raise ValueError("actual CLI raw differs from fixed real reference")


def record(ruby, form, work=None):
    """実CLIだけを起動し、全原本/outと出力を固定実参照に照合する。

    Returns:
        dict: 元の36条件の1行を実出力から再生成した記録。

    Raises:
        ValueError: 入力・原本/out・終了値・全raw/JSONが契約と異なる場合。

    """
    forms = case_inputs()
    index = next((i for i, expected in enumerate(forms) if expected["id"] == form.get("id")), None)
    if index is None or not _same(forms[index], form):
        raise ValueError("unknown/changed fixed input")
    with _workspace(work) as root:
        for name in form["directories"]:
            (root / name).mkdir()
        for name, text in form["files"].items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode())
        before = _inventory(root, form["files"])
        child = subprocess.run(
            [
                ruby,
                "-e",
                LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n',
                "--",
                *form["args"],
            ],
            cwd=root,
            env=_environment(form["env"]),
            capture_output=True,
            timeout=60,
            check=False,
        )
        if before != _inventory(root, form["files"]) or (root / "report.json").exists() != (
            "report.json" in form["files"]
        ):
            raise ValueError("input/out bytes or mtime changed")
        row = {
            **form,
            "exit": child.returncode,
            "stdout_hex": _public_cli(child.stdout, form, root).hex(),
            "stderr_hex": child.stderr.hex(),
            "bytes_mtime_unchanged": True,
        }
        _validate_row(row, form, _template()["cases"][index])
        with (root / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(
                {
                    "exit": child.returncode,
                    "stdout_hex": child.stdout.hex(),
                    "stderr_hex": child.stderr.hex(),
                },
                handle,
                indent=2,
            )
            handle.write("\n")
        return row


def _public_api(value, root):
    projected = copy.deepcopy(value)
    for phase in ("raw_before", "raw_after"):
        excludes = projected[phase]["Naming/PredicatePrefix"]["Exclude"]
        if (
            not isinstance(excludes, list)
            or not excludes
            or excludes[0] != root.as_posix() + "/spec/**/*"
        ):
            raise ValueError("unexpected exact API Exclude[0] origin/suffix")
        excludes[0] = "{api_work}/spec/**/*"
    return projected


def record_api(ruby, form, work=None):
    """String/Classの4実APIを採取し、privateな8 fieldだけを限定投影する。

    Returns:
        dict: 他の全fieldを保った1条件の実API値。

    Raises:
        ValueError: API順/型・原本・raw不変・lookup値が実参照と異なる場合。

    """
    if form["id"] not in API_IDS:
        raise ValueError("unknown API input")
    fixed = next(case for case in case_inputs() if case["id"] == form["id"])
    if not _same(form, fixed):
        raise ValueError("changed API input")
    with _workspace(work) as root:
        for name in ("config.yml", "Gemfile"):
            (root / name).write_bytes(form["files"][name].encode())
        before = _inventory(root, ("config.yml", "Gemfile"))
        child = subprocess.run(
            [ruby, "-e", RUNTIME_API],
            cwd=root,
            env=_environment({"CLICOLOR_FORCE": "0"}),
            capture_output=True,
            timeout=60,
            check=False,
        )
        if (
            type(child.returncode) is not int
            or child.returncode != 0
            or before != _inventory(root, before)
            or (root / "report.json").exists()
        ):
            raise ValueError("API failed or changed original/out")
        value = json.loads(child.stdout)
        if not _same(value["raw_before"], value["raw_after"]) or not _same(
            value["first"], value["repeat"]
        ):
            raise ValueError("raw changed or repeated lookup differs")
        row = {
            "id": form["id"],
            "values": _public_api(value, root),
            "stderr_hex": child.stderr.hex(),
            "bytes_mtime_unchanged": True,
        }
        expected = next(row for row in _template()["runtime_api"] if row["id"] == form["id"])
        if not _same(row, expected):
            raise ValueError("actual API fields/types differ from real reference")
        with (root / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(
                {"values": value, "stderr_hex": child.stderr.hex()},
                handle,
                ensure_ascii=False,
                indent=2,
            )
            handle.write("\n")
        return row


def validate_fixture(value):
    """36CLI/4APIの実行完了と有限比較範囲を型付きで固定する。

    Raises:
        ValueError: 版/source・全ID順・型・raw・比較範囲が実参照と異なる場合。

    """
    expected = _template()
    if not _same(value, expected):
        raise ValueError("incomplete or changed fixture fields/types")
    forms = case_inputs()
    for row, form, reference in zip(value["cases"], forms, expected["cases"], strict=True):
        _validate_row(row, form, reference)


def _order_template():
    data = json.loads(FIXTURE.with_name("renamed_config_warning_order.json").read_bytes())
    forms = [{key: row[key] for key in ("id", "files", "args", "env")} for row in data["cases"]]
    digest = hashlib.sha256(
        json.dumps(forms, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if (
        type(data.get("schema")) is not int
        or data["schema"] != 1
        or tuple(row["id"] for row in forms)
        != (
            "rename-parameter-plain",
            "rename-parameter-force-color",
            "fatal-suppresses-rename-parameter",
        )
        or digest != ORDER_INPUT_SHA256
    ):
        raise ValueError("incomplete warning-order input matrix")
    return data, forms


def record_order(ruby, form, work=None):
    """独立した併存3条件の全warning/outを、主36/API4へ混ぜず採取する。

    Returns:
        dict: 実出力と原本/out保持を記録した1条件。

    Raises:
        ValueError: 固定入力・原本・out・warning順・JSONが実参照と異なる場合。

    """
    expected, forms = _order_template()
    index = next((i for i, row in enumerate(forms) if row["id"] == form.get("id")), None)
    if index is None or not _same(form, forms[index]):
        raise ValueError("unknown warning-order condition")
    with _workspace(work) as root:
        for name, text in form["files"].items():
            (root / name).write_bytes(text.encode())
        names = [name for name in form["files"] if name != "report.json"]
        before = _inventory(root, names)
        output_before = _inventory(root, ("report.json",))
        child = subprocess.run(
            [
                ruby,
                "-e",
                LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n',
                "--",
                *form["args"],
            ],
            cwd=root,
            env=_environment(form["env"]),
            capture_output=True,
            timeout=60,
            check=False,
        )
        if (
            type(child.returncode) is not int
            or child.returncode != expected["cases"][index]["exit"]
            or before != _inventory(root, names)
        ):
            raise ValueError("warning-order phase/original differs")
        out = (root / "report.json").read_bytes()
        preserved = output_before == _inventory(root, ("report.json",))
        if child.returncode == 1:
            _report(out, form)
        row = {
            **form,
            "exit": child.returncode,
            "stdout_hex": child.stdout.hex(),
            "stderr_hex": child.stderr.hex(),
            "out_before_hex": output_before["report.json"][0].hex(),
            "out_hex": out.hex(),
            "out_preserved": preserved,
            "preserved_input_paths": names,
            "input_bytes_mtime_unchanged": True,
            "coverage": "strict",
        }
        if not _same(row, expected["cases"][index]):
            raise ValueError("warning-order actual raw/output differs")
        with (root / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(row, handle, indent=2)
            handle.write("\n")
        return row


def main():
    """候補pathなしの実RuboCop専用入口で、主36/4または独立追補3を保存する。

    Raises:
        ValueError: 追補の実runtime/source取得が失敗した場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--warning-order", action="store_true")
    options = parser.parse_args()
    work, output = _fresh_paths(options.work_dir, options.output)
    if options.warning_order:
        expected, forms = _order_template()
        work.mkdir(parents=True, exist_ok=False)
        paths = list(expected["sources"])
        script = (
            LOADER
            + r"""
require 'json'
require 'digest'
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
sources = JSON.parse(ARGV.fetch(0)).to_h { |p| [p, Digest::SHA256.file(File.join(root, p)).hexdigest] }
puts JSON.generate({runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
 gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] }}, sources: sources})
"""
        )
        child = subprocess.run(
            [options.ruby, "-e", script, "--", json.dumps(paths)],
            cwd=work,
            env=_environment(),
            capture_output=True,
            timeout=60,
            check=False,
        )
        if type(child.returncode) is not int or child.returncode != 0 or child.stderr:
            raise ValueError("warning-order metadata failed")
        metadata = json.loads(child.stdout)
        if not _same(metadata, {"runtime": expected["runtime"], "sources": expected["sources"]}):
            raise ValueError("warning-order runtime/source differs")
        rows = [record_order(options.ruby, form, work / form["id"]) for form in forms]
        result = {"schema": 1, **metadata, "scope": expected["scope"], "cases": rows}
        with output.open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(result, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        return
    forms = case_inputs()
    work.mkdir(parents=True, exist_ok=False)
    primary = _primary(options.ruby, work)
    rows = []
    for form in forms:
        rows.append(record(options.ruby, form, work / form["id"]))
        print(form["id"], flush=True)
    apis = [
        record_api(
            options.ruby, next(form for form in forms if form["id"] == name), work / ("api-" + name)
        )
        for name in API_IDS
    ]
    result = {
        "schema": 1,
        "primary": primary,
        "cases": rows,
        "runtime_api": apis,
        "coverage": _template()["coverage"],
    }
    validate_fixture(result)
    with output.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(result, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


if __name__ == "__main__":
    main()
