#!/usr/bin/env python3
"""Windows Ruby 4.0.7 の OS argv 境界を候補から独立して記録する。"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REVISION = "229531a6cfbf07e3caef30dbac24a2a3f3fed482"
ENV_KEYS = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
RUBY_PROBE = """require 'json'
STDOUT.binmode
print JSON.generate(version:RUBY_VERSION,revision:RUBY_REVISION,words:ARGV)
"""
CONFIG = (
    "AllCops:\n  NewCops: disable\n  UseProjectIndex: false\n"
    "  TargetRubyVersion: 3.3\n  ParserEngine: parser_prism\n"
)


def _require(condition, context):
    if not condition:
        raise RuntimeError(f"reference contract failed: {context}")


def _tails():
    cases = []
    values = [
        ("lf", "\n"),
        ("crlf", "\r\n"),
        ("space", " "),
        ("tab", "\t"),
        ("vt", "\v"),
        ("ff", "\f"),
        ("cr", "\r"),
        ("lf_twice", "\n\n"),
        ("lf_space", "\n "),
        ("empty", ""),
    ]
    following = [
        ("known", ["Layout/SpaceAroundOperators", "--format", "json", "case.rb"]),
        ("unknown", ["Unknown/Cop", "--format", "json", "case.rb"]),
        ("option", ["--format", "json", "case.rb"]),
        (
            "other-selector",
            ["--except", "Layout/SpaceAroundOperators", "--format", "json", "case.rb"],
        ),
        ("end", []),
    ]
    for flag in ("--only", "--except"):
        for label, value in values:
            for suffix, words in following:
                cases.append(
                    (
                        f"legacy/{flag}/{label}/{suffix}",
                        subprocess.list2cmdline([flag, value, *words]),
                    )
                )
    for label, value in (
        ("lf", "\n"),
        ("crlf", "\r\n"),
        ("vt", "\v"),
        ("ff", "\f"),
        ("cr", "\r"),
    ):
        cases.append(("quoted/" + label, '--only "' + value + '" Known case.rb'))
    for label, value in (
        ("inside_vt", "Known\vTail"),
        ("leading_vt", "\vKnown"),
        ("inside_crlf", "Known\r\nTail"),
        ("inside_lf", "Known\nTail"),
    ):
        cases.append(
            ("position/" + label, subprocess.list2cmdline(["--only", value, "case.rb"]))
        )
    # 先行の実CLI18条件についても、同じ生commandlineの開始時ARGVを保持する。
    for flag in ("--only", "--except"):
        prefix = [
            "--config",
            "config.yml",
            "--cache",
            "false",
            "--format",
            "json",
            flag,
        ]
        for label, value in (
            ("lf", "\n"),
            ("crlf", "\r\n"),
            ("space", " "),
            ("vt", "\v"),
        ):
            cases.append(
                (
                    f"cli/{flag}/{label}",
                    subprocess.list2cmdline(
                        [*prefix, value, "Layout/SpaceAroundOperators", "case.rb"]
                    ),
                )
            )
        for label, tail in (
            ("unknown", ["Unknown/Cop", "case.rb"]),
            ("option", ["--format", "json", "case.rb"]),
            ("end", []),
        ):
            cases.append(
                (f"cli/{flag}/{label}", subprocess.list2cmdline([*prefix, "\n", *tail]))
            )
        cases.append(
            (
                f"cli/{flag}/quoted",
                subprocess.list2cmdline(prefix)
                + ' "\n" Layout/SpaceAroundOperators case.rb',
            )
        )
        cases.append(
            (
                f"cli/{flag}/lf-space",
                subprocess.list2cmdline(
                    [*prefix, "\n ", "Layout/SpaceAroundOperators", "case.rb"]
                ),
            )
        )
    for label, control in (
        ("lf", "\n"),
        ("crlf", "\r\n"),
        ("vt", "\v"),
        ("ff", "\f"),
        ("cr", "\r"),
    ):
        for quote in ("", "'", '"'):
            for place, value in (
                ("only", control),
                ("leading", control + "Known"),
                ("inside", "left" + control + "right"),
            ):
                cases.append(
                    (
                        f"extended/{label}/{quote!r}/{place}",
                        "--only " + quote + value + quote + " Tail",
                    )
                )
        for count in range(6):
            cases.append(
                (
                    f"extended/{label}/slashes-{count}",
                    '--only "prefix' + "\\" * count + control + '" Tail',
                )
            )
        for name, value in (
            ("double", '"a""b' + control + 'c"'),
            ("single", "'a\"b" + control + "c'"),
            ("mixed", "\"a'b" + control + 'c"'),
            ("path", '"C:\\space dir\\' + control + 'file.rb"'),
            ("emoji", '"日本語💎é' + control + '終"'),
            ("escaped", '"a\\"b' + control + 'c"'),
        ):
            cases.append((f"extended/{label}/{name}", "--only " + value + " Tail"))
    for label, value in (
        ("empty", '""'),
        ("space", '" "'),
        ("tab", '"\t"'),
        ("nbsp", "a b"),
        ("line-separator", "a\u2028b"),
        ("trailing-backslash", '"C:\\work dir\\\\"'),
    ):
        cases.append(("ordinary/" + label, "--only " + value + " Tail"))
    _require(
        len(cases) == len({name for name, _ in cases}) == 238,
        "reference contract failed",
    )
    return cases


def _environment():
    environment = dict(os.environ)
    for key in ENV_KEYS:
        environment.pop(key, None)
    return environment


def _capture(ruby, probe, case, environment):
    name, tail = case
    line = subprocess.list2cmdline([ruby, str(probe)]) + " " + tail
    result = subprocess.run(
        line, env=environment, capture_output=True, check=False, timeout=30
    )
    _require(result.returncode == 0 and (not result.stderr), (name, result.stderr))
    report = json.loads(result.stdout)
    _require(report["version"] == "4.0.7" and report["revision"] == REVISION, report)
    _require(isinstance(report["words"], list), "reference contract failed")
    _require(
        all(isinstance(word, str) for word in report["words"]),
        "reference contract failed",
    )
    _require(report["words"] and report["words"][0] == tail.split(" ", 1)[0], name)
    return {"name": name, "tail": tail, "words": report["words"]}


def _cli(ruby, wrapper, environment):
    records = []
    for flag in ("--only", "--except"):
        for label, control, stdin in (
            ("lf", "\n", False),
            ("crlf", "\r\n", False),
            ("vt", "\v", False),
            ("ordinary", "", False),
            ("stdin-lf", "\n", True),
        ):
            with tempfile.TemporaryDirectory(
                prefix="sonicop-windows-argv-cli-"
            ) as temporary:
                cwd = Path(temporary)
                source = b"x=1\r\n"
                (cwd / "input.rb").write_bytes(source)
                (cwd / "config.yml").write_text(CONFIG, encoding="utf-8", newline="\n")
                tail = (
                    "--config config.yml --cache false --format json "
                    + flag
                    + " "
                    + control
                    + " Layout/SpaceAroundOperators "
                )
                tail += "--stdin input.rb" if stdin else "input.rb"
                result = subprocess.run(
                    subprocess.list2cmdline([ruby, str(wrapper)]) + " " + tail,
                    cwd=cwd,
                    env=environment,
                    input=source if stdin else None,
                    capture_output=True,
                    check=False,
                    timeout=180,
                )
                _require(
                    result.returncode in (0, 1) and (not result.stderr), result.stderr
                )
                report = json.loads(result.stdout)
                _require(
                    report["metadata"]["rubocop_version"] == "1.89.0",
                    "reference contract failed",
                )
                _require(
                    report["summary"]["target_file_count"] == 1,
                    "reference contract failed",
                )
                _require(
                    report["summary"]["inspected_file_count"] == 1,
                    "reference contract failed",
                )
                _require(
                    len(report["files"]) == 1
                    and report["files"][0]["path"] == "input.rb",
                    "reference contract failed",
                )
                _require(
                    report["summary"]["offense_count"]
                    == len(report["files"][0]["offenses"]),
                    "reference contract failed",
                )
                _require(
                    (cwd / "input.rb").read_bytes() == source,
                    "reference contract failed",
                )
                records.append(
                    {
                        "name": flag + "/" + label,
                        "tail": tail,
                        "stdin": stdin,
                        "source_hex": source.hex(),
                        "config": CONFIG,
                        "exit": result.returncode,
                        "files": report["files"],
                        "summary": report["summary"],
                        "raw_stdout_hex": result.stdout.hex(),
                        "raw_stderr_hex": result.stderr.hex(),
                    }
                )
    _require(len(records) == 10, "reference contract failed")
    return records


def main():
    """版・件数・全原本bytesを確認し、公開容器だけをLFで保存する。

    Raises:
        RuntimeError: Windowsや固定仕様版の実行条件を満たさない場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--ruby", default="ruby")
    args = parser.parse_args()
    _require(not args.output.exists(), "output already exists")
    if sys.platform != "win32":
        raise RuntimeError("The reference collector requires Windows Ruby 4.0.7")
    environment = _environment()
    with tempfile.TemporaryDirectory(prefix="sonicop-windows-argv-") as temporary:
        root = Path(temporary)
        probe = root / "probe.rb"
        probe.write_text(RUBY_PROBE, encoding="utf-8", newline="\n")
        cases = [_capture(args.ruby, probe, case, environment) for case in _tails()]
        wrapper = root / "reference.rb"
        wrapper.write_text(
            "gem 'parser','3.3.12.0'\ngem 'prism','1.8.1'\n"
            "gem 'rubocop-ast','1.50.0'\ngem 'rainbow','3.1.1'\n"
            "gem 'rubocop','1.89.0'\n"
            "raise 'unexpected Ruby identity' unless RUBY_VERSION == '4.0.7' && "
            f"RUBY_REVISION == '{REVISION}'\n"
            "{'parser'=>'3.3.12.0', 'prism'=>'1.8.1', 'rubocop-ast'=>'1.50.0', "
            "'rainbow'=>'3.1.1', 'rubocop'=>'1.89.0'}.each do |name, version|\n"
            "  raise 'unexpected gem identity' unless "
            "Gem.loaded_specs.fetch(name).version.to_s == version\nend\n"
            "load Gem.bin_path('rubocop','rubocop','1.89.0')\n",
            encoding="utf-8",
            newline="\n",
        )
        cli = _cli(args.ruby, wrapper, environment)
    report = {
        "ruby_version": "4.0.7",
        "ruby_revision": REVISION,
        "environment": {key: None for key in ENV_KEYS},
        "cases": cases,
        "cli": cli,
    }
    with args.output.open("x", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(report, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
