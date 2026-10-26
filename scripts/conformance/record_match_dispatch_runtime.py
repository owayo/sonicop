"""自作 core-only match dispatch 入力を指定した二つの MRI で再実行する。"""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _expand(part):
    return (part["prefix"] + part["padding"] * part["count"] + part["suffix"]).encode()


def main():
    """候補や外部 project source を読まず、保存済み最小入力だけを実行する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("fixture", "out", "work", "ruby27", "ruby40"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    source = Path(args.fixture).resolve()
    destination = Path(args.out).resolve()
    _require(source != destination, "正本の実行記録は上書きしない")
    fixture = json.loads(source.read_bytes())
    _require(fixture["reference"]["core_only"] is True, "core-only fixtureが必要")
    _require(len(fixture["cases"]) == 26, "入力数不一致")
    runtimes = {"2.7.8": args.ruby27, "4.0.7": args.ruby40}
    # 起動環境の require を取り込まず、自作の core-only 本文だけを実行する。
    environment = os.environ.copy()
    environment.pop("RUBYOPT", None)
    environment.pop("RUBYLIB", None)
    for version, executable in runtimes.items():
        probe = subprocess.run(
            [executable, "--disable-gems", "-v"],
            capture_output=True,
            check=False,
            timeout=10,
            env=environment,
        )
        _require(probe.returncode == 0 and not probe.stderr, "Ruby版probe失敗")
        _require(
            probe.stdout.decode().startswith("ruby " + version + " ")
            or probe.stdout.decode().startswith("ruby " + version + "p"),
            "Ruby版不一致",
        )
    directory = Path(args.work).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    for case in fixture["cases"]:
        _require("candidate" not in case["id"], "候補を正本にしない")
        _require(Path(case["id"]).name == case["id"], "入力名不正")
        data = _expand(case["source"])
        _require(hashlib.sha256(data).hexdigest() == case["source_sha256"], "実行入力hash不一致")
        path = directory / (case["id"] + ".rb")
        path.write_bytes(data)
        observed = {}
        for version, executable in runtimes.items():
            observed[version] = {}
            for mode in ("syntax", "run"):
                command = [executable, "--disable-gems"]
                if mode == "syntax":
                    command.append("-c")
                command.append(path.name)
                result = subprocess.run(
                    command,
                    cwd=directory,
                    capture_output=True,
                    check=False,
                    timeout=10,
                    env=environment,
                )
                _require(
                    result.returncode == 0 and not result.stderr,
                    (case["id"], version, result.stderr),
                )
                observed[version][mode] = {
                    "exit": result.returncode,
                    "stdout_hex": result.stdout.hex(),
                    "stderr_hex": result.stderr.hex(),
                }
        case["observations"] = observed
    destination.write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
    )
    print("recorded 104 core-only syntax/runtime executions")


if __name__ == "__main__":
    main()
