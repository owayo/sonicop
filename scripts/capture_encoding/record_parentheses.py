#!/usr/bin/env python3
"""自作の捕捉符号化 fixture を実 RuboCop 1.89.0 から再記録する。候補は実行しない。"""
import argparse
import json
from pathlib import Path
import shlex
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", default="rubocop _1.89.0_")
    parser.add_argument("--fixture", type=Path, default=Path(__file__).resolve().parents[2] /
                        "tests/fixtures/capture_parentheses_reports.json")
    args = parser.parse_args()
    fixture = json.loads(args.fixture.read_text(encoding="utf-8"))
    rows = fixture["cases"] + fixture["controls"] + fixture["reverse"]
    groups = {}
    for row in rows:
        key = (row["ending"], row["mode"], row["only"])
        groups.setdefault(key, []).append(row)
    with tempfile.TemporaryDirectory(prefix="capture-parentheses-") as temporary:
        root = Path(temporary)
        for number, ((ending, mode, only), cases) in enumerate(groups.items()):
            directory = root / str(number)
            directory.mkdir()
            config = directory / ".rubocop.yml"
            config.write_text("AllCops:\n  TargetRubyVersion: 2.7\n"
                              "  ParserEngine: parser_whitequark\n  NewCops: disable\n", encoding="utf-8")
            paths = []
            for index, row in enumerate(cases):
                path = directory / (str(index) + ".rb")
                path.write_bytes(bytes.fromhex(row["source_hex"]))
                paths.append(path)
            command = shlex.split(args.reference) + ["--cache", "false", "--config", str(config),
                                                     "--only", only, "--format", "json"]
            if mode != "inspect":
                command.append("-a" if mode == "safe" else "-A")
            process = subprocess.run(command + list(map(str, paths)), cwd=directory,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
            report = json.loads(process.stdout)
            assert process.returncode in (0, 1), process.stderr
            assert report["metadata"]["rubocop_version"] == "1.89.0"
            assert report["summary"]["inspected_file_count"] == len(paths)
            assert report["summary"]["target_file_count"] == len(paths)
            files = {Path(item["path"]).name: item for item in report["files"]}
            assert set(files) == {path.name for path in paths}
            for row, path in zip(cases, paths):
                row["upstream_offenses"] = files[path.name]["offenses"]
                row["upstream_bytes_hex"] = path.read_bytes().hex()
                row["upstream_exit"] = process.returncode
            print(ending, mode, only, len(paths), "completed", flush=True)
        # 逆方向の診断対照は捕捉名だけ ASCII に替える。同じ行のメソッド引数は移動しない。
        for index, row in enumerate(fixture["reverse"]):
            directory = root / ("method-" + str(index))
            directory.mkdir()
            config = directory / ".rubocop.yml"
            config.write_text("AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n", encoding="utf-8")
            path = directory / "example.rb"
            path.write_bytes(bytes.fromhex(row["source_hex"]).replace(b"\\u3042", b"safe"))
            command = shlex.split(args.reference) + ["--cache", "false", "--config", str(config),
                                                     "--only", row["only"], "--format", "json"]
            if row["mode"] != "inspect":
                command.append("-a" if row["mode"] == "safe" else "-A")
            process = subprocess.run(command + [str(path)], cwd=directory,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
            report = json.loads(process.stdout)
            assert process.returncode in (0, 1), process.stderr
            row["method_control_offenses"] = [offense for offense in report["files"][0]["offenses"]
                                               if offense["cop_name"] != "Lint/Syntax"]
    # expected_safe_hex は元符号化/改行の保存と物理束縛を定める意図的差。候補出力から生成しない。
    args.fixture.write_text(json.dumps(fixture, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
