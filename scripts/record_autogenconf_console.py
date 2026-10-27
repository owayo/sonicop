#!/usr/bin/env python3
"""Autogenconf console の期待値を、固定版の実 CLI から保存する。"""

import argparse
import json
import os
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
from pathlib import Path

_ONLY = "Layout/SpaceAroundOperators"
_MIXED = _ONLY + ",Style/NumericPredicate"
_SOURCE = "x=1\r\n"
_CLEAN = "x = 1\r\n"
_MIXED_SOURCE = "x=1\r\nputs x == 0\r\n"


def _cases():
    cases = []

    def _add(
        name,
        files,
        paths,
        flags=None,
        only=_ONLY,
        formatter="autogenconf",
        source=None,
        outputs=None,
        yaml=None,
    ):
        args = ["--cache", "false", "--only", only]
        if yaml is None:
            args += ["--force-default-config", "--enable-pending-cops"]
        else:
            files = {**files, "config.yml": yaml}
            args += ["--config", "config.yml"]
        args += ["--format", formatter, *(flags or []), *paths]
        cases.append(
            {
                "name": name,
                "files": files,
                "args": args,
                "input_hex": None if source is None else source.encode().hex(),
                "outputs": outputs or [],
            }
        )

    _add("offense", {"sample.rb": _SOURCE}, ["sample.rb"])
    _add("clean", {"sample.rb": _CLEAN}, ["sample.rb"])
    _add("zero", {}, ["empty"])
    cases[-1]["directories"] = ["empty"]
    _add("two-offending", {"a.rb": _SOURCE, "b.rb": "y=2\r\n"}, ["a.rb", "b.rb"])
    _add("clean-then-offending", {"a.rb": _CLEAN, "b.rb": _SOURCE}, ["a.rb", "b.rb"])
    _add("explicit-reverse-order", {"a.rb": _CLEAN, "b.rb": _SOURCE}, ["b.rb", "a.rb"])
    _add(
        "fail-fast",
        {"a.rb": _SOURCE, "b.rb": _SOURCE},
        ["a.rb", "b.rb"],
        ["--fail-fast"],
    )
    cases[-1]["mtime"] = 100
    _add(
        "fail-fast-after-clean",
        {"a.rb": _CLEAN, "b.rb": _SOURCE, "c.rb": _SOURCE},
        ["a.rb", "b.rb", "c.rb"],
        ["--fail-fast"],
    )
    cases[-1]["mtime"] = 100
    _add("alias", {"sample.rb": _SOURCE}, ["sample.rb"], formatter="a")
    _add("unicode", {"sample.rb": "あ=1\r\n"}, ["sample.rb"])
    _add("without-final-newline", {"sample.rb": "x=1"}, ["sample.rb"])
    _add("lf-input", {"sample.rb": "x=1\n"}, ["sample.rb"])
    _add("color-offense", {"sample.rb": _SOURCE}, ["sample.rb"], ["--color"])
    _add("color-clean", {"sample.rb": _CLEAN}, ["sample.rb"], ["--color"])
    for severity in ["info", "refactor", "warning", "error", "fatal"]:
        yaml = (
            "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
            f"{_ONLY}:\n  Severity: {severity}\n"
        )
        _add(
            "color-" + severity,
            {"sample.rb": _SOURCE},
            ["sample.rb"],
            ["--color"],
            yaml=yaml,
        )
    for mode, label in [("-a", "safe"), ("-A", "all")]:
        _add(
            "disk-" + label,
            {"sample.rb": _MIXED_SOURCE},
            ["sample.rb"],
            [mode],
            only=_MIXED,
        )
        _add(
            "stdin-" + label,
            {},
            ["--stdin", "sample.rb"],
            [mode],
            only=_MIXED,
            source=_MIXED_SOURCE,
        )
        _add(
            "color-" + label,
            {"sample.rb": _MIXED_SOURCE},
            ["sample.rb"],
            [mode, "--color"],
            only=_MIXED,
        )
    _add("clean-correction", {"sample.rb": _CLEAN}, ["sample.rb"], ["-a"])
    _add("stdin-inspection", {}, ["--stdin", "sample.rb"], source=_SOURCE)
    _add(
        "stdout-to-file",
        {"sample.rb": _SOURCE},
        ["sample.rb"],
        ["--out", "console.txt"],
        outputs=["console.txt"],
    )
    _add("stderr", {"sample.rb": _SOURCE}, ["sample.rb"], ["--stderr"])
    _add(
        "separate-outputs",
        {"sample.rb": _SOURCE},
        ["sample.rb"],
        ["--out", "console.txt", "--format", "simple", "--out", "simple.txt"],
        outputs=["console.txt", "simple.txt"],
    )
    _add(
        "separate-outputs-reversed",
        {"sample.rb": _SOURCE},
        ["sample.rb"],
        ["--out", "simple.txt", "--format", "autogenconf", "--out", "console.txt"],
        formatter="simple",
        outputs=["simple.txt", "console.txt"],
    )
    _add(
        "progress-control", {"sample.rb": _SOURCE}, ["sample.rb"], formatter="progress"
    )
    _add("simple-control", {"sample.rb": _SOURCE}, ["sample.rb"], formatter="simple")
    return cases


def _todo_cases():
    cases = []
    for name, files, flags in [
        ("ordinary-path", {"sample.rb": _SOURCE}, []),
        ("exclude-limit", {"a.rb": _SOURCE, "b.rb": _SOURCE}, ["--exclude-limit", "1"]),
    ]:
        yaml = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
        cases.append(
            {
                "name": name,
                "files": {".rubocop.yml": yaml, **files},
                "args": [
                    "--config",
                    ".rubocop.yml",
                    "--cache",
                    "false",
                    "--only",
                    _ONLY,
                    "--auto-gen-config",
                    "--auto-gen-only-exclude",
                    "--no-auto-gen-timestamp",
                    "--no-auto-gen-enforced-style",
                    *flags,
                    *files,
                ],
            }
        )
    return cases


