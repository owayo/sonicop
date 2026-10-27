"""cache/configの最後優先と、既存の拒否境界を本家実CLIから記録する。"""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

from record_formatter_colors import LOADER, VERSIONS, _environment

SOURCE = b"x = 2**3\n"
CONFIG_A = (
    b"AllCops:\n  NewCops: disable\n  TargetRubyVersion: 3.3\n"
    b"  UseProjectIndex: false\nLayout/SpaceAroundOperators:\n"
    b"  EnforcedStyleForExponentOperator: space\n"
)
CONFIG_B = CONFIG_A.replace(b": space\n", b": no_space\n")


def _require(ok, message):
    if not ok:
        raise RuntimeError(message)


def _cases():
    reports = [
        ("cache-true-false", ["--cache", "true", "--cache", "false"]),
        ("cache-false-true", ["--cache", "false", "--cache", "true"]),
        ("cache-same-false", ["--cache", "false", "--cache", "false"]),
        ("cache-same-true", ["--cache", "true", "--cache", "true"]),
        ("cache-three", ["--cache", "false", "--cache", "invalid", "--cache", "true"]),
        ("cache-invalid-first", ["--cache", "invalid", "--cache", "false"]),
        ("cache-empty-first", ["--cache=", "--cache", "false"]),
        ("cache-equals", ["--cache=true", "--cache=false"]),
        ("cache-short", ["-Ctrue", "-Cfalse"]),
        ("cache-mixed", ["-C", "false", "--cache=true"]),
        ("config-a-b", ["--config", "a.yml", "--config", "b.yml", "--cache", "false"]),
        ("config-b-a", ["--config", "b.yml", "--config", "a.yml", "--cache", "false"]),
        ("config-same", ["--config", "a.yml", "--config", "a.yml", "--cache", "false"]),
        (
            "config-absent-first",
            ["--config", "missing.yml", "--config", "a.yml", "--cache", "false"],
        ),
        (
            "config-invalid-first",
            ["--config", "invalid.yml", "--config", "a.yml", "--cache", "false"],
        ),
        ("config-empty-first", ["--config=", "--config", "a.yml", "--cache", "false"]),
        ("config-equals", ["--config=a.yml", "--config=b.yml", "--cache", "false"]),
        ("config-short", ["-ca.yml", "-cb.yml", "--cache", "false"]),
        ("config-mixed", ["-c", "b.yml", "--config=a.yml", "--cache", "false"]),
        (
            "combined",
            [
                "--config",
                "missing.yml",
                "--cache",
                "invalid",
                "--config",
                "b.yml",
                "--cache",
                "true",
            ],
        ),
        ("single-cache", ["--cache", "false"]),
        ("single-config", ["--config", "a.yml", "--cache", "false"]),
    ]
    cases = [{"id": name, "options": options, "kind": "report"} for name, options in reports]
    cases += [
        {
            "id": "dot-env-cli-cache",
            "dotfile": "--cache invalid",
            "env_options": "--cache false",
            "options": ["--cache", "true"],
            "kind": "report",
        },
        {
            "id": "env-cli-cache",
            "env_options": "--cache invalid",
            "options": ["--cache", "false"],
            "kind": "report",
        },
        {
            "id": "dot-env-cli-config",
            "dotfile": "--config missing.yml",
            "env_options": "--config a.yml",
            "options": ["--config", "b.yml", "--cache", "false"],
            "kind": "report",
        },
        {
            "id": "env-beats-dot-config",
            "dotfile": "--config a.yml",
            "env_options": "--config b.yml",
            "options": ["--cache", "false"],
            "kind": "report",
        },
        {
            "id": "cli-beats-dot-config",
            "dotfile": "--config b.yml",
            "options": ["--config", "a.yml", "--cache", "false"],
            "kind": "report",
        },
        {
            "id": "formatter-slots",
            "options": [
                "--config",
                "a.yml",
                "--config",
                "b.yml",
                "--cache",
                "true",
                "--cache",
                "false",
            ],
            "formats": ["-f", "json", "-o", "first.json", "-f", "json", "-o", "second.json"],
            "kind": "report",
        },
        {
            "id": "selector-override",
            "options": [
                "--only",
                "Unknown/Cop",
                "--only",
                "Layout/SpaceAroundOperators",
                "--cache",
                "true",
                "--cache",
                "false",
            ],
            "kind": "report",
        },
    ]
    cases += [
        {
            "id": "formatter-options-interleaved",
            "options": [],
            "formats": [
                "-f",
                "json",
                "-o",
                "first.json",
                "--config",
                "a.yml",
                "--cache",
                "true",
                "-f",
                "json",
                "--config",
                "b.yml",
                "--cache",
                "false",
                "-o",
                "second.json",
            ],
            "kind": "report",
        },
        {
            "id": "out-before-formatter-interleaved",
            "options": [],
            "formats": [
                "-o",
                "first.json",
                "--cache",
                "true",
                "-f",
                "json",
                "--config",
                "a.yml",
                "-o",
                "second.json",
                "--cache",
                "false",
                "--config",
                "b.yml",
            ],
            "kind": "report",
        },
    ]
    rejections = [
        ("cache-invalid-last", ["--cache", "false", "--cache", "invalid"]),
        ("cache-empty-last", ["--cache", "false", "--cache="]),
        ("cache-missing-last", ["--cache", "false", "--cache"]),
        ("cache-missing-first", ["--cache", "--cache", "false"]),
        ("config-missing-last", ["--config", "a.yml", "--config"]),
        ("config-missing-first", ["--config", "--config", "a.yml"]),
        ("config-absent-last", ["--config", "a.yml", "--config", "missing.yml"]),
        ("config-invalid-last", ["--config", "a.yml", "--config", "invalid.yml"]),
        ("config-empty-last", ["--config", "a.yml", "--config="]),
        (
            "severity-invalid-first",
            ["--fail-level", "invalid", "--fail-level", "error", "--cache", "false"],
        ),
        (
            "limit-invalid-first",
            ["--exclude-limit", "invalid", "--exclude-limit", "1", "--cache", "false"],
        ),
    ]
    cases += [{"id": name, "options": options, "kind": "rejection"} for name, options in rejections]
    return cases


