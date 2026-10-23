#!/usr/bin/env python3
"""本家の生補正と、元DATAを保持する既存方針の出力を区別して記録する。"""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("output", type=Path)
args = parser.parse_args()
assert subprocess.check_output(["ruby", "-e", "print RUBY_VERSION"], text=True) == "4.0.8"
body = 'STDOUT.write(DATA.read.unpack1("H*"))'
cases = {}
for name, data in {
    "control_pair": "x\r\ny\r\n",
    "double": "x\r\r\ny\r\n",
    "triple": "x\r\r\r\ny\r\n",
    "double_twice": "x\r\r\nz\r\r\np\r\n",
    "bare_separate": "x\rZ\r\nw\r\n",
    "only_bare": "x\ry\r\nz\r\n",
    "empty": "",
    "unterminated": "x\r\r\ny\r",
}.items():
    cases["pure_" + name] = (body + "\r\n__END__\r\n" + data, data, True)
for name, data in {
    "control_pair": "x\r\ny\n",
    "double": "x\r\r\ny\n",
    "triple": "x\r\r\r\ny\n",
    "double_twice": "x\r\r\nz\r\r\np\n",
    "bare_separate": "x\rZ\nw\r\n",
    "only_bare": "x\ry\nz\n",
    "lf": "x\ny\n",
    "unterminated": "x\r",
}.items():
    cases["mixed_" + name] = (body + "\n__END__\n" + data, data, False)
records = []
for target, engine in [("2.7", "parser_whitequark"), ("3.4", "parser_prism")]:
    config = f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {target}\n  ParserEngine: {engine}\n"
    for cops in [["Style/FrozenStringLiteralComment"], ["Style/FrozenStringLiteralComment", "Layout/EmptyLineAfterMagicComment"]]:
        for mode in [None, "-a", "-A"]:
            with tempfile.TemporaryDirectory(prefix="sonicop-pure-cr-") as directory:
                root = Path(directory)
                (root / ".rubocop.yml").write_text(config)
                paths = [name + ".rb" for name in cases]
                for name, (source, _, _) in cases.items():
                    (root / (name + ".rb")).write_bytes(source.encode())
                command = ["rubocop", "_1.89.0_", "--config", ".rubocop.yml", "--cache", "false", "--only", ",".join(cops), "--format", "json"]
                if mode:
                    command.append(mode)
                def run():
                    result = subprocess.run(command + paths, cwd=root, capture_output=True, timeout=120)
                    report = json.loads(result.stdout)
                    assert result.returncode in (0, 1) and not result.stderr
                    assert report["metadata"]["rubocop_version"] == "1.89.0"
                    assert report["summary"]["target_file_count"] == report["summary"]["inspected_file_count"] == len(paths) > 0
                    actual_paths = [Path(f["path"]).name for f in report["files"]]
                    assert len(actual_paths) == len(set(actual_paths)) == len(paths)
                    assert set(actual_paths) == set(paths)
                    assert report["summary"]["offense_count"] == sum(len(f["offenses"]) for f in report["files"])
                    return dict(exit=result.returncode, files={Path(f["path"]).name: f["offenses"] for f in report["files"]})
                def runtime(path):
                    result = subprocess.run(["ruby", path], cwd=root, capture_output=True, timeout=20)
                    assert result.returncode == 0 and not result.stderr
                    return result.stdout.hex()
                original_runtime = {path: runtime(path) for path in paths}
                first = run()
                reference_after = {path: (root / path).read_bytes() for path in paths}
                reference_runtime = {path: runtime(path) for path in paths}
                reference_repeated = run()
                assert all((root / path).read_bytes() == reference_after[path] for path in paths)
                expected = {}
                for name, (source, data, pure) in cases.items():
                    path = name + ".rb"
                    assert bytes.fromhex(original_runtime[path]) == data.encode().hex().encode()
                    expected[path] = source.encode()
                    if reference_after[path] != source.encode():
                        assert mode == "-A"
                        assert {o["cop_name"] for o in first["files"][path] if o["corrected"]} == set(cops)
                        prefix = "# frozen_string_literal: true\n" + ("\n" if len(cops) == 2 else "")
                        assert reference_after[path].startswith(prefix.encode())
                        expected[path] = (prefix.replace("\n", "\r\n") if pure else prefix).encode() + source.encode()
                    (root / path).write_bytes(expected[path])
                    assert runtime(path) == original_runtime[path]
                repeated = run()
                assert all((root / path).read_bytes() == expected[path] for path in paths)
                for name, (source, data, pure) in cases.items():
                    path = name + ".rb"
                    records.append(dict(name=name, target=target, engine=engine, config=config, cops=cops, mode=mode,
                        source_hex=source.encode().hex(), pure=pure, data_hex=data.encode().hex(),
                        first=dict(exit=first["exit"], offenses=first["files"][path]),
                        reference_after_hex=reference_after[path].hex(), after_hex=expected[path].hex(),
                        repeated=dict(exit=repeated["exit"], offenses=repeated["files"][path]),
                        reference_repeated=dict(exit=reference_repeated["exit"], offenses=reference_repeated["files"][path]),
                        original_runtime_hex=original_runtime[path], reference_runtime_hex=reference_runtime[path]))
assert len(records) == 192
args.output.write_text(json.dumps(dict(rubocop_version="1.89.0", ruby_version="4.0.8", records=records), ensure_ascii=False, indent=2) + "\n")