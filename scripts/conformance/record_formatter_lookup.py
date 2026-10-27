"""未知builtin formatterの本家見出しとFile初期化を実CLIから記録する。"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from record_formatter_colors import LOADER, VERSIONS, _environment

CONFIG = b"AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
SOURCE = b"x=1\n"


def _require(condition, message):
    if not condition:
        raise RuntimeError(message)


def _cases():
    cases = [
        ("unknown-first", ["-f", "unknown_formatter", "-o", "first.txt"]),
        (
            "json-then-unknown",
            ["-f", "json", "-o", "first.txt", "-f", "unknown_formatter", "-o", "second.txt"],
        ),
        (
            "unknown-stops-later",
            ["-f", "unknown_formatter", "-o", "first.txt", "-f", "json", "-o", "second.txt"],
        ),
    ]
    for label, name in (
        ("unicode", "未知formatter"),
        ("quote", 'bad" formatter'),
        ("backslash", "bad\\ formatter"),
        ("newline", "bad\n formatter"),
        ("carriage-return", "bad\r formatter"),
        ("vertical-tab", "bad\v formatter"),
        ("form-feed", "bad\f formatter"),
        ("interpolation", "bad#{value}"),
        ("nonprintable", "bad\u0085 formatter"),
    ):
        cases.append((label, ["-f", name, "-o", "first.txt"]))
    for name in ("jsn", "progres", "file"):
        cases.append(("suggest-" + name, ["-f", name, "-o", "first.txt"]))
    cases += [
        ("color-unknown", ["--color", "-f", "unknown_formatter", "-o", "first.txt"]),
        ("color-suggestion", ["--color", "-f", "jsn", "-o", "first.txt"]),
    ]
    return cases


def _record(ruby, project, case, environment, evidence=None):
    name, formats = case
    project.mkdir()
    (project / "a.rb").write_bytes(SOURCE)
    (project / "config.yml").write_bytes(CONFIG)
    for stem in ("first", "second"):
        (project / (stem + ".txt")).write_bytes((stem + "-original\n").encode())
    original = {
        path: (path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_mode)
        for path in (project / "a.rb", project / "config.yml")
    }
    args = [
        "--config",
        "config.yml",
        "--cache",
        "false",
        "--only",
        "Layout/SpaceAroundOperators",
    ]
    if "--color" not in formats:
        args.append("--no-color")
    args += [*formats, "a.rb"]
    loader = LOADER.replace(
        "load Gem.bin_path(",
        'File.binwrite("formatter_source.json", JSON.generate('
        'Gem.loaded_specs.fetch("rubocop").full_gem_path + '
        '"/lib/rubocop/formatter/formatter_set.rb")); load Gem.bin_path(',
        1,
    )
    result = subprocess.run(
        [ruby, "-e", loader, "--", *args],
        cwd=project,
        env=environment,
        capture_output=True,
        check=False,
        timeout=90,
    )
    if evidence is not None:
        (evidence / (name + ".stdout")).open("xb").write(result.stdout)
        (evidence / (name + ".stderr")).open("xb").write(result.stderr)
    _require(
        result.returncode == 2 and result.stdout == b"",
        "reference did not reject unknown formatter",
    )
    _require(
        json.loads((project / "versions.json").read_bytes()) == VERSIONS,
        "loaded reference versions differ",
    )
    source_path = json.loads((project / "formatter_source.json").read_bytes())
    frame = re.search(rb"(?m)^" + re.escape(source_path.encode()) + rb":\d+:in ", result.stderr)
    _require(frame is not None and frame.start() > 0, "reference formatter frame is missing")
    message = result.stderr[: frame.start()]
    plain = re.sub(rb"\x1b\[[0-9;]*m", b"", message)
    _require(
        plain.startswith(b'Formatter "')
        and b"not found." in plain
        and message.endswith(b"\r\n" if os.name == "nt" else b"\n"),
        "reference formatter error header is missing",
    )
    _require(
        all(
            (path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_mode) == value
            for path, value in original.items()
        ),
        "reference changed input bytes/stat",
    )
    files = {name: (project / name).read_bytes().hex() for name in ("first.txt", "second.txt")}
    _require(files["first.txt"] == "", "first output was not opened")
    expected_second = b"" if formats[1] == "json" else b"second-original\n"
    _require(files["second.txt"] == expected_second.hex(), "later output initialization differs")
    return {
        "name": name,
        "args": args,
        "exit": result.returncode,
        "stdout_hex": result.stdout.hex(),
        "stderr_message_prefix_hex": message.hex(),
        "files_hex": files,
    }


def main():
    """候補を使わず、Ruby固有stackのraw原本を任意の私有領域へ分ける。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--evidence-directory", type=Path)
    options = parser.parse_args()
    _require(not options.output.exists(), "output already exists")
    if options.evidence_directory is not None:
        options.evidence_directory.mkdir(parents=True, exist_ok=False)
    environment = _environment()
    parent = dict(os.environ)
    with tempfile.TemporaryDirectory(prefix="sonicop-formatter-lookup-") as directory:
        root = Path(directory)
        cases = [
            _record(options.ruby, root / case[0], case, environment, options.evidence_directory)
            for case in _cases()
        ]
    _require(
        dict(os.environ) == parent and len(cases) == 17, "reference environment or case set differs"
    )
    report = {
        "reference": VERSIONS,
        "platform": sys.platform + "-pipe-non-tty",
        "source_hex": SOURCE.hex(),
        "config_hex": CONFIG.hex(),
        "scope": "builtin formatter error message prefix and physical output files; "
        "Ruby implementation backtrace is not a new full-stderr equality",
        "cases": cases,
    }
    with options.output.open("x", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(report, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