def _run(command, case):
    with tempfile.TemporaryDirectory(prefix="sonicop-autogenconf-") as temporary:
        cwd = Path(temporary)
        for name in case.get("directories", []):
            (cwd / name).mkdir(parents=True)
        for name, source in case["files"].items():
            path = cwd / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(source.encode())
        if "mtime" in case:
            # mtime が同じなら、fail-fast の探索順をファイル作成速度へ依存させない。
            for name in case["files"]:
                os.utime(cwd / name, (case["mtime"], case["mtime"]))
        environment = os.environ.copy()
        # 仕様版への起動設定の注入を遮断し、gem・DLL 探索に要る親環境は保持する。
        for name in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "RUBOCOP_OPTS",
            "NO_COLOR",
        ]:
            environment.pop(name, None)
        source = case.get("input_hex")
        result = subprocess.run(
            command + case["args"],
            cwd=cwd,
            env=environment,
            input=None if source is None else bytes.fromhex(source),
            capture_output=True,
            check=False,
        )
        if result.returncode not in (0, 1):
            raise RuntimeError(f"{case['name']}: {result.returncode} {result.stderr!r}")
        if result.stderr and "--stderr" not in case["args"]:
            raise RuntimeError(f"{case['name']}: unexpected stderr {result.stderr!r}")
        stdout = result.stdout
        stderr = result.stderr
        captured = {
            "exit": result.returncode,
            "raw_stdout_hex": stdout.hex(),
            "raw_stderr_hex": stderr.hex(),
        }
        # 元 byte は全て残し、report の transport と補正本文を別々に比較する。
        if b"=" * 20 in stdout:
            stdout, _, tail = stdout.partition(b"=" * 20)
            if tail.startswith(b"\r\n"):
                tail = tail[2:]
            elif tail.startswith(b"\n"):
                tail = tail[1:]
            else:
                raise RuntimeError("missing stdin source separator newline")
            captured["corrected_stdin_hex"] = tail.hex()
        captured["stdout"] = stdout.decode().replace("\r\n", "\n")
        captured["stderr"] = stderr.decode().replace("\r\n", "\n")
        captured["files_after"] = {
            name: (cwd / name).read_bytes().hex() for name in case["files"]
        }
        captured["outputs_after"] = {
            name: (cwd / name).read_bytes().hex() for name in case.get("outputs", [])
        }
        captured["generated_files"] = {
            name: (cwd / name).read_bytes().hex()
            for name in [".rubocop_todo.yml", ".rubocop.yml"]
            if name not in case["files"] and (cwd / name).exists()
        }
        return {**case, **captured}


def _write_json(stream, value):
    """予約した同じbinary streamへLF容器だけを出し、再openで他者を上書きしない。"""
    stream.write(
        (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    )


def main():
    """全出力のleafを検査・排他予約してから参照を起動し、失敗partialを残す。

    Raises:
        ValueError: 複数の出力名が同じleafを指す。
        FileExistsError: 出力leafを既に他者が所有する。

    """
    parser = argparse.ArgumentParser()
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--native-before", type=Path)
    parser.add_argument(
        "--output", type=Path, default=Path("tests/fixtures/autogenconf_console.json")
    )
    arguments = parser.parse_args()
    output = arguments.output.absolute()
    paths = {"fixture": output, "console": output.with_suffix(".console-only.json")}
    if arguments.native_before:
        paths["native"] = output.parent / "native_todo_before.json"
    if len(set(paths.values())) != len(paths):
        raise ValueError("output leaves overlap")
    for path in paths.values():
        if path.exists() or path.is_symlink():
            raise FileExistsError(path)

    # 全leafの検査を先に終え、競合時は先に予約した0Bも消さず所有根拠を残す。
    output.parent.mkdir(parents=True, exist_ok=True)
    with ExitStack() as reserved:
        streams = {
            name: reserved.enter_context(path.open("xb"))
            for name, path in paths.items()
        }
        with tempfile.TemporaryDirectory(
            prefix="sonicop-autogenconf-oracle-"
        ) as temporary:
            wrapper = Path(temporary) / "reference.rb"
            wrapper.write_text(
                'gem "parser", "3.3.12.0"\ngem "prism", "1.8.1"\n'
                'gem "rainbow", "3.1.1"\n'
                'gem "rubocop", "1.89.0"\n'
                'load Gem.bin_path("rubocop", "rubocop", "1.89.0")\n',
                encoding="utf-8",
            )
            command = [arguments.ruby, str(wrapper)]
            with ThreadPoolExecutor(max_workers=2) as pool:
                cases = list(pool.map(lambda case: _run(command, case), _cases()))
                _write_json(streams["console"], cases)
                todo = list(pool.map(lambda case: _run(command, case), _todo_cases()))
            if arguments.native_before:
                baseline = [
                    _run([str(arguments.native_before.resolve())], case)
                    for case in _todo_cases()
                ]
                _write_json(streams["native"], baseline)
        fixture = {
            "reference": "RuboCop 1.89.0 / parser 3.3.12.0 / Prism 1.8.1 actual CLI",
            "normalization": (
                "report transport CRLF -> LF only; "
                "source and generated file raw bytes retained"
            ),
            "cases": cases,
            "todo_controls": todo,
        }
        _write_json(streams["fixture"], fixture)
    # close/flushまで成功してから、未完了を成功と区別する完了行を出す。
    print(f"recorded {len(cases)} console cases and {len(todo)} todo controls")


if __name__ == "__main__":
    main()
