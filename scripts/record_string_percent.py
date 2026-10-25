"""文字列後の剰余を、EndOfLine を除く既定 cop と本家の独立した CLI で記録する。"""

import argparse
import json
import os
import pathlib
import subprocess
import tempfile

parser = argparse.ArgumentParser(description="RuboCop 1.89.0 の文字列後の剰余を記録する")
parser.add_argument("--ruby", default="ruby")
parser.add_argument(
    "--output",
    type=pathlib.Path,
    default=pathlib.Path(__file__).resolve().parents[1]
    / "tests/fixtures/string_percent_reports.json",
)
args = parser.parse_args()
sources = {
    "array": 'puts "%3d %s"%[l+1, line]\n',
    "spaced": 'puts "%3d %s" %[l+1, line]\n',
    "quoted_chain": 'puts %q(a) "b"\n',
    "percent_receiver": 'puts %q(%s)%[value+1]\n',
    "typed": 'puts "a"%q[value+1]\n',
    "interpolation": 'puts "#{1}"%[value+1]\n',
}
loader = (
    "gem 'parser', '3.3.12.0'; gem 'rubocop', '1.89.0'; "
    "load Gem.bin_path('rubocop', 'rubocop', '1.89.0')"
)
groups = []
metadata = None
# 明示した対象版を開発者のシェルに残る環境変数で上書きさせない。
environment = dict(os.environ)
environment.pop("RUBOCOP_TARGET_RUBY_VERSION", None)
for target in ["2.7", "3.4"]:
    config = (
        f"AllCops:\n  TargetRubyVersion: {target}\n  NewCops: disable\n"
        "Layout/EndOfLine:\n  Enabled: false\n"
    )
    for ending in ["\n", "\r\n"]:
        for mode in [None, "-a", "-A"]:
            with tempfile.TemporaryDirectory(prefix="sonicop-string-percent-") as temporary:
                directory = pathlib.Path(temporary)
                (directory / ".rubocop.yml").write_bytes(config.encode())
                cases = []
                for index, (name, source) in enumerate(sources.items()):
                    filename = f"input_{index:03}.rb"
                    source = source.replace("\n", ending)
                    (directory / filename).write_bytes(source.encode())
                    cases.append({"name": name, "filename": filename, "source": source})
                command = [
                    args.ruby, "-e", loader, "--", "--config", ".rubocop.yml",
                    "--cache", "false", "--format", "json",
                ]
                if mode:
                    command.append(mode)
                run = subprocess.run(command + ["."], cwd=directory, env=environment, capture_output=True)
                if run.returncode not in [0, 1]:
                    raise RuntimeError(run.stderr.decode(errors="replace"))
                if run.stderr:
                    raise RuntimeError(run.stderr.decode(errors="replace"))
                report = json.loads(run.stdout)
                metadata = report.pop("metadata")
                assert metadata["rubocop_version"] == "1.89.0", metadata
                assert report["summary"]["target_file_count"] == len(cases), report
                assert report["summary"]["inspected_file_count"] == len(cases), report
                assert [file["path"] for file in report["files"]] == [
                    case["filename"] for case in cases
                ], report
                for case in cases:
                    case["after"] = (directory / case["filename"]).read_bytes().decode()
                groups.append({
                    "target": target, "yaml": config, "ending": ending, "mode": mode,
                    "exit": run.returncode, "report": report, "cases": cases,
                })
                print(target, repr(ending), mode, report["summary"]["offense_count"], flush=True)
fixture = {
    "provenance": (
        "RuboCop 1.89.0 CLI / Parser 3.3.12.0; recorded 2026-10-25. "
        "Parser regressions with the platform Layout/EndOfLine cop disabled on both tools; "
        "all other default-enabled cops, independent CLI per target/ending/mode. "
        "Reports and on-disk reference bytes are preserved. Candidate writeback keeps "
        "the original LF/CRLF format under the existing intentional divergence."
    ),
    "excluded_cops": ["Layout/EndOfLine"],
    "reference_metadata": metadata,
    "groups": groups,
}
args.output.write_text(json.dumps(fixture, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
