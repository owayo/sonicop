"""固定RuboCopの曖昧名23条件を、phase色とnative改行を保って記録する。"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from contextlib import contextmanager
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

CASE_IDS = (
    "multiple",
    "wrong",
    "self",
    "snake",
    "color-option",
    "force-color",
    "force-no-color",
    "inherit-force",
    "root-color-option",
    "root-forced",
    "nested-color",
    "nested-no-color",
    "nested-inherit-color",
    "nested-cwd",
    "out-absent",
    "out-sentinel",
    "stdin",
    "version",
    "notice",
    "priority-obsolete",
    "priority-parameter",
    "canonical",
    "priority-cache",
)
LATE_CASES = frozenset(("nested-color", "nested-no-color", "nested-inherit-color"))
RED = frozenset(
    (
        "force-color",
        "force-no-color",
        "inherit-force",
        "root-forced",
        "nested-color",
        "nested-inherit-color",
    )
)
SOURCES = {
    "lib/rubocop/cli.rb": "7a097a2918f7bf4b03b71a49d868910f243b78aa26e31035520822709d33e933",
    "lib/rubocop/cop/registry.rb": "41af570b3bbf8f3d28778e18be26d69130f5d23006327783d4b22e57e684fff7",
    "lib/rubocop/config_loader.rb": "90dd87be340fd22a81f7205ac0dbc4315f5a9eba08d3f3aef80f0fde3394f38e",
}

MESSAGES = {
    "multiple": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "wrong": "Ambiguous cop name `Wrong/MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "self": "Ambiguous cop name `SelfAssignment` used in <root>/config.yml needs department qualifier. Did you mean Lint/SelfAssignment or Style/SelfAssignment?",
    "snake": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "color-option": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "force-color": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "force-no-color": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "inherit-force": "Ambiguous cop name `MultipleComparison` used in <root>/parent.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "root-color-option": "Ambiguous cop name `MultipleComparison` used in <root>/.rubocop.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "root-forced": "Ambiguous cop name `MultipleComparison` used in <root>/.rubocop.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "nested-color": "Ambiguous cop name `MultipleComparison` used in <root>/nested/.rubocop.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "nested-no-color": "Ambiguous cop name `MultipleComparison` used in <root>/nested/.rubocop.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "nested-inherit-color": "Ambiguous cop name `MultipleComparison` used in <root>/nested/parent.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "nested-cwd": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "out-absent": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "out-sentinel": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "stdin": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "version": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "notice": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "priority-obsolete": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
    "priority-parameter": "Ambiguous cop name `MultipleComparison` used in <root>/config.yml needs department qualifier. Did you mean Lint/MultipleComparison or Style/MultipleComparison?",
}
PREFIXES = {
    "multiple": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "wrong": "",
    "self": "config.yml: Warning: no department given for SelfAssignment.\r\n",
    "snake": "",
    "color-option": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "force-color": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "force-no-color": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "inherit-force": "parent.yml: Warning: no department given for MultipleComparison.\r\n",
    "root-color-option": ".rubocop.yml: Warning: no department given for MultipleComparison.\r\n",
    "root-forced": ".rubocop.yml: Warning: no department given for MultipleComparison.\r\n",
    "nested-color": "nested/.rubocop.yml: Warning: no department given for MultipleComparison.\r\n",
    "nested-no-color": "nested/.rubocop.yml: Warning: no department given for MultipleComparison.\r\n",
    "nested-inherit-color": "nested/parent.yml: Warning: no department given for MultipleComparison.\r\n",
    "nested-cwd": "<root>/config.yml: Warning: no department given for MultipleComparison.\r\n",
    "out-absent": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "out-sentinel": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "stdin": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "version": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "notice": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "priority-obsolete": "config.yml: Warning: no department given for MultipleComparison.\r\n",
    "priority-parameter": "config.yml: Warning: no department given for MultipleComparison.\r\n",
}

BASE = (
    "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
    "  SuggestExtensions: false\n  DisabledByDefault: true\n"
)

BAD = BASE + "MultipleComparison:\n  Enabled: true\n"

COMMON = ["--cache", "false", "--format", "json"]

NOTICE = b"-P/--parallel is being ignored because it is not compatible with --cache false.\r\n"

ANSI = re.compile(rb"\x1b\[[0-9;]*m")


def case_inputs():
    """初期ロードとStore後段を、別の実CLI条件として固定する。

    Returns:
        list: 順序を固定した23条件の入力。

    Raises:
        ValueError: 固定ID順が欠けた場合。

    """
    cases = []

    def add(name, files=None, args=None, force="0", cwd=".", stdin=None, kind="ambiguity"):
        cases.append(
            {
                "id": name,
                "files": {"Gemfile": "", **(files or {"config.yml": BAD, "a.rb": "x=1\n"})},
                "args": args or ["--config", "config.yml", *COMMON, "a.rb"],
                "env": {"CLICOLOR_FORCE": force},
                "cwd": cwd,
                "stdin_hex": stdin.hex() if stdin is not None else None,
                "kind": kind,
            }
        )

    add("multiple")
    add(
        "wrong",
        {"config.yml": BASE + "Wrong/MultipleComparison:\n  Enabled: true\n", "a.rb": "x=1\n"},
    )
    add("self", {"config.yml": BASE + "SelfAssignment:\n  Enabled: true\n", "a.rb": "x=1\n"})
    add("snake", {"config.yml": BASE + "multiple_comparison:\n  Enabled: true\n", "a.rb": "x=1\n"})
    add("color-option", args=["--color", "--config", "config.yml", *COMMON, "a.rb"])
    add("force-color", force="1")
    add("force-no-color", args=["--no-color", "--config", "config.yml", *COMMON, "a.rb"], force="1")
    add(
        "inherit-force",
        {"config.yml": BASE + "inherit_from: parent.yml\n", "parent.yml": BAD, "a.rb": "x=1\n"},
        force="1",
    )
    root = {".rubocop.yml": BAD, "a.rb": "x=1\n"}
    add("root-color-option", root, ["--color", *COMMON, "a.rb"])
    add("root-forced", root, [*COMMON, "a.rb"], force="1")
    nested = {".rubocop.yml": BASE, "nested/.rubocop.yml": BAD, "nested/a.rb": "x=1\n"}
    add("nested-color", nested, ["--color", *COMMON, "nested/a.rb"])
    add("nested-no-color", nested, ["--no-color", *COMMON, "nested/a.rb"], force="1")
    add(
        "nested-inherit-color",
        {
            ".rubocop.yml": BASE,
            "nested/.rubocop.yml": BASE + "inherit_from: parent.yml\n",
            "nested/parent.yml": BAD,
            "nested/a.rb": "x=1\n",
        },
        ["--color", *COMMON, "nested/a.rb"],
    )
    add(
        "nested-cwd",
        {"config.yml": BAD, "nested/a.rb": "x=1\n"},
        ["--config", "../config.yml", *COMMON, "a.rb"],
        cwd="nested",
    )
    add("out-absent", args=["--config", "config.yml", *COMMON, "--out", "report.json", "a.rb"])
    add(
        "out-sentinel",
        {"config.yml": BAD, "a.rb": "x=1\n", "report.json": "sentinel\n"},
        ["--config", "config.yml", *COMMON, "--out", "report.json", "a.rb"],
    )
    add("stdin", args=["--config", "config.yml", *COMMON, "--stdin", "a.rb"], stdin=b"x=1\r\n")
    add("version", args=["--version", "--config", "config.yml", *COMMON, "a.rb"])
    add("notice", args=["--config", "config.yml", *COMMON, "--no-parallel", "a.rb"])
    add(
        "priority-obsolete",
        {"config.yml": BAD + "Layout/TrailingBlankLines:\n  Enabled: false\n", "a.rb": "x=1\n"},
    )
    add(
        "priority-parameter",
        {"config.yml": BAD + "Layout/SpaceAroundOperators:\n  WrongKey: true\n", "a.rb": "x=1\n"},
    )
    add(
        "canonical",
        {"config.yml": BASE + "Lint/MultipleComparison:\n  Enabled: true\n", "a.rb": "x=1\n"},
        kind="normal",
    )
    add(
        "priority-cache",
        args=["--cache", "invalid", "--config", "config.yml", "--format", "json", "a.rb"],
        kind="option",
    )
    for case in cases:
        case["coverage"] = "strict"
    if tuple(case["id"] for case in cases) != CASE_IDS:
        raise ValueError("incomplete fixed input matrix")
    return cases


def child_environment(intentional=None):
    """共用ENV4除去の後にambient NO_COLORを外し、実測する色だけを戻す。

    Returns:
        dict: GEM/PATH/OSと親を保つ独立した子環境。

    """
    child = reference_environment()
    for key in list(child):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            child.pop(key)
    child.update(intentional or {})
    return child


def _forms():
    forms = case_inputs()
    if tuple(case["id"] for case in forms) != CASE_IDS or any(
        case["coverage"] != "strict" for case in forms
    ):
        raise ValueError("incomplete fixed input/coverage matrix")
    return forms


def _report(raw, inspected):
    report = json.loads(raw)
    identity = report.get("metadata", {})
    summary = report.get("summary", {})
    if (
        identity.get("rubocop_version") != "1.89.0"
        or identity.get("ruby_engine") != "ruby"
        or identity.get("ruby_version") != "4.0.7"
        or any(
            type(summary.get(key)) is not int
            for key in ("offense_count", "target_file_count", "inspected_file_count")
        )
        or summary
        != {"offense_count": 0, "target_file_count": 1, "inspected_file_count": inspected}
        or report.get("files") != ([{"path": "a.rb", "offenses": []}] if inspected else [])
    ):
        raise ValueError("invalid reference report identity/completion/offenses")
    return report


def _validate_case(case, form):
    if {key: case.get(key) for key in form} != form:
        raise ValueError("fixed input/coverage differs")
    if type(case.get("exit")) is not int or type(case.get("stdout_json")) is not bool:
        raise ValueError("wrong output field type")
    stdout, stderr = bytes.fromhex(case["stdout_hex"]), bytes.fromhex(case["stderr_hex"])
    if case["stdout_json"] is not stdout.startswith(b"{"):
        raise ValueError("wrong report phase")
    if case["kind"] == "normal":
        if case["exit"] != 0 or stderr or not case["stdout_json"]:
            raise ValueError("normal control failed")
        _report(stdout, 1)
    elif case["kind"] == "option":
        if (
            case["exit"] != 2
            or stdout
            or stderr != b"-C/--cache argument must be true or false\r\n"
        ):
            raise ValueError("option priority differs")
    else:
        if case["exit"] != 2:
            raise ValueError("ambiguity was not rejected")
        if case["id"] in LATE_CASES:
            _report(stdout, 0)
        elif stdout != (NOTICE if case["id"] == "notice" else b""):
            raise ValueError("unexpected early formatter output")
        message = case.get("message")
        if (
            not isinstance(message, str)
            or "\n" in message
            or not message.startswith("Ambiguous cop name `")
        ):
            raise ValueError("invalid ambiguity message/backtrace")
        if message != MESSAGES[case["id"]]:
            raise ValueError("measured ambiguity body/origin differs")
        error = b"Error: " + message.encode()
        if case["id"] in RED:
            error = b"\x1b[31m" + error + b"\x1b[0m"
        error += b"\r\n"
        if not stderr.endswith(error):
            raise ValueError("ambiguity color/native ending differs")
        prefix = PREFIXES[case["id"]].encode()
        if stderr[: -len(error)] != prefix:
            raise ValueError("invalid namespace prefix")
    if case.get("after_hex") != {name: text.encode().hex() for name, text in form["files"].items()}:
        raise ValueError("physical inputs changed")


def validate_fixture(fixture):
    """固定23入力と23strictを区別し、偽の完了や色を拒む。

    Raises:
        ValueError: 版・source・型・固定入力・出力段階が契約と異なる場合。

    """
    if type(fixture.get("schema")) is not int or fixture["schema"] != 1:
        raise ValueError("wrong schema")
    runtime = fixture.get("runtime", {})
    if (
        runtime.get("ruby") != "4.0.7"
        or runtime.get("gems") != VERSIONS
        or runtime.get("platform") != "x64-mingw-ucrt"
    ):
        raise ValueError("fixed Windows runtime mismatch")
    if fixture.get("sources") != SOURCES:
        raise ValueError("reference source identity differs")
    cases = fixture.get("cases", [])
    if not isinstance(cases, list) or tuple(row["id"] for row in cases) != CASE_IDS:
        raise ValueError("incomplete finite cases")
    for case, form in zip(cases, _forms(), strict=True):
        _validate_case(case, form)


def _fresh_paths(work, output):
    if os.path.lexists(work) or os.path.lexists(output):
        raise ValueError("existing entry or dangling symlink")
    paths = tuple(path.resolve(strict=False) for path in (work, output))
    if len({os.path.normcase(os.path.normpath(str(path))) for path in paths}) != 2 or any(
        os.path.lexists(path) for path in paths
    ):
        raise ValueError("alias or existing normalized destination")
    return paths


@contextmanager
def _workspace(work):
    if work is None:
        with tempfile.TemporaryDirectory(prefix="sonicop-ambiguous-config-") as temporary:
            yield Path(temporary)
    else:
        work.mkdir(parents=True, exist_ok=False)
        yield work


def _inventory(root, names):
    return {name: ((root / name).read_bytes(), (root / name).stat().st_mtime_ns) for name in names}


def _public_stderr(raw, root, form):
    text = raw.decode()
    if form["kind"] != "ambiguity":
        return text
    # errorとnested-cwdの先頭warningを別々に置換し、任意の本文をrootと同一視しない。
    physical = root.resolve() if sys.platform == "darwin" else root
    original = physical.as_posix()
    expected_origin = (
        MESSAGES[form["id"]].split(" used in ", 1)[1].split(" needs department qualifier.", 1)[0]
    )
    needle = f" used in {expected_origin.replace('<root>', original)} needs department qualifier."
    if text.count(needle) != 1:
        raise ValueError("unexpected dynamic error origin")
    text = text.replace(needle, f" used in {expected_origin} needs department qualifier.")
    if form["id"] == "nested-cwd":
        warning = PREFIXES[form["id"]].replace("<root>", original)
        if not text.startswith(warning):
            raise ValueError("unexpected absolute warning origin")
        text = PREFIXES[form["id"]] + text[len(warning) :]
    return text


def record(ruby, form, work=None):
    """実CLIの原本/out不変と出力段階を検査してから、制御したoriginだけを公開化する。

    Returns:
        dict: 元入力と全raw/終了値/coverageを保つ1条件。

    Raises:
        ValueError: 固定入力・原本・out状態・曖昧名の出力が契約に反する場合。

    """
    expected = next((case for case in _forms() if case["id"] == form.get("id")), None)
    if expected is None or form != expected:
        raise ValueError("unknown or changed scenario")
    with _workspace(work) as root:
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
            cwd=root / form["cwd"],
            env=child_environment(form["env"]),
            input=bytes.fromhex(form["stdin_hex"]) if form["stdin_hex"] is not None else None,
            capture_output=True,
            timeout=60,
            check=False,
        )
        after = _inventory(root, form["files"])
        if before != after or (root / form["cwd"] / "report.json").exists() != (
            "report.json" in form["files"]
        ):
            raise ValueError("source/config/out bytes or mtime changed")
        text = _public_stderr(child.stderr, root, form)
        row = {
            **form,
            "exit": child.returncode,
            "stdout_hex": child.stdout.hex(),
            "stdout_json": child.stdout.startswith(b"{"),
            "stderr_hex": text.encode().hex(),
            "after_hex": {name: value[0].hex() for name, value in after.items()},
        }
        if form["kind"] == "ambiguity":
            row["message"] = (
                ANSI.sub(b"", text.encode()).decode().split("Error: ", 1)[1].rstrip("\r\n")
            )
        row["coverage"] = row.pop("coverage")
        _validate_case(row, form)
        for name, data in (("reference.stdout", child.stdout), ("reference.stderr", child.stderr)):
            with (root / name).open("xb") as handle:
                handle.write(data)
        return row


def main():
    """候補引数を受け取らず、実RuboCop23条件だけをfresh/exclusive保存する。

    Raises:
        ValueError: runtimeの取得に失敗した場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    options = parser.parse_args()
    work, output = _fresh_paths(options.work_dir, options.output)
    forms = _forms()
    work.mkdir(parents=True, exist_ok=False)
    script = (
        LOADER
        + r"""
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
sources = JSON.parse(ARGV.fetch(0)).to_h do |path|
  [path, Digest::SHA256.file(File.join(spec.full_gem_path, path)).hexdigest]
end
puts JSON.generate({runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h do |name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]
  end}, sources: sources})
"""
    )
    child = subprocess.run(
        [options.ruby, "-e", script, "--", json.dumps(list(SOURCES))],
        cwd=work,
        env=child_environment(),
        capture_output=True,
        timeout=60,
        check=False,
    )
    if type(child.returncode) is not int or child.returncode != 0 or child.stderr:
        raise ValueError("runtime/source query failed")
    meta = json.loads(child.stdout)
    if meta.get("sources") != SOURCES or meta.get("runtime") != {
        "ruby": "4.0.7",
        "platform": "x64-mingw-ucrt",
        "gems": VERSIONS,
    }:
        raise ValueError("runtime/source identity differs")
    rows = []
    for form in forms:
        rows.append(record(options.ruby, form, work / form["id"]))
        print(form["id"], flush=True)
    fixture = {"schema": 1, "runtime": meta["runtime"], "cases": rows, "sources": meta["sources"]}
    validate_fixture(fixture)
    with output.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(fixture, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


if __name__ == "__main__":
    main()
