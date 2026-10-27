"""実RuboCopからdirectory stdinと拒否時debugの6条件を直列で記録する。"""

import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

import record_debug_streams as reference


def forms():
    """対象ゼロ・拒否・一覧を分け、同じ条件の重複で未検証を埋めない。"""
    return [
        ("directory", False, False, False, False),
        ("directory_stderr", False, False, False, True),
        ("empty_directory", False, False, True, False),
        ("listing", False, True, False, False),
        ("unknown_stderr", True, False, False, True),
        ("unknown_listing", True, True, False, False),
    ]


def _state(path):
    info = path.stat()
    return {"hex": path.read_bytes().hex(), "mtime_ns": info.st_mtime_ns, "mode": info.st_mode}


def validate(row, expected_paths, inspected, scanned):
    """未完了のJSON、stream、出力先を成功する参照へ変えない。"""
    stdout, stderr = row["stdout"], row["stderr"]
    reference._require(len(stdout["reference_headers"]) == 2, "configuration headerが欠落")
    reference._require(not stderr["reference_headers"], "configuration headerのstreamが不一致")
    reference._require(
        stdout["timer_count"] == 1 and stderr["timer_count"] == 0, "timerのstreamが不一致"
    )
    paths = re.findall(r"Scanning <cwd>/([^\r\n]+)", stdout["text"] + stderr["text"])
    reference._require(paths == scanned, "Scanning対象が不一致")
    destination = "stderr" if "--stderr" in row["args"] else "stdout"
    other = "stdout" if destination == "stderr" else "stderr"
    reference._require(
        re.findall(r"Scanning <cwd>/([^\r\n]+)", row[destination]["text"]) == scanned
        and not re.findall(r"Scanning <cwd>/([^\r\n]+)", row[other]["text"]),
        "Scanningのstreamが不一致",
    )
    reference._require(not stdout["reports"] and not stderr["reports"], "JSONが出力先から漏れた")
    if row["listing"]:
        reference._require(row["output"] is None, "一覧にformatter fileが生成された")
        listing = stdout["text"].removesuffix("Finished in <elapsed> seconds\r\n").splitlines()
        reference._require(
            len(listing) == len(expected_paths) and set(listing) == set(expected_paths),
            "一覧のpath集合が未完了",
        )
        reference._require(not stderr["text"], "一覧のstderrが空でない")
    else:
        reference._require(row["output"] is not None, "formatter fileが欠落")
        reference._validate_report(row["output"]["report"], len(expected_paths), inspected)


def _record(ruby, launcher, base, form):
    name, unknown, listing, empty, stderr = form
    base.mkdir(parents=True, exist_ok=False)
    project = base / "project"
    project.mkdir()
    config = base / "config.yml"
    config.write_bytes(reference.CONFIG.encode())
    inputs = {} if empty else {"a.rb": "x=1\n", "b.rb": "y=2\n"}
    times = {"a.rb": 1_700_000_000_100_000_000, "b.rb": 1_700_000_001_100_000_000}
    for path, source in inputs.items():
        target = project / path
        target.write_bytes(source.encode())
        os.utime(target, ns=(times[path], times[path]))
    before = {path: _state(project / path) for path in inputs}
    config_before = _state(config)
    args = [
        "--config",
        "../config.yml",
        "--cache",
        "false",
        "--no-color",
        "--debug",
        "--only",
        "Unknown/Cop" if unknown else reference.SPACE,
    ]
    if stderr:
        args += ["--stderr"]
    args += ["--stdin", "."]
    args += ["--fail-fast", "-L"] if listing else ["--format", "json", "--out", "report.out"]
    done = subprocess.run(
        [ruby, str(launcher), *args],
        cwd=project,
        env=reference._environment(),
        input=reference.SOURCE.encode(),
        capture_output=True,
        check=False,
        timeout=60,
    )
    expected_exit = 0 if listing or empty else 2 if unknown else 1
    reference._require(done.returncode == expected_exit, "本家終了値が不一致")
    reference._require(b"An error occurred" not in done.stderr, "本家内部エラー")
    if unknown and not listing:
        reference._require(
            b"Unrecognized cop or department: Unknown/Cop." in done.stderr,
            "想定した未知cop拒否ではない",
        )
    output_path = project / "report.out"
    output = None
    if output_path.exists():
        raw = output_path.read_bytes()
        output = {"raw_hex": raw.hex(), "report": reference._report(raw)}
    row = {
        "id": name,
        "args": args,
        "inputs": inputs,
        "mtime_ns": {path: times[path] for path in inputs},
        "stdin_source": reference.SOURCE,
        "target_count": len(inputs),
        "listing": listing,
        "exit": done.returncode,
        "stdout": reference._project_reference(done.stdout, project.as_posix()),
        "stderr": reference._project_reference(done.stderr, project.as_posix()),
        "output": output,
        "raw_stdout_sha256": hashlib.sha256(done.stdout).hexdigest(),
        "raw_stderr_sha256": hashlib.sha256(done.stderr).hexdigest(),
    }
    scanned = [] if listing or empty else ["a.rb"] if unknown else list(inputs)
    inspected = [] if unknown else list(inputs)
    validate(row, list(inputs), inspected, scanned)
    after = {path: _state(project / path) for path in inputs}
    reference._require(
        before == after and config_before == _state(config), "原本byteまたは属性が変わった"
    )
    row["input_before"] = before
    row["input_after"] = after
    for stream in ("stdout", "stderr"):
        (base / (stream + ".raw")).write_bytes(getattr(done, stream))
    (base / "raw.json").write_text(
        json.dumps(
            {
                "args": args,
                "exit": done.returncode,
                "stdout_hex": done.stdout.hex(),
                "stderr_hex": done.stderr.hex(),
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return row


def main():
    """候補を読まず、未使用の出力先へ実本家の6条件だけを保存する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "launcher", "work", "out"):
        parser.add_argument("--" + name, required=True)
    options = parser.parse_args()
    work, output = Path(options.work), Path(options.out)
    reference._require(not work.exists() and not output.exists(), "採取先が使用済み")
    parent = dict(os.environ)
    runtime = reference._verify_runtime(options.ruby)
    work.mkdir(parents=True, exist_ok=False)
    cases = [_record(options.ruby, options.launcher, work / form[0], form) for form in forms()]
    reference._require(parent == dict(os.environ), "親の実行環境が変わった")
    document = {
        "runtime": runtime,
        "config": reference.CONFIG,
        "launcher_sha256": hashlib.sha256(Path(options.launcher).read_bytes()).hexdigest(),
        "cases": cases,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(document, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


if __name__ == "__main__":
    main()
