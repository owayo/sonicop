"""制御文字を含む本文を別ファイルへ保ち、本家の実 CLI から期待値を記録する。"""

import argparse
import json
import pathlib
import subprocess
import tempfile

parser = argparse.ArgumentParser(
    description="RuboCop 1.89.0 の曖昧な引数の実 CLI 記録を再生成する"
)
parser.add_argument(
    "--output",
    type=pathlib.Path,
    default=pathlib.Path(__file__).resolve().parents[1]
    / "tests/fixtures/ambiguity_whitespace_cli.json",
)
out = parser.parse_args().output
cases = []


def add(name, source):
    """エスケープを再解釈せず、実際に検査する本文を保存する。"""
    cases.append({"name": name, "source": source})


operators = ["+1", "-1", "*args", "**kwargs", "&blk", "/re/"]
separators = [
    ("space", " "),
    ("spaces", "  "),
    ("tabs", "\t \t"),
    ("ff", "\f"),
    ("cr", "\r"),
    ("continued", "\\\n"),
    ("space_continued", " \\\n"),
    ("indent_continued", "\\\n  "),
    ("twice_continued", "\\\n\\\n"),
]
for separator_name, separator in separators:
    for operator in operators:
        add(f"{separator_name}-{operator}", f"unknown{separator}{operator}\n")
for separator_name, separator in [("ff", "\f"), ("continued", "\\\n")]:
    for operator in operators:
        add(
            f"receiver-{separator_name}-{operator}",
            f"object.unknown{separator}{operator}\n",
        )
        add(
            f"safe-receiver-{separator_name}-{operator}",
            f"object&.unknown{separator}{operator}\n",
        )
        for keyword in ["yield", "super"]:
            if keyword == "yield" and operator == "&blk":
                continue
            add(
                f"{keyword}-{separator_name}-{operator}",
                f"def run\n  {keyword}{separator}{operator}\nend\n",
            )
    for operator in ["+1", "-1", "*args", "**kwargs", "&blk", "/2"]:
        add(
            f"local-{separator_name}-{operator}",
            f"unknown = 1\nunknown{separator}{operator}\n",
        )
    add(f"later-local-{separator_name}", f"unknown{separator}+1\nunknown = 1\n")
for name, source in [
    ("ordinary-newline", "unknown\n+1\n"),
    ("indented-newline", "unknown\n  +1\n"),
    ("comment-newline", "unknown\\\n# comment\n  +1\n"),
    ("regexp-newline", "unknown\n  /re/\n"),
    ("parentheses", "unknown(+1)\n"),
    ("spaced-binary", "unknown + 1\n"),
    ("lambda", "unknown ->x { x }\n"),
    ("keyword-return", "def run\n  return *args\nend\n"),
    ("unicode", "名前\\\n+1\n"),
    ("static-regexp-match", "unknown\\\n/re/ =~ text\n"),
    ("indented-static-regexp-match", "unknown\\\n  /re/ =~ text\n"),
    ("block", "unknown\\\n*args do |value|\n  value\nend\n"),
]:
    add(name, source)

groups = []
for target in ["2.7", "3.4"]:
    config = f"AllCops:\n  TargetRubyVersion: {target}\n  NewCops: disable\n"
    for eol in ["\n", "\r\n"]:
        for mode in ["", "-a", "-A"]:
            with tempfile.TemporaryDirectory(prefix="sonicop33-oracle-") as tmp:
                directory = pathlib.Path(tmp)
                (directory / ".rubocop.yml").write_text(config, newline="")
                records = []
                for index, case in enumerate(cases):
                    filename = f"input_{index:03}.rb"
                    source = case["source"].replace("\n", eol)
                    (directory / filename).write_bytes(source.encode())
                    records.append(
                        {"name": case["name"], "file": filename, "source": source}
                    )
                arguments = [
                    "rubocop",
                    "_1.89.0_",
                    "--config",
                    ".rubocop.yml",
                    "--cache",
                    "false",
                    "--only",
                    "Lint/AmbiguousOperator,Lint/AmbiguousRegexpLiteral",
                    "--format",
                    "json",
                ]
                if mode:
                    arguments.append(mode)
                run = subprocess.run(
                    arguments + ["."], cwd=directory, capture_output=True
                )
                report = json.loads(run.stdout)
                assert report["metadata"]["rubocop_version"] == "1.89.0"
                assert report["metadata"]["ruby_version"] == "4.0.8"
                assert report["summary"]["inspected_file_count"] == len(cases), report
                assert report["summary"]["target_file_count"] == len(cases), report
                assert len(report["files"]) == len(cases), report
                by_path = {item["path"]: item["offenses"] for item in report["files"]}
                assert len(by_path) == len(cases), report
                assert set(by_path) == {record["file"] for record in records}, report
                for record in records:
                    record["offenses"] = by_path[record["file"]]
                    assert all(
                        item["cop_name"] != "Lint/Syntax" for item in record["offenses"]
                    ), record
                    record["corrected"] = (
                        (directory / record["file"]).read_bytes().decode()
                    )
                group = {
                    "target": target,
                    "config": config,
                    "eol": eol,
                    "mode": mode,
                    "exit": run.returncode,
                    "cases": records,
                }
                groups.append(group)
                print(
                    json.dumps(
                        {
                            "target": target,
                            "eol": repr(eol),
                            "mode": mode,
                            "exit": run.returncode,
                            "cases": len(records),
                            "offenses": report["summary"]["offense_count"],
                        }
                    ),
                    flush=True,
                )
# バージョン・改行・補正モードの同値を実測してから、重複する記録だけを畳む。
inspected = groups[0]["cases"]
corrected = groups[1]["cases"]
assert len(groups) == 12
for group in groups:
    expected = inspected if not group["mode"] else corrected
    assert group["exit"] == (1 if not group["mode"] else 0)
    for actual, oracle in zip(group["cases"], expected, strict=True):
        assert actual["name"] == oracle["name"]
        assert actual["offenses"] == oracle["offenses"]
        # 本家の書き戻しによる CRLF 正規化だけを分け、裸の CR は保持する。
        assert actual["corrected"].replace("\r\n", "\n") == oracle["corrected"]
fixture = {
    "reference": "RuboCop 1.89.0 / Ruby 4.0.8",
    "verified_targets": ["2.7", "3.4"],
    "verified_runs": 12,
    "cases": [
        {
            "name": before["name"],
            "source": before["source"],
            "inspected_offenses": before["offenses"],
            "corrected_offenses": after["offenses"],
            "corrected_source": after["corrected"],
        }
        for before, after in zip(inspected, corrected, strict=True)
    ],
}
out.write_text(
    json.dumps(fixture, ensure_ascii=False, indent=2) + "\n",
    encoding="utf-8",
    newline="",
)
