#!/usr/bin/env python3
"""元bare CRの複数pass保持と、本家の生bytes/実行値を別々に記録する。"""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("output", type=Path)
args = parser.parse_args()
assert subprocess.check_output(["ruby", "-e", "print RUBY_VERSION"], text=True) == "4.0.8"
body = 'STDOUT.write(DATA.read.unpack1("H*"))\n__END__\n'
cases = {
    "double": body + "x\r\r\ny\n",
    "triple": body + "x\r\r\r\ny\n",
    "control_pair": body + "x\r\ny\n",
    "bare_separate": body + "x\rZ\nw\r\n",
    "only_bare": body + "x\ry\nz\n",
    "double_twice": body + "x\r\r\nz\r\r\np\n",
}
only = "Style/FrozenStringLiteralComment,Layout/EmptyLineAfterMagicComment"
records = []
for target, engine in [("2.7", "parser_whitequark"), ("3.4", "parser_prism")]:
    config = (f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {target}\n"
              f"  ParserEngine: {engine}\n")
    for name, source in cases.items():
        original = source.encode()
        for mode in [None, "-a", "-A"]:
            with tempfile.TemporaryDirectory(prefix="sonicop-bare-cr-") as directory:
                root = Path(directory)
                (root / ".rubocop.yml").write_text(config)
                (root / "example.rb").write_bytes(original)
                command = ["rubocop", "_1.89.0_", "--cache", "false", "--only", only, "--format", "json"]
                if mode:
                    command.append(mode)
                def run():
                    result = subprocess.run(command + ["example.rb"], cwd=root, capture_output=True, timeout=120)
                    report = json.loads(result.stdout)
                    assert result.returncode in (0, 1) and not result.stderr
                    assert report["metadata"]["rubocop_version"] == "1.89.0"
                    assert report["summary"]["target_file_count"] == report["summary"]["inspected_file_count"] == 1
                    assert len(report["files"]) == 1 and Path(report["files"][0]["path"]).name == "example.rb"
                    assert report["summary"]["offense_count"] == len(report["files"][0]["offenses"])
                    return dict(exit=result.returncode, offenses=report["files"][0]["offenses"])
                def runtime():
                    result = subprocess.run(["ruby", "example.rb"], cwd=root, capture_output=True)
                    assert result.returncode == 0 and not result.stderr
                    return result.stdout.hex()
                original_runtime = runtime()
                first = run()
                reference_after = (root / "example.rb").read_bytes()
                reference_runtime = runtime()
                reference_repeated = run()
                assert (root / "example.rb").read_bytes() == reference_after
                expected = original
                if reference_after != original:
                    assert mode == "-A"
                    assert {o["cop_name"] for o in first["offenses"] if o["corrected"]} == set(only.split(","))
                    prefix = b"# frozen_string_literal: true\n\n"
                    assert reference_after == prefix + original.replace(b"\r\n", b"\n").replace(b"\r\n", b"\n")
                    expected = prefix + original
                (root / "example.rb").write_bytes(expected)
                assert runtime() == original_runtime
                repeated = run()
                assert (root / "example.rb").read_bytes() == expected
                records.append(dict(
                    name=name, target=target, engine=engine, mode=mode, config=config,
                    source_hex=original.hex(), first=first, reference_after_hex=reference_after.hex(),
                    after_hex=expected.hex(), repeated=repeated, reference_repeated=reference_repeated,
                    data_hex=original.split(b"__END__\n", 1)[1].hex(),
                    original_runtime_hex=original_runtime, reference_runtime_hex=reference_runtime,
                ))
args.output.write_text(json.dumps(dict(rubocop_version="1.89.0", ruby_version="4.0.8", only=only, records=records),
                                  ensure_ascii=False, indent=2) + "\n")
