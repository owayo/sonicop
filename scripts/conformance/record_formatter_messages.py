"""RuboCop の共通 message 注釈を実 CLI から記録する。候補版は呼ばない。"""

import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile

SOURCE = b"x=1\n"
BASE_CONFIG = (
    "AllCops:\n"
    "  NewCops: disable\n"
    "  UseProjectIndex: false\n"
    "  TargetRubyVersion: 3.3\n"
    "  ParserEngine: parser_prism\n"
)
COP_CONFIG = (
    "Layout/SpaceAroundOperators:\n"
    "  Details: Extra detail.\n"
    '  StyleGuide: "#custom"\n'
    "  References:\n"
    "    - https://example.test/reference\n"
)
CASES = [
    ("json-default", ["json"], [], ""),
    ("json-names", ["json"], ["--display-cop-names"], ""),
    ("json-no-names", ["json"], ["--no-display-cop-names"], ""),
    (
        "json-both-on-last",
        ["json"],
        ["--no-display-cop-names", "--display-cop-names"],
        "",
    ),
    (
        "json-both-off-last",
        ["json"],
        ["--display-cop-names", "--no-display-cop-names"],
        "",
    ),
    ("json-config-on", ["json"], [], "  DisplayCopNames: true\n"),
    ("json-config-off", ["json"], [], "  DisplayCopNames: false\n"),
    (
        "json-flag-over-config-off",
        ["json"],
        ["--display-cop-names"],
        "  DisplayCopNames: false\n",
    ),
    (
        "json-no-flag-over-config-on",
        ["json"],
        ["--no-display-cop-names"],
        "  DisplayCopNames: true\n",
    ),
    ("json-simple", ["json", "simple"], [], ""),
    ("simple-json", ["simple", "json"], [], ""),
    ("json-json", ["json", "json"], [], ""),
    ("simple-json-alias", ["simple", "j"], [], ""),
    ("json-alias", ["j"], [], ""),
    ("json-simple-config-off", ["json", "simple"], [], "  DisplayCopNames: false\n"),
    ("json-simple-no-names", ["json", "simple"], ["--no-display-cop-names"], ""),
    ("simple-json-names", ["simple", "json"], ["--display-cop-names"], ""),
    ("simple-default", ["simple"], [], ""),
    ("simple-no-names", ["simple"], ["--no-display-cop-names"], ""),
    ("config-default-json", [], [], "  DefaultFormatter: json\n"),
    ("json-style", ["json"], ["--display-style-guide"], ""),
    ("json-config-style", ["json"], [], "  DisplayStyleGuide: true\n"),
    ("json-details", ["json"], ["--extra-details"], ""),
    ("json-config-details", ["json"], [], "  ExtraDetails: true\n"),
    (
        "json-all",
        ["json"],
        ["--display-cop-names", "--extra-details", "--display-style-guide"],
        "",
    ),
    ("json-debug-off", ["json"], ["--debug", "--no-display-cop-names"], ""),
]
CACHE_CASES = [
    ("cache-names", ["json"], ["--display-cop-names"], ""),
    ("cache-default", ["json"], [], ""),
    ("cache-simple-json", ["simple", "json"], [], ""),
    ("cache-json-simple", ["json", "simple"], [], ""),
    ("cache-no-names", ["json"], ["--no-display-cop-names"], ""),
    ("cache-alias", ["j"], [], ""),
    ("cache-all", ["json"], ["-D", "-E", "-S"], ""),
    ("cache-all-repeated", ["json"], ["-D", "-E", "-S"], ""),
    ("cache-default-again", ["json"], [], ""),
]
CACHE_CONTROLS = [
    ("control-names", ["json"], ["--display-cop-names"], ""),
    ("control-no-names", ["json"], ["--no-display-cop-names"], ""),
    ("control-json-simple-names", ["json", "simple"], ["--display-cop-names"], ""),
    ("control-simple-json-no-names", ["simple", "json"], ["--no-display-cop-names"], ""),
    ("control-alias-names", ["j"], ["--display-cop-names"], ""),
    ("control-all", ["json"], ["-D", "-E", "-S"], ""),
    ("control-simple-json-all", ["simple", "json"], ["-D", "-E", "-S"], ""),
    ("control-json-simple-no-names", ["json", "simple"], ["--no-display-cop-names"], ""),
    ("control-names-again", ["json"], ["--display-cop-names"], ""),
]
LAUNCHER = """gem 'parser', '3.3.12.0'
gem 'prism', '1.8.1'
gem 'rubocop-ast', '1.50.0'
gem 'rubocop', '1.89.0'
require 'rubocop'
if ARGV == ['--record-runtime']
  require 'json'
  puts JSON.generate({ruby: RUBY_VERSION, rubocop: RuboCop::Version::STRING,
    parser: Gem.loaded_specs.fetch('parser').version.to_s,
    prism: Gem.loaded_specs.fetch('prism').version.to_s,
    rubocop_ast: Gem.loaded_specs.fetch('rubocop-ast').version.to_s})
else
  exit RuboCop::CLI.new.run(ARGV)
end
"""
EXPECTED_RUNTIME = {
    "ruby": "4.0.7",
    "rubocop": "1.89.0",
    "parser": "3.3.12.0",
    "prism": "1.8.1",
    "rubocop_ast": "1.50.0",
}


