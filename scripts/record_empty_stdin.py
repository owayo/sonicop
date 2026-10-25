#!/usr/bin/env python3
"""空 stdin 名の診断・formatter・補正 byte を RuboCop 1.89.0 の CLI から記録する。"""

import argparse
import json
import os
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# 通常 filename でも差のある fuubar/autogenconf と、本家が IO#winsize で失敗する
# pacman は別の監査記録へ残す。この fixture の互換比較へは数えない。
FORMATS = [
    "progress",
    "simple",
    "clang",
    "emacs",
    "json",
    "junit",
    "html",
    "markdown",
    "github",
    "tap",
    "files",
    "offenses",
    "worst",
    "quiet",
]
ROOT = (
    "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 3.4\n"
    "Layout/SpaceAroundOperators:\n  EnforcedStyleForExponentOperator: space\n"
)
NESTED = (
    "inherit_from: ../.rubocop.yml\n"
    "Layout/SpaceAroundOperators:\n  EnforcedStyleForExponentOperator: no_space\n"
)


def child_environment():
    """親とGem探索を保ち、測定を変える4変数だけをRuby子のcopyから除く。

    Returns:
        dict: 親を変更せず作ったRuby子用の環境。

    """
    environment = os.environ.copy()
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(key, None)
    return environment


def common(formatter="json", configured=False, only="Layout/SpaceAroundOperators"):
    """新規 cop の警告を診断本文へ混ぜない option 条件を固定する。

    Returns:
        list: 仕様版 CLI へ渡す引数。

    """
    args = [] if configured else ["--force-default-config", "--enable-pending-cops"]
    args += ["--cache", "false", "--format", formatter]
    if only:
        args += ["--only", only]
    return args


def specifications():
    """引数の空文字・改行・設定・補正を、実入力のまま記録する。

    Returns:
        list: 入力 byte と CLI 引数を持つケース。

    """
    cases = []

    def add(
        name,
        source,
        tail,
        formatter="json",
        files=None,
        configured=False,
        only="Layout/SpaceAroundOperators",
    ):
        """通常名と空名へ同じ option 条件を与える。"""
        cases.append(
            {
                "name": name,
                "input_hex": source.hex(),
                "args": common(formatter, configured, only) + tail,
                "format": formatter,
                "files": files or {},
            }
        )

    for name, source, tail in [
        ("empty-long-crlf", b"x=1\r\n", ["--stdin", ""]),
        ("empty-equals-crlf", b"x=1\r\n", ["--stdin="]),
        ("empty-short-crlf", b"x=1\r\n", ["-s", ""]),
        ("empty-lf", b"x=1\n", ["--stdin", ""]),
        ("empty-eof", b"", ["--stdin", ""]),
        ("empty-without-final-newline", b"x=1", ["--stdin", ""]),
        ("empty-unicode", "あ=1\r\n".encode(), ["--stdin", ""]),
        ("named", b"x=1\r\n", ["--stdin", "example.rb"]),
        ("named-unicode", b"x=1\r\n", ["--stdin", "あ💎.rb"]),
    ]:
        add(name, source, tail)
    add("default-cops-named", b"puts 1+2\r\n", ["--stdin", "example.rb"], only=None)
    files = {".rubocop.yml": ROOT, "nested/.rubocop.yml": NESTED}
    add("cwd-config", b"puts 2**3\r\n", ["--stdin", ""], files=files, configured=True)
    add(
        "nested-config",
        b"puts 2**3\r\n",
        ["--stdin", "nested/example.rb"],
        files=files,
        configured=True,
    )
    add(
        "explicit-config",
        b"puts 2**3\r\n",
        ["--config", "nested/.rubocop.yml", "--stdin", ""],
        files=files,
        configured=True,
    )
    add(
        "named-root-config",
        b"puts 2**3\r\n",
        ["--stdin", "example.rb"],
        files=files,
        configured=True,
    )
    for target in ["2.7", "3.4"]:
        yaml = f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {target}\n"
        add(
            "discovered-target-" + target,
            b"def foo = 1\r\n",
            ["--stdin", ""],
            files={".rubocop.yml": yaml},
            configured=True,
            only="Lint/Syntax",
        )
        add(
            "explicit-target-" + target,
            b"def foo = 1\r\n",
            ["--config", ".rubocop.yml", "--stdin", ""],
            files={".rubocop.yml": yaml},
            configured=True,
            only="Lint/Syntax",
        )
    for name, source, mode in [
        ("correct-safe", b"x=1\r\n", "-a"),
        ("correct-all", b"x=1\r\n", "-A"),
        ("correct-unicode", "あ=1\r\n".encode(), "-a"),
        ("correct-eof", b"x=1", "-a"),
        ("correct-empty", b"", "-A"),
        ("correct-clean", b"x = 1\r\n", "-a"),
    ]:
        # 補正なしの CRLF raw print は #95 の別監査へ残す。
        # ここは JSON の診断を照合する。
        formatter = "json" if name == "correct-clean" else "simple"
        add(name, source, [mode, "--stdin", ""], formatter=formatter)
    add(
        "named-correction",
        b"x=1\r\n",
        ["-a", "--stdin", "example.rb"],
        formatter="simple",
        files={"example.rb": "original_disk = :kept\r\n"},
    )
    for flag in ["--stdin", "-s"]:
        add("missing-" + flag.lstrip("-"), b"x=1\r\n", [flag])
        cases[-1]["missing_value"] = True
    return cases


