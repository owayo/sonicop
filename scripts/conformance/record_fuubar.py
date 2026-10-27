"""固定版 RuboCop の非 TTY Fuubar を、独立入力の実 CLI から記録する。"""

import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import tempfile

BASE_CONFIG = (
    "AllCops:\n  NewCops: disable\n  UseProjectIndex: false\n"
    "  TargetRubyVersion: 3.3\n  ParserEngine: parser_prism\n"
)
COP = "Layout/SpaceAroundOperators"
RUNTIME = {
    "ruby": "4.0.7",
    "rubocop": "1.89.0",
    "parser": "3.3.12.0",
    "prism": "1.8.1",
    "rubocop_ast": "1.50.0",
    "ruby_progressbar": "1.13.0",
}
LAUNCHER = """gem 'parser', '3.3.12.0'
gem 'prism', '1.8.1'
gem 'rubocop-ast', '1.50.0'
gem 'rainbow', '3.1.1'
gem 'rubocop', '1.89.0'
gem 'ruby-progressbar', '1.13.0'
require 'rubocop'
require 'ruby-progressbar'
if ARGV == ['--record-runtime']
  require 'json'
  puts JSON.generate({ruby: RUBY_VERSION, rubocop: RuboCop::Version::STRING,
    parser: Gem.loaded_specs.fetch('parser').version.to_s,
    prism: Gem.loaded_specs.fetch('prism').version.to_s,
    rubocop_ast: Gem.loaded_specs.fetch('rubocop-ast').version.to_s,
    ruby_progressbar: Gem.loaded_specs.fetch('ruby-progressbar').version.to_s})
else
  exit RuboCop::CLI.new.run(ARGV)
end
"""


def scenario(
    name,
    *,
    files=None,
    stdin=None,
    flags=(),
    all_cops="",
    cops="",
    only=COP,
    width="80",
    formats=None,
    comparison="strict",
    mtime_ns=None,
):
    """実入力と引数だけを構築し、期待出力は実行結果から取得する。

    Returns:
        dict: ソース・設定・引数・比較範囲を持つ独立入力。

    """
    if files is None:
        files = {} if stdin is not None else {"case.rb": b"x=1\n"}
    result = {
        "name": name,
        "files": {path: data.hex() for path, data in files.items()},
        "stdin_hex": stdin.hex() if stdin is not None else None,
        "targets": ["sample.rb"] if stdin is not None else list(files) or ["."],
        "flags": list(flags),
        "config": BASE_CONFIG + all_cops + cops,
        "only": only,
        "env": {
            "RUBY_PROGRESS_BAR_LENGTH": width,
            "RUBY_PROGRESS_BAR_FORMAT": "ignored in non-TTY",
        },
        "formats": formats if formats is not None else [["fuubar", None]],
        "comparison": comparison,
    }
    if mtime_ns is not None:
        result["mtime_ns"] = mtime_ns
    return result


SCENARIOS = [
    scenario("disk-offense"),
    scenario("disk-crlf", files={"case.rb": b"x=1\r\n"}),
    scenario("disk-eof", files={"case.rb": b"x=1"}),
    scenario("disk-clean", files={"case.rb": b"x = 1\n"}),
    scenario("stdin-lf", stdin=b"x=1\n"),
    scenario("stdin-crlf", stdin=b"x=1\r\n"),
    scenario("explicit-no-color", flags=["--no-color"]),
    scenario("no-cop-name", flags=["--no-display-cop-names"]),
    scenario("alias", formats=[["fu", None]]),
    scenario("default-formatter", formats=[], all_cops="  DefaultFormatter: fuubar\n"),
    scenario("color", flags=["--color"]),
    scenario("color-clean", files={"case.rb": b"x = 1\n"}, flags=["--color"]),
    scenario("color-warning", flags=["--color"], cops=f"{COP}:\n  Severity: warning\n"),
    scenario("color-info", flags=["--color"], cops=f"{COP}:\n  Severity: info\n"),
    scenario("safe-correction", flags=["-a"]),
    scenario("all-correction", flags=["-A"]),
    scenario("color-correction", flags=["--color", "-a"]),
    scenario("safe-remaining", only="Style/FrozenStringLiteralComment", flags=["-a"]),
    scenario(
        "partial-correction-color",
        files={"case.rb": b"# abcdefghijklmnop\nx=1\n"},
        only=f"{COP},Layout/LineLength",
        flags=["--color", "-a"],
        cops=f"{COP}:\n  Severity: error\nLayout/LineLength:\n  Max: 10\n  Severity: warning\n",
    ),
    scenario(
        "annotated-color",
        flags=["--color", "-D", "-E", "-S"],
        cops=f"{COP}:\n  Details: 'Review `size`.'\n  StyleGuide: '#operator'\n  References: ['https://example.test/ref']\n",
    ),
    scenario("width20", width="20"),
    scenario("width12", width="12"),
    scenario("width-small", width="4"),
    scenario("width-ruby-prefix", width="  +20tail"),
    scenario("width-invalid", width="unknown"),
    scenario("stderr", flags=["--stderr"]),
    scenario("empty", files={}),
    scenario("two-offenses", files={"a.rb": b"x=1\n", "b.rb": b"y=2\n"}, comparison="events"),
    scenario(
        "three-mixed",
        files={"a.rb": b"x=1\n", "b.rb": b"y = 2\n", "c.rb": b"z=3\n"},
        comparison="events",
    ),
    scenario(
        "fail-fast",
        files={"a.rb": b"x=1\n", "b.rb": b"y=2\n"},
        flags=["--fail-fast"],
        comparison="events",
    ),
    scenario(
        "separate-fuubar-first",
        flags=["--no-display-cop-names"],
        formats=[["fuubar", "fuubar.txt"], ["simple", "simple.txt"]],
    ),
    scenario(
        "separate-fuubar-last",
        flags=["--no-display-cop-names"],
        formats=[["simple", "simple.txt"], ["fuubar", "fuubar.txt"]],
    ),
    scenario(
        "fail-fast-equal-mtime",
        files={"a.rb": b"x=1\n", "b.rb": b"y=2\n"},
        flags=["--fail-fast"],
        comparison="events",
        mtime_ns=1700000000000000000,
    ),
]