def reference_environment():
    """Gem・DLL探索を保ち、本家の入力を変える4キーだけを子のcopyから除く。

    Returns:
        dict: 親環境を変更せずに作った参照プロセス専用の環境。

    """
    environment = os.environ.copy()
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(key, None)
    return environment


def validate_report(report):
    """全項目の比較に必要な対象・発火・完了の証拠を要求する。

    Returns:
        dict: 実行環境の metadata を除く本家の files と summary。

    Raises:
        ValueError: 対象・発火・全項目・完了件数の証拠が欠けている。

    """
    # 空集合や別ファイルの成功は、この 1 file / 1 offense の測定にならない。
    files = report.get("files", [])
    if len(files) != 1 or files[0].get("path") != "case.rb":
        raise ValueError("unexpected or incomplete inspected paths")
    offenses = files[0].get("offenses", [])
    if len(offenses) != 1 or offenses[0].get("cop_name") != "Layout/SpaceAroundOperators":
        raise ValueError("unexpected or missing offense")
    required = {"severity", "message", "cop_name", "corrected", "correctable", "location"}
    location = {
        "start_line",
        "start_column",
        "last_line",
        "last_column",
        "length",
        "line",
        "column",
    }
    if not required <= offenses[0].keys() or not location <= offenses[0]["location"].keys():
        raise ValueError("incomplete offense fields")
    if report.get("summary") != {
        "offense_count": 1,
        "target_file_count": 1,
        "inspected_file_count": 1,
    }:
        raise ValueError("inconsistent completion summary")
    return {key: report[key] for key in ("files", "summary")}


def execute(command, directory, env):
    """途中失敗を診断差へ混ぜず、参照 CLI の正常な終了だけを受け取る。

    Returns:
        subprocess.CompletedProcess: 正常に完了した参照プロセスの実出力。

    Raises:
        ValueError: 終了 0/1 以外、または stderr がある。

    """
    result = subprocess.run(
        command, cwd=directory, env=env, capture_output=True, timeout=120, check=False
    )
    if result.returncode not in (0, 1) or result.stderr:
        raise ValueError(f"reference failed: exit={result.returncode}, stderr={result.stderr!r}")
    return result