def canonical(text, cwd):
    """診断表示の改行と一時 cwd だけを置換し、補正 byte は変えない。

    Returns:
        str: 一時ディレクトリに依存しない診断表示。

    """
    cwd = cwd.as_posix()
    return (
        text.replace("\r\n", "\n")
        .replace(cwd.replace("/", "."), "<CWD_CLASS>")
        .replace(cwd, "<CWD>")
    )


def run(ruby, wrapper, case, environment=None):
    """本家の内部例外やディスク原本の変更を成功として記録しない。

    Returns:
        dict: 入力条件と本家の実結果。

    Raises:
        RuntimeError: 本家が失敗するか、ディスク原本が変わった場合。

    """
    environment = child_environment() if environment is None else environment
    with tempfile.TemporaryDirectory(prefix="sonicop-empty-stdin-") as temporary:
        cwd = Path(temporary)
        for name, text in case["files"].items():
            path = cwd / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode())

        def invoke(source, invocation_args):
            """予定外の終了を録取失敗として止める。

            Returns:
                dict: 本家が返した診断と補正 byte。

            Raises:
                RuntimeError: 本家が予定外の終了や不正な区切りを返した場合。

            """
            result = subprocess.run(
                [ruby, str(wrapper), *invocation_args],
                check=False,
                cwd=cwd,
                env=environment,
                input=source,
                capture_output=True,
            )
            if case.get("missing_value"):
                if result.returncode != 2 or result.stdout or not result.stderr:
                    raise RuntimeError(
                        f"{case['name']}: unexpected missing-value result"
                    )
                return {"exit": 2, "stderr": canonical(result.stderr.decode(), cwd)}
            if result.returncode not in (0, 1) or result.stderr:
                raise RuntimeError(
                    f"{case['name']}: {result.returncode} {result.stderr!r}"
                )
            recorded = {"exit": result.returncode}
            formatter = invocation_args[invocation_args.index("--format") + 1]
            if formatter == "json":
                report = json.loads(result.stdout)
                report.pop("metadata")
                recorded["report"] = report
            else:
                head, separator, tail = result.stdout.partition(b"=" * 20)
                recorded["text"] = canonical(head.decode(), cwd)
                if separator:
                    if tail.startswith(b"\r\n"):
                        tail = tail[2:]
                    elif tail.startswith(b"\n"):
                        tail = tail[1:]
                    else:
                        raise RuntimeError("missing corrected-source separator newline")
                    recorded["corrected_hex"] = tail.hex()
            return recorded

        recorded = invoke(bytes.fromhex(case["input_hex"]), case["args"])
        if "corrected_hex" in recorded:
            # 再実行は JSON で全診断を確認し、Windows の無補正 raw print と混ぜない。
            recorded["rerun_args"] = list(case["args"])
            recorded["rerun_args"][recorded["rerun_args"].index("--format") + 1] = (
                "json"
            )
            recorded["rerun"] = invoke(
                bytes.fromhex(recorded["corrected_hex"]), recorded["rerun_args"]
            )
        for name, text in case["files"].items():
            if (cwd / name).read_bytes() != text.encode():
                raise RuntimeError(f"stdin changed disk input: {name}")
        return {**case, **recorded}


def main():
    """固定した gem の実 CLI だけを fixture の期待値とする。"""
    parser = argparse.ArgumentParser()
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument(
        "--output", type=Path, default=Path("tests/fixtures/empty_stdin_reports.json")
    )
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sonicop-empty-stdin-oracle-") as temporary:
        wrapper = Path(temporary) / "reference.rb"
        wrapper.write_text(
            'gem "parser", "3.3.12.0"\ngem "prism", "1.8.1"\n'
            'gem "rubocop", "1.89.0"\n'
            'load Gem.bin_path("rubocop", "rubocop", "1.89.0")\n',
            encoding="utf-8",
        )
        cases = specifications()
        formats = [
            {
                "name": formatter + ("-anonymous" if not name else "-named"),
                "format": formatter,
                "args": common(formatter) + ["--stdin", name],
                "input_hex": b"x=1\r\n".hex(),
                "files": {},
            }
            for formatter in FORMATS
            for name in ["", "example.rb"]
        ]
        environment = child_environment()
        with ThreadPoolExecutor(max_workers=2) as pool:
            recorded = list(
                pool.map(
                    lambda case: run(args.ruby, wrapper, case, environment),
                    cases + formats,
                )
            )
    fixture = {
        "reference": "RuboCop 1.89.0 / parser 3.3.12.0 / Prism 1.8.1; actual CLI",
        "text_normalization": (
            "report CRLF -> LF; cwd and JUnit classname placeholders; "
            "corrected bytes stay exact"
        ),
        "separate_formatter_audits": ["fuubar", "autogenconf", "pacman"],
        "cases": recorded[: len(cases)],
        "formats": recorded[len(cases) :],
    }
    args.output.write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"recorded {len(cases)} cases and {len(formats)} formatter runs")


if __name__ == "__main__":
    main()
