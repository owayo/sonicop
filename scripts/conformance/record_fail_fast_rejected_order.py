"""未知copの一覧表示にもdirectory順序を適用する実本家の2条件を採取する。"""

import argparse
import json
import os
import pathlib
import tempfile

import record_fail_fast_order as reference


def _record(ruby, launcher, base, stdin):
    project = base / "project"
    project.mkdir(parents=True, exist_ok=False)
    (base / "config.yml").write_bytes(reference.CONFIG.encode())
    files = {"a.rb": b"x=1\n".hex(), "b.rb": b"y=2\n".hex()}
    mtimes = {"a.rb": 1_700_000_000_100_000_000, "b.rb": 1_700_000_001_100_000_000}
    for name, value in files.items():
        (project / name).write_bytes(bytes.fromhex(value))
        os.utime(project / name, ns=(mtimes[name], mtimes[name]))
    before = reference._stamp(project, files)
    if any(before[name]["mtime_ns"] != ns for name, ns in mtimes.items()):
        raise ValueError("filesystem did not preserve requested mtime")
    args = [
        "--config",
        "../config.yml",
        "--cache",
        "false",
        "--only",
        "Unknown/Cop",
        "--fail-fast",
        "-L",
    ]
    args += ["--stdin", "."] if stdin else ["."]
    source = b"shared = true\n" if stdin else None
    result = reference._run([ruby, str(launcher), *args], project, source)
    listing = result.stdout.decode().replace("\r\n", "\n").splitlines()
    if result.returncode != 0 or result.stderr or len(listing) != 2 or set(listing) != set(files):
        raise ValueError("rejected-selection listing is incomplete")
    if (
        before != reference._stamp(project, files)
        or (base / "config.yml").read_bytes() != reference.CONFIG.encode()
    ):
        raise ValueError("reference changed physical input")
    return {
        "name": "stdin-directory" if stdin else "disk-directory",
        "args": args,
        "files": files,
        "mtime_ns": mtimes,
        "stdin_hex": source.hex() if source is not None else None,
        "exit": result.returncode,
        "stdout_hex": result.stdout.hex(),
        "stderr_hex": result.stderr.hex(),
        "listing": listing,
        "input_before": before,
        "input_after": reference._stamp(project, files),
    }


def collect(ruby, root):
    """固定runtimeの実CLIだけから、欠落のない2条件を記録する。

    Returns:
        dict: 実runtimeと2条件のraw出力・原本stat。

    """
    root.mkdir(parents=True, exist_ok=False)
    launcher = root / "reference.rb"
    launcher.write_text(reference.LAUNCHER, encoding="utf-8", newline="\n")
    runtime = reference._runtime(ruby, launcher, root)
    cases = [
        _record(ruby, launcher, root / name, stdin)
        for name, stdin in (("disk", False), ("stdin", True))
    ]
    return {"runtime": runtime, "config": reference.CONFIG, "cases": cases}


def main():
    """既存記録を上書きせず、候補なしで実本家の記録を保存する。

    Raises:
        ValueError: 出力先が使用済みの場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    options = parser.parse_args()
    if options.output.exists():
        raise ValueError("recording output already exists")
    with tempfile.TemporaryDirectory(prefix="sonicop-rejected-order-") as directory:
        document = collect(options.ruby, pathlib.Path(directory) / "recording")
    options.output.parent.mkdir(parents=True, exist_ok=True)
    options.output.write_text(
        json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n"
    )


if __name__ == "__main__":
    main()
