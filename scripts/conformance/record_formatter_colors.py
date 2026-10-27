"""RuboCop 1.89.0 の単一 formatter の色付けを実 CLI から記録する。"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

VERSIONS = {
    "ruby": "4.0.7",
    "rubocop": "1.89.0",
    "parser": "3.3.12.0",
    "prism": "1.8.1",
    "rubocop_ast": "1.50.0",
}
BASE_CONFIG = (
    b"AllCops:\n  NewCops: disable\n  UseProjectIndex: false\n"
    b"  TargetRubyVersion: 3.3\n  ParserEngine: parser_prism\n"
    b"  StyleGuideBaseURL: https://example.test/style\n"
    b"Layout/SpaceAroundOperators:\n"
)
LOADER = (
    'gem "rainbow", "3.1.1"; '
    'gem "parser", "3.3.12.0"; gem "prism", "1.8.1"; '
    'gem "rubocop-ast", "1.50.0"; gem "rubocop", "1.89.0"; '
    'require "json"; require "rubocop"; '
    # 公開24条件のmetadata shapeは保ち、paletteは実childのloaded版で厳密に照合する。
    'raise "unexpected Rainbow version" unless '
    'Gem.loaded_specs.fetch("rainbow").version.to_s == "3.1.1"; '
    'File.binwrite("versions.json", JSON.generate({ruby: RUBY_VERSION, '
    "rubocop: RuboCop::Version::STRING, "
    'parser: Gem.loaded_specs["parser"].version.to_s, '
    'prism: Gem.loaded_specs["prism"].version.to_s, '
    'rubocop_ast: Gem.loaded_specs["rubocop-ast"].version.to_s})); '
    'load Gem.bin_path("rubocop", "rubocop", "1.89.0")'
)


def _environment():
    # Gem/PATH/OS の探索は維持し、参照を変える4キーだけを子の copy から除く。
    environment = dict(os.environ)
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(key, None)
    return environment


def _cases():
    cases = [
        (severity, severity, [], b"x=1\n")
        for severity in ("info", "refactor", "convention", "warning", "error", "fatal")
    ]
    cases += [
        ("no-color", "convention", ["--no-color"], b"x=1\n"),
        ("no-cop", "convention", ["--no-display-cop-names"], b"x=1\n"),
        ("details-style", "convention", ["-E", "-S"], b"x=1\n"),
        ("corrected", "convention", ["-a"], b"x=1\n"),
        ("clean", "convention", [], b"x = 1\n"),
    ]
    return cases


def _require(condition, message):
    if not condition:
        raise RuntimeError(message)


def _record(
    ruby,
    root,
    case,
    formatter,
    environment,
    *,
    only="Layout/SpaceAroundOperators",
    extra_configuration=b"",
):
    name, severity, flags, source = case
    root.mkdir()
    config = BASE_CONFIG + f"  Severity: {severity}\n".encode()
    if name == "details-style":
        config += (
            b"  Details: 'note `detail`'\n  StyleGuide: '#operators'\n"
            b"  Reference: https://example.test/reference\n"
        )
    config += extra_configuration
    source_path = root / "case.rb"
    source_path.write_bytes(source)
    (root / "config.yml").write_bytes(config)
    before = source_path.stat().st_mtime_ns
    arguments = [
        "--config",
        "config.yml",
        "--cache",
        "false",
        "--only",
        only,
    ]
    if "--no-color" not in flags:
        arguments += ["--color"]
    arguments += flags + ["--format", formatter, "case.rb"]
    output = subprocess.run(
        [ruby, "-e", LOADER, "--", *arguments],
        cwd=root,
        env=environment,
        capture_output=True,
        check=False,
        timeout=90,
    )
    _require(output.returncode in (0, 1), "unexpected reference exit")
    _require(not output.stderr, "reference stderr is not empty")
    _require(
        json.loads((root / "versions.json").read_bytes()) == VERSIONS,
        "loaded reference versions differ",
    )
    projected = output.stdout.replace(b"\r\n", b"\n")
    # 空出力や途中終了を色の一致として記録しない。
    _require(
        b"1 file inspected" in re.sub(rb"\x1b\[[0-9;]*m", b"", projected),
        "reference did not finish inspection",
    )
    _require((root / "config.yml").read_bytes() == config, "reference changed configuration")
    if "-a" not in flags:
        _require(source_path.read_bytes() == source, "reference changed source")
        _require(source_path.stat().st_mtime_ns == before, "reference changed mtime")
    return {
        "name": name + "-" + formatter,
        "format": formatter,
        "severity": severity,
        "arguments": arguments,
        "source_hex": source.hex(),
        "configuration_hex": config.hex(),
        "expected": {
            "exit": output.returncode,
            "stdout_hex": output.stdout.hex(),
            "lf_projected_hex": projected.hex(),
            "stderr_hex": output.stderr.hex(),
            "source_after_hex": source_path.read_bytes().hex(),
            "ansi": [part.decode() for part in re.findall(rb"\x1b\[[0-9;]*m", projected)],
        },
    }


def collect(ruby):
    """候補を起動せず、22主条件とTapの共有境界2条件を独立して保存する。

    Returns:
        dict: 本家の版、実行条件、raw stdout/stderr と原本bytesを含む記録。

    """
    environment = _environment()
    with tempfile.TemporaryDirectory(prefix="sonicop-formatter-colors-") as directory:
        root = Path(directory)
        cases = [
            _record(ruby, root / (case[0] + "-" + formatter), case, formatter, environment)
            for case in _cases()
            for formatter in ("simple", "clang")
        ]
        controls = [
            _record(ruby, root / ("tap-" + case[0]), case, "tap", environment)
            for case in _cases()
            if case[0] in ("convention", "no-color")
        ]
    _require(len(cases) == 22 and len(controls) == 2, "reference case set differs")
    return {
        "reference": VERSIONS,
        "platform": sys.platform + "-pipe-non-tty",
        "cases": cases,
        "tap_controls": controls,
    }


def collect_supplemental(ruby):
    """部分補正のcyanとUnicodeの引用境界を本家だけで追加計測する。"""
    environment = _environment()
    cases = []
    with tempfile.TemporaryDirectory(prefix="sonicop-color-boundaries-") as directory:
        root = Path(directory)
        for formatter in ("simple", "clang"):
            for color in (True, False):
                name = "partial" if color else "partial-no-color"
                flags = ["-a"] if color else ["-a", "--no-color"]
                cases.append(
                    _record(
                        ruby,
                        root / (name + "-" + formatter),
                        (name, "convention", flags, b'x=1\nputs "abcdefghijklmnopqrstuvwxyz"\n'),
                        formatter,
                        environment,
                        only="Layout/SpaceAroundOperators,Layout/LineLength",
                        extra_configuration=b"Layout/LineLength:\n  Max: 20\n",
                    )
                )
        for formatter in ("simple", "clang", "tap"):
            cases.append(
                _record(
                    ruby,
                    root / ("unicode-" + formatter),
                    ("unicode-details", "convention", ["-E"], b"x=1\n"),
                    formatter,
                    environment,
                    extra_configuration="  Details: '前`漢字🙂`後`値`末'\n".encode(),
                )
            )
        cases.append(
            _record(
                ruby,
                root / "unmatched-simple",
                ("unicode-unmatched", "convention", ["-E"], b"x=1\n"),
                "simple",
                environment,
                extra_configuration="  Details: '前`漢字🙂末'\n".encode(),
            )
        )
    _require(len(cases) == 8, "supplemental case set differs")
    return {"reference": VERSIONS, "platform": sys.platform + "-pipe-non-tty", "cases": cases}


def main():
    """固定版の本家を実行し、JSONコンテナをLFで書く。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--supplemental", action="store_true")
    options = parser.parse_args()
    if options.output:
        _require(not options.output.exists(), "output already exists")
    collector = collect_supplemental if options.supplemental else collect
    encoded = (json.dumps(collector(options.ruby), indent=2) + "\n").encode("ascii")
    if options.output:
        options.output.parent.mkdir(parents=True, exist_ok=True)
        with options.output.open("xb") as output:
            output.write(encoded)
    else:
        sys.stdout.buffer.write(encoded)


if __name__ == "__main__":
    main()