def collect(ruby, directory, cache_controls=False):
    """固定版を検証し、独立入力と共有 cache の両方から記録する。

    Returns:
        dict: 固定版・入力・各実 CLI の設定、引数、全出力。

    Raises:
        ValueError: 参照版、共有入力、完了、原本保持の条件が成立しない。

    """
    launcher = directory / "reference.rb"
    launcher.write_text(LAUNCHER, encoding="utf-8", newline="\n")
    env = reference_environment()
    runtime = execute([ruby, str(launcher), "--record-runtime"], directory, env)
    if runtime.returncode != 0 or json.loads(runtime.stdout) != EXPECTED_RUNTIME:
        raise ValueError(f"reference version mismatch: {runtime.stdout!r}")
    records = []
    groups = ((True, CACHE_CONTROLS),) if cache_controls else ((False, CASES), (True, CACHE_CASES))
    for cached, cases in groups:
        for name, formats, flags, all_config in cases:
            project = directory / ("shared-cache" if cached else name)
            project.mkdir(exist_ok=True)
            config = BASE_CONFIG + all_config + COP_CONFIG
            config_path = project / "config.yml"
            source_path = project / "case.rb"
            if source_path.exists():
                # 本文だけでなく stat も保ち、候補の cache hit を実際に検査できる入力にする。
                if (
                    source_path.read_bytes() != SOURCE
                    or config_path.read_bytes() != config.encode()
                ):
                    raise ValueError("shared-cache input changed")
            else:
                config_path.write_text(config, encoding="utf-8", newline="\n")
                source_path.write_bytes(SOURCE)
            args = [
                "--config",
                "config.yml",
                "--cache",
                str(cached).lower(),
                "--only",
                "Layout/SpaceAroundOperators",
                "--no-color",
            ]
            if cached:
                args.extend(["--cache-root", "cache"])
            for index, fmt in enumerate(formats):
                args.extend(["--format", fmt, "--out", f"output{index}.txt"])
            if not formats:
                args.extend(["--out", "output0.txt"])
            args.extend([*flags, "case.rb"])
            # 共有ディレクトリの以前の出力で、出力欠落を隠さない。
            for output in project.glob("output*.txt"):
                output.unlink()
            result = execute([ruby, str(launcher), *args], project, env)
            if result.returncode != 1 or (project / "case.rb").read_bytes() != SOURCE:
                raise ValueError("reference did not inspect the unchanged positive input")
            outputs = []
            for index, fmt in enumerate(formats or ["json"]):
                data = (project / f"output{index}.txt").read_bytes()
                if fmt in ("json", "j"):
                    output = {"format": "json", "report": validate_report(json.loads(data))}
                else:
                    text = data.decode("utf-8").replace("\r\n", "\n")
                    if "1 file inspected, 1 offense detected" not in text:
                        raise ValueError("incomplete simple output")
                    output = {"format": fmt, "text": text}
                outputs.append(output)
            records.append(
                {
                    "name": name,
                    "cached": cached,
                    "config": config,
                    "args": args,
                    "exit": result.returncode,
                    "outputs": outputs,
                }
            )
            print(name, flush=True)
    return {
        "reference": EXPECTED_RUNTIME,
        "source_hex": SOURCE.hex(),
        "cases": records,
    }


def main(argv=None):
    """公開 CLI から同じ LF コンテナを再生成する。

    Returns:
        int: 正常な記録を保存した場合の終了 0。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby", help="固定版 Ruby 4.0.7 の実行ファイル")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument(
        "--cache-controls",
        action="store_true",
        help="元の 35 件を置き換えず追加の明示 on/off 9 件を採る",
    )
    args = parser.parse_args(argv)
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("既存outputを上書きしない。新しい公開先を指定する")
    # 元leafを解決せず排他予約し、競合時は子やscratchを作る前に拒む。
    # 採取・書込みが失敗した場合は予約fileを証拠として残し、成功表示しない。
    with output.open("xb") as destination:
        with tempfile.TemporaryDirectory(prefix="sonicop-formatter-messages-") as scratch:
            document = collect(args.ruby, pathlib.Path(scratch), args.cache_controls)
        data = (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
        destination.write(data)
    print(f"cases={len(document['cases'])} sha256={hashlib.sha256(data).hexdigest()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