def _report(raw):
    report = json.loads(raw)
    report.pop("metadata")
    _require(
        report["summary"]["target_file_count"] == report["summary"]["inspected_file_count"] == 1,
        "unfinished inspection",
    )
    _require([f["path"] for f in report["files"]] == ["case.rb"], "target set differs")
    return report


def _record(ruby, project, case, evidence=None):
    project.mkdir()
    originals = {
        "case.rb": SOURCE,
        "a.yml": CONFIG_A,
        "b.yml": CONFIG_B,
        ".rubocop.yml": CONFIG_A,
        "invalid.yml": b"AllCops: [\n",
    }
    if "dotfile" in case:
        originals[".rubocop"] = case["dotfile"].encode()
    for name, raw in originals.items():
        (project / name).write_bytes(raw)
    states = {
        name: (raw, (project / name).stat().st_mtime_ns, (project / name).stat().st_mode)
        for name, raw in originals.items()
    }
    env = _environment()
    env["XDG_CACHE_HOME"] = str(project / "cache-home")
    env["RUBOCOP_CACHE_ROOT"] = str(project / "cache-home/reference")
    if "env_options" in case:
        env["RUBOCOP_OPTS"] = case["env_options"]
    args = [
        "--no-color",
        "--only",
        "Layout/SpaceAroundOperators",
        *case.get("formats", ["-f", "json"]),
        *case["options"],
        "case.rb",
    ]
    result = subprocess.run(
        [ruby, "-e", LOADER, "--", *args],
        cwd=project,
        env=env,
        capture_output=True,
        timeout=90,
        check=False,
    )
    if evidence is not None:
        (evidence / (case["id"] + ".stdout")).open("xb").write(result.stdout)
        (evidence / (case["id"] + ".stderr")).open("xb").write(result.stderr)
    _require(
        json.loads((project / "versions.json").read_bytes()) == VERSIONS, "loaded versions differ"
    )
    _require(
        all(
            (path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_mode) == states[name]
            for name in states
            for path in [project / name]
        ),
        "input bytes/stat changed",
    )
    outputs = {
        name: _report((project / name).read_bytes()) if (project / name).exists() else None
        for name in ("first.json", "second.json")
    }
    if case["kind"] == "report":
        _require(
            result.returncode in (0, 1) and result.stderr == b"", "report was rejected or warned"
        )
        report = _report(result.stdout) if result.stdout else None
        _require(
            report is not None or any(value is not None for value in outputs.values()),
            "missing report",
        )
    else:
        _require(
            result.returncode == 2 and result.stdout == b"" and result.stderr,
            "rejection envelope differs",
        )
        report = None
    cache_files = any(path.is_file() for path in (project / "cache-home").rglob("*"))
    return {
        **case,
        "args": args,
        "exit": result.returncode,
        "report": report,
        "outputs": outputs,
        "cache_files": cache_files,
        "stderr_nonempty": bool(result.stderr),
    }


def main():
    """未完了・原本変更を参照へ混ぜず、既存の記録を上書きしない。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--evidence-directory", type=Path)
    options = parser.parse_args()
    _require(not options.output.exists(), "output already exists")
    if options.evidence_directory is not None:
        options.evidence_directory.mkdir(parents=True, exist_ok=False)
    parent = dict(os.environ)
    forms = _cases()
    _require(
        forms and len({case["id"] for case in forms}) == len(forms), "duplicate or empty case set"
    )
    with tempfile.TemporaryDirectory(prefix="sonicop-repeated-options-") as temporary:
        cases = [
            _record(options.ruby, Path(temporary) / case["id"], case, options.evidence_directory)
            for case in forms
        ]
    _require(dict(os.environ) == parent, "parent environment changed")
    result = {
        "reference": VERSIONS,
        "source_hex": SOURCE.hex(),
        "config_a_hex": CONFIG_A.hex(),
        "config_b_hex": CONFIG_B.hex(),
        "scope": (
            "report: full common JSON and cache writing; rejection: exit/stdout/input guard only"
        ),
        "cases": cases,
    }
    with options.output.open("x", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(result, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