def arguments(case, diagnostic=False):
    """同じ入力と検査条件で、描画用と独立した診断証拠用の引数を作る。

    Returns:
        list: 作業ディレクトリに相対な参照 CLI 引数。

    """
    args = ["--config", "config.yml", "--cache", "false", "--only", case["only"]]
    formats = [["json", "report.json"]] if diagnostic else case["formats"]
    for fmt, path in formats:
        args.extend(["--format", fmt])
        if path is not None:
            args.extend(["--out", path])
    args.extend(flag for flag in case["flags"] if not diagnostic or flag != "--stderr")
    if case["stdin_hex"] is not None:
        args.extend(["--stdin", case["targets"][0]])
    else:
        args.extend(case["targets"])
    return args


def prepare(project, case):
    """補正実行同士が原本を共有しないよう、入力を別ディレクトリへ置く。"""
    project.mkdir()
    (project / "config.yml").write_text(case["config"], encoding="utf-8", newline="\n")
    for name, data in case["files"].items():
        path = project / name
        path.write_bytes(bytes.fromhex(data))
        if "mtime_ns" in case:
            os.utime(path, ns=(case["mtime_ns"], case["mtime_ns"]))


def run(command, project, env, stdin):
    """参照 CLI の中断を診断差や空の成功へ読み替えない。

    Returns:
        subprocess.CompletedProcess: 終了 0/1 の参照 CLI の実出力。

    Raises:
        ValueError: CLI が通常の診断終了に到達しない。

    """
    result = subprocess.run(
        command, cwd=project, env=env, input=stdin, capture_output=True, check=False, timeout=120
    )
    if result.returncode not in (0, 1):
        raise ValueError(f"reference failed: exit={result.returncode}, stderr={result.stderr!r}")
    return result


def validate_report(report, case):
    """実際の対象・全診断・完了件数を検査し、空集合の偽成功を拒む。

    Returns:
        dict: metadata を除く本家の診断全項目と完了件数。

    Raises:
        ValueError: 対象・診断項目・cop・完了数が期待入力と整合しない。

    """
    files = report.get("files", [])
    paths = [file.get("path") for file in files]
    expected = [] if not case["files"] and case["stdin_hex"] is None else case["targets"]
    if "--fail-fast" in case["flags"]:
        expected = expected[:1]
    if paths != expected or len(paths) != len(set(paths)):
        raise ValueError(f"unexpected inspected paths: {paths!r}, expected {expected!r}")
    offenses = [offense for file in files for offense in file.get("offenses", [])]
    for offense in offenses:
        required = {"severity", "message", "cop_name", "corrected", "correctable", "location"}
        locations = {
            "start_line",
            "start_column",
            "last_line",
            "last_column",
            "length",
            "line",
            "column",
        }
        if not required <= offense.keys() or not locations <= offense["location"].keys():
            raise ValueError("incomplete offense fields")
        if offense["cop_name"] not in case["only"].split(","):
            raise ValueError("unexpected cop or syntax diagnostic")
    summary = report.get("summary")
    expected_total = 0 if not expected and not case["files"] else len(case["targets"])
    if summary != {
        "offense_count": len(offenses),
        "target_file_count": expected_total,
        "inspected_file_count": len(expected),
    }:
        raise ValueError("incomplete or inconsistent completion summary")
    return {key: report[key] for key in ("files", "summary")}


