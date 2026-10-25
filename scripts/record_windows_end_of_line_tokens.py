#!/usr/bin/env python3
"""Windows native の EOF 規則を、明示 crlf とは別の実 CLI から記録する。"""

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

ENV_KEYS = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")


def require(condition, message):
    """未完了・対象欠落・補正破壊を、空の成功記録にしない。"""
    if not condition:
        raise RuntimeError(message)


def record(ruby, source_fixture):
    source_bytes = source_fixture.read_bytes()
    source = json.loads(source_bytes)
    require(len(source["cases"]) == 92, "expected all 92 source inputs")
    env = dict(os.environ)
    for key in ENV_KEYS:
        env.pop(key, None)
    groups = []
    with tempfile.TemporaryDirectory(prefix="sonicop-windows-endings-") as directory:
        root = Path(directory)
        wrapper = root / "reference.rb"
        wrapper.write_text(
            "gem 'parser', '3.3.12.0'\ngem 'prism', '1.8.1'\n"
            "gem 'rubocop-ast', '1.50.0'\ngem 'rubocop', '1.89.0'\n"
            "load Gem.bin_path('rubocop', 'rubocop', '1.89.0')\n",
            encoding="utf-8",
            newline="\n",
        )
        for original in source["groups"]:
            if original["style"] != "native":
                continue
            engine = original["engine"]
            work = root / engine
            work.mkdir()
            (work / ".rubocop.yml").write_text(
                original["config"], encoding="utf-8", newline="\n"
            )
            names = [item["path"] for item in original["files"]]
            require(len(names) == (90 if engine == "parser_whitequark" else 92), engine)
            require(len(set(names)) == len(names), "duplicate source paths")
            for name in names:
                (work / name).write_bytes(bytes.fromhex(source["cases"][name]))
            expected = None
            for mode in (None, "-a", "-A"):
                for _ in range(2):
                    command = [
                        ruby,
                        str(wrapper),
                        "--config",
                        ".rubocop.yml",
                        "--cache",
                        "false",
                        "--only",
                        "Layout/EndOfLine",
                        "--format",
                        "json",
                    ]
                    if mode:
                        command.append(mode)
                    result = subprocess.run(
                        command + names,
                        cwd=work,
                        env=env,
                        capture_output=True,
                        check=False,
                        timeout=180,
                    )
                    require(result.returncode in (0, 1), repr(result.stderr))
                    require(not result.stderr, repr(result.stderr))
                    report = json.loads(result.stdout)
                    metadata = report["metadata"]
                    require(metadata["rubocop_version"] == "1.89.0", "wrong RuboCop")
                    require(metadata["ruby_version"] == "4.0.7", "wrong Ruby")
                    require(
                        metadata["ruby_platform"] == "x64-mingw-ucrt", "not Windows"
                    )
                    require(
                        [item["path"] for item in report["files"]] == names,
                        "path order",
                    )
                    summary = report["summary"]
                    require(summary["target_file_count"] == len(names), "target count")
                    require(
                        summary["inspected_file_count"] == len(names), "incomplete run"
                    )
                    require(
                        summary["offense_count"]
                        == sum(len(item["offenses"]) for item in report["files"]),
                        "offense count",
                    )
                    require(
                        all(
                            (work / name).read_bytes().hex() == source["cases"][name]
                            for name in names
                        ),
                        "reference changed source bytes",
                    )
                    current = {"exit": result.returncode, **report}
                    if expected is None:
                        expected = current
                    else:
                        require(current == expected, "mode/repeat differs")
            groups.append(
                {
                    "engine": engine,
                    "style": "native",
                    "config": original["config"],
                    **expected,
                }
            )
    require(len(groups) == 2, "missing engine")
    return {
        "reference": "Actual RuboCop 1.89.0 / Ruby 4.0.7 / parser3.3.12.0 / "
        "Prism1.8.1 / Windows x64 UCRT CLI",
        "source_fixture": "tests/fixtures/end_of_line_tokens.json",
        "source_fixture_sha256": hashlib.sha256(source_bytes).hexdigest(),
        "scope": "Windows native only; explicit lf/crlf and non-Windows native "
        "retain existing independently recorded groups",
        "reference_modes": ["inspect", "-a", "-A"],
        "repeats_per_mode": 2,
        "all_mode_repeat_reports_identical": True,
        "all_input_bytes_unchanged": True,
        "groups": groups,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--ruby", default="ruby")
    args = parser.parse_args()
    source = (
        Path(__file__).resolve().parents[1] / "tests/fixtures/end_of_line_tokens.json"
    )
    args.output.write_text(
        json.dumps(record(args.ruby, source), ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )


if __name__ == "__main__":
    main()
