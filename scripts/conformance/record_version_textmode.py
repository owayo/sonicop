"""本家versionの輸送・出力先・早期終了だけを固定し、候補は起動しない。"""

import argparse
import json
import subprocess
from pathlib import Path

from record_formatter_colors import LOADER as CLI_LOADER
from record_formatter_colors import VERSIONS as BASE_VERSIONS
from record_formatter_colors import _environment as reference_environment

LOADER = CLI_LOADER[: CLI_LOADER.index("load Gem.bin_path(")]
VERSIONS = {
    "parser": BASE_VERSIONS["parser"],
    "prism": BASE_VERSIONS["prism"],
    "rubocop-ast": BASE_VERSIONS["rubocop_ast"],
    "rubocop": BASE_VERSIONS["rubocop"],
    "rainbow": "3.1.1",
}

FILES = {
    "Gemfile": "",
    ".rubocop.yml": (
        "AllCops:\n  UseCache: false\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
        "  SuggestExtensions: false\n  DisabledByDefault: true\n"
        "Layout/SpaceAroundOperators:\n  Enabled: true\n"
    ),
    "good.rb": "x=1\n",
}
METADATA = (
    LOADER
    + """
require 'json'
require 'digest'
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
puts JSON.generate(
  ruby: RUBY_VERSION, revision: RUBY_REVISION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow].to_h do |name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]
  end,
  source_sha256: %w[cli.rb options.rb version.rb cli/command/version.rb].to_h do |file|
    [file, Digest::SHA256.file(File.join(root, 'lib/rubocop', file)).hexdigest]
  end
)
"""
)


def inputs():
    """alias・routing・out保護と既存cache拒否に限定した20条件を返す。

    Returns:
        list: 実行順を固定した本家CLI入力。

    """
    rows = []
    for name, flag, kind in (
        ("long-version", "--version", "version"),
        ("short-version", "-v", "version"),
        ("long-verbose", "--verbose-version", "verbose"),
        ("short-verbose", "-V", "verbose"),
    ):
        for suffix, extra in (("plain", []), ("stderr", ["--stderr"])):
            rows.append(
                {
                    "id": f"{name}-{suffix}",
                    "kind": kind,
                    "args": [flag, *extra, "--format", "json", "good.rb"],
                    "files": dict(FILES),
                }
            )
    for name, flag, kind in (
        ("version", "--version", "version"),
        ("verbose", "--verbose-version", "verbose"),
    ):
        for suffix, extra, out in (
            ("out-absent", ["--out", "out.txt"], None),
            ("out-sentinel", ["--out", "out.txt"], "KEEP\x00\r\n"),
            ("parallel", ["--parallel"], None),
            ("missing-input", [], None),
            ("invalid-cache", ["--cache", "bad"], None),
            ("invalid-cache-out", ["--cache", "bad", "--out", "out.txt"], "KEEP\x00\r\n"),
        ):
            files = dict(FILES)
            if out is not None:
                files["out.txt"] = out
            rows.append(
                {
                    "id": f"{name}-{suffix}",
                    "kind": "rejection" if suffix.startswith("invalid-cache") else kind,
                    "args": [
                        flag,
                        *extra,
                        "--format",
                        "json",
                        "missing.rb" if suffix == "missing-input" else "good.rb",
                    ],
                    "files": files,
                }
            )
    for row in rows:
        row["args"].insert(0, "--no-color")
        if row["kind"] == "rejection":
            row["residual_issue"] = 137
    return rows


def validate(document):
    """候補identityと本家identityを同一視せず、輸送と完了条件を検査する。

    Raises:
        ValueError: 版・入力・raw channel・元byteの証拠が欠けた場合。

    """
    runtime = document["runtime"]
    if (
        runtime["ruby"] != "4.0.7"
        or runtime["revision"] != "229531a6cfbf07e3caef30dbac24a2a3f3fed482"
        or runtime["platform"] != "x64-mingw-ucrt"
        or runtime["gems"] != VERSIONS
    ):
        raise ValueError("unexpected runtime")
    if runtime["source_sha256"] != {
        "cli.rb": "7a097a2918f7bf4b03b71a49d868910f243b78aa26e31035520822709d33e933",
        "options.rb": "adfe551344fc98ea2d13f21852b56ff1cd445a6fdf36cd0bde8cd2a154a4d8c4",
        "version.rb": "28569eb9621dc2f5e4f4fabd2d96e6c9ca3474a3ef963a15458a52a0da19934b",
        "cli/command/version.rb": (
            "364f85d7c1e113fcd140eb9804d57b42a9fc2cc4b71a17adb7528efef2553f2b"
        ),
    }:
        raise ValueError("unexpected upstream source content")
    expected = inputs()
    if len(document["cases"]) != len(expected):
        raise ValueError("incomplete input set")
    for case, row in zip(expected, document["cases"], strict=True):
        if any(row[key] != case[key] for key in ("id", "kind", "args", "files")):
            raise ValueError("input or order changed")
        stdout, stderr = bytes.fromhex(row["stdout_hex"]), bytes.fromhex(row["stderr_hex"])
        if case["kind"] == "rejection":
            if row.get("residual_issue") != 137:
                raise ValueError("cache/version residual binding changed")
            if (
                row["exit"] != 2
                or stdout
                or stderr != b"-C/--cache argument must be true or false\r\n"
            ):
                raise ValueError("invalid cache rejection changed")
        elif (
            row["exit"] != 0
            or stderr
            or stdout != (document["reference_lines"][case["kind"]] + "\r\n").encode()
        ):
            raise ValueError("version body, routing or textmode changed")
        if (
            row["after_hex"]
            != {name: value.encode().hex() for name, value in case["files"].items()}
            or not row["original_bytes_mtime_mode_equal"]
        ):
            raise ValueError("original files changed")
        out = case["files"].get("out.txt")
        if row["out_exists"] != (out is not None) or row["out_hex"] != (
            None if out is None else out.encode().hex()
        ):
            raise ValueError("early exit touched output file")