def validate_render(text, report):
    """進捗 bar や summary だけの偽出力を、正の診断の実測として受け取らない。

    Raises:
        ValueError: 先頭 bar、完了件数、診断本文が欠落している。

    """
    text = re.sub(r"\x1b\[[0-9;]*m", "", text)
    summary = report["summary"]
    inspected = summary["inspected_file_count"]
    offenses = summary["offense_count"]
    file_word = "file" if inspected == 1 else "files"
    offense_phrase = (
        "no offenses" if offenses == 0 else f"{offenses} offense" + ("s" if offenses != 1 else "")
    )
    if (
        not text.startswith("\nProgress: |")
        or f"{inspected} {file_word} inspected, {offense_phrase} detected" not in text
    ):
        raise ValueError("missing Fuubar rendering or completion")
    for file in report["files"]:
        for offense in file["offenses"]:
            location = offense["location"]
            prefix = (
                f"{file['path']}:{location['line']}:{location['column']}: "
                f"{offense['severity'][0].upper()}:"
            )
            if prefix not in text:
                raise ValueError("missing displayed offense")


def collect(ruby, root, selected=None):
    """候補を呼ばず、固定版の実出力と独立診断証拠を収集する。

    Returns:
        dict: 入力・固定版・raw 出力・独立診断証拠を持つ参照記録。

    Raises:
        ValueError: 仕様版、診断証拠、描画、原本保持の条件が成立しない。

    """
    launcher = root / "reference.rb"
    launcher.write_text(LAUNCHER, encoding="utf-8", newline="\n")
    env = os.environ.copy()
    for key in (
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "RUBOCOP_OPTS",
        "RUBY_PROGRESS_BAR_LENGTH",
        "RUBY_PROGRESS_BAR_FORMAT",
    ):
        env.pop(key, None)
    runtime = run([ruby, str(launcher), "--record-runtime"], root, env, None)
    if runtime.returncode != 0 or runtime.stderr or json.loads(runtime.stdout) != RUNTIME:
        raise ValueError("reference runtime mismatch")
    records = []
    for case in SCENARIOS:
        if selected is not None and case["name"] != selected:
            continue
        project = root / case["name"]
        diagnostic = root / (case["name"] + "-diagnostic")
        prepare(project, case)
        prepare(diagnostic, case)
        case_env = env | case["env"]
        stdin = bytes.fromhex(case["stdin_hex"]) if case["stdin_hex"] is not None else None
        result = run([ruby, str(launcher), *arguments(case)], project, case_env, stdin)
        evidence = run([ruby, str(launcher), *arguments(case, True)], diagnostic, case_env, stdin)
        if evidence.stderr or evidence.returncode != result.returncode:
            raise ValueError("independent diagnostic run failed or changed exit status")
        report = validate_report(json.loads((diagnostic / "report.json").read_bytes()), case)
        outputs = {"stdout": result.stdout.hex(), "stderr": result.stderr.hex()}
        for _, path in case["formats"]:
            if path is not None:
                outputs[path] = (project / path).read_bytes().hex()
        primary = "stderr" if "--stderr" in case["flags"] else "stdout"
        if any(fmt in ("fuubar", "fu") and path for fmt, path in case["formats"]):
            primary = next(path for fmt, path in case["formats"] if fmt in ("fuubar", "fu"))
        if primary != "stderr" and result.stderr:
            raise ValueError(f"unexpected stderr: {result.stderr!r}")
        text = bytes.fromhex(outputs[primary]).decode("utf-8").replace("\r\n", "\n")
        validate_render(text, report)
        after = {path: (project / path).read_bytes().hex() for path in case["files"]}
        if not any(flag in ("-a", "-A") for flag in case["flags"]) and after != case["files"]:
            raise ValueError("inspection changed original input bytes")
        records.append(
            {
                "case": case,
                "exit": result.returncode,
                "primary": primary,
                "outputs_hex": outputs,
                "after_hex": after,
                "report": report,
            }
        )
        print(case["name"], flush=True)
    return {"reference": RUNTIME, "cases": records}


def main(argv=None):
    """公開 CLI から LF の JSON コンテナを作り、raw transport bytes は hex で保持する。

    Returns:
        int: 完全な参照記録を保存した場合の終了 0。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--case", choices=[case["name"] for case in SCENARIOS])
    args = parser.parse_args(argv)
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("既存outputを上書きしない。未使用の公開先を指定する")
    # 元leafを解決せず予約し、競合時はOracleやworkを作る前に拒否する。
    # 採取・保存に失敗した予約fileは0B/partialの証拠として保持し、成功表示しない。
    with output.open("xb") as destination:
        with tempfile.TemporaryDirectory(prefix="sonicop-fuubar-reference-") as scratch:
            document = collect(args.ruby, pathlib.Path(scratch), args.case)
        data = (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
        destination.write(data)
    print(f"cases={len(document['cases'])} sha256={hashlib.sha256(data).hexdigest()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