def record(ruby, output, work):
    """既存記録を先に拒否し、実本家の20条件を別directoryへ記録する。

    Raises:
        FileExistsError: 出力または作業directoryが既に占有されている場合。
        ValueError: 原本・runtime・参照出力が契約と違う場合。

    """
    if output.exists() or output.is_symlink():
        raise FileExistsError(output)
    work.mkdir(parents=True, exist_ok=False)
    env = reference_environment()
    metadata = subprocess.run(
        [str(ruby), "-e", METADATA], cwd=work, env=env, capture_output=True, check=True, timeout=40
    )
    if metadata.stderr:
        raise ValueError("metadata wrote stderr")
    runtime = json.loads(metadata.stdout)
    wrapper = work / "reference.rb"
    wrapper.write_text(
        LOADER + "\nload Gem.bin_path('rubocop','rubocop','1.89.0')\n",
        encoding="utf-8",
        newline="\n",
    )
    rows = []
    for case in inputs():
        cwd = work / case["id"]
        cwd.mkdir()
        for name, value in case["files"].items():
            (cwd / name).write_bytes(value.encode())
        before = {
            name: (
                (cwd / name).read_bytes(),
                (cwd / name).stat().st_mtime_ns,
                (cwd / name).stat().st_mode,
            )
            for name in case["files"]
        }
        result = subprocess.run(
            [str(ruby), str(wrapper), *case["args"]],
            cwd=cwd,
            env=env,
            capture_output=True,
            timeout=40,
        )
        after = {
            name: (
                (cwd / name).read_bytes(),
                (cwd / name).stat().st_mtime_ns,
                (cwd / name).stat().st_mode,
            )
            for name in case["files"]
        }
        if before != after:
            raise ValueError("reference changed original bytes or mtime")
        out = cwd / "out.txt"
        row = {
            **case,
            "exit": result.returncode,
            "stdout_hex": result.stdout.hex(),
            "stderr_hex": result.stderr.hex(),
            "after_hex": {name: value[0].hex() for name, value in after.items()},
            "original_bytes_mtime_mode_equal": before == after,
            "out_exists": out.exists(),
            "out_hex": out.read_bytes().hex() if out.exists() else None,
        }
        (cwd / "raw.json").write_text(
            json.dumps(row, indent=2) + "\n", encoding="utf-8", newline="\n"
        )
        (cwd / "original-stat.json").write_text(
            json.dumps(
                {
                    name: {"before_mtime_ns": before[name][1], "after_mtime_ns": value[1]}
                    for name, value in after.items()
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
            newline="\n",
        )
        rows.append(row)
    document = {
        "runtime": runtime,
        "scope": (
            "Windows pipe version transport; native identity retained separately; "
            "16 transport/early-exit conditions; 4 invalid-cache controls remain issue137"
        ),
        # envなしのVersion.verbose APIは親gemspecの版を拾うため、実CLI本文だけを保存する。
        "reference_lines": {
            kind: bytes.fromhex(next(row["stdout_hex"] for row in rows if row["kind"] == kind))
            .decode()
            .removesuffix("\r\n")
            for kind in ("version", "verbose")
        },
        "cases": rows,
    }
    validate(document)
    with output.open("x", encoding="utf-8", newline="\n") as file:
        file.write(json.dumps(document, indent=2) + "\n")


def main():
    """新規出力先と固定Rubyを明示し、原本記録を上書きしない。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    record(args.ruby.resolve(), args.output.absolute(), args.work_dir.resolve())


if __name__ == "__main__":
    main()
