#!/usr/bin/env python3
"""LF解析ビューとmixed原本書出しの期待値を本家CLIから記録する。"""
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
cases = {
    "mixed_data_crlf": body + "\n__END__\nalpha\r\nbeta\r\n",
    "mixed_code_crlf": body + "\r\n__END__\nalpha\nbeta\n",
    "mixed_alternating": body + "\r\n__END__\nalpha\r\nbeta\n",
    "all_crlf": body + "\r\n__END__\r\nalpha\r\nbeta\r\n",
    "all_lf": body + "\n__END__\nalpha\nbeta\n",
    "mixed_comment": "a = 1\r\n# \x01 kept\n" + body + "\n__END__\r\nalpha\r\nbeta\n",
    "mixed_comment_nul": "a = 1\r\n# \0 kept\n",
    "mixed_code_nul": "a = 1\0ignored\n# kept\r\n",
    "mixed_heredoc": "puts <<~TEXT\r\nbody\nTEXT\r\n# kept\n",
    "mixed_duplicate_lines": "a = 1\r\na = 1\na = 1\r\n# kept\n",
}
records = []
for target, engine in [("2.7", "parser_whitequark"), ("3.4", "parser_prism")]:
    config = (f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {target}\n"
              f"  ParserEngine: {engine}\nLayout/EndOfLine:\n  EnforcedStyle: lf\n")
    for name, source in cases.items():
        original = source.encode()
        for mode in [None, "-a", "-A"]:
            with tempfile.TemporaryDirectory(prefix="sonicop-mixed-") as directory:
                root = Path(directory)
                (root / ".rubocop.yml").write_text(config)
                (root / "example.rb").write_bytes(original)
                command = ["rubocop", "_1.89.0_", "--cache", "false", "--only",
                           "Layout/EndOfLine,Style/FrozenStringLiteralComment", "--format", "json"]
                if mode:
                    command.append(mode)
                def run():
                    result = subprocess.run(command + ["example.rb"], cwd=root,
                                            capture_output=True, timeout=120)
                    report = json.loads(result.stdout)
                    assert result.returncode in (0, 1), result.returncode
                    assert report["metadata"]["rubocop_version"] == "1.89.0"
                    assert report["summary"]["target_file_count"] == 1
                    assert report["summary"]["inspected_file_count"] == 1
                    assert len(report["files"]) == 1
                    assert Path(report["files"][0]["path"]).name == "example.rb"
                    assert report["summary"]["offense_count"] == len(report["files"][0]["offenses"])
                    assert not result.stderr, result.stderr
                    return dict(exit=result.returncode, offenses=report["files"][0]["offenses"])
                first = run()
                reference_after = (root / "example.rb").read_bytes()
                reference_repeated = run()
                assert (root / "example.rb").read_bytes() == reference_after
                expected = original
                if reference_after != original:
                    prefix = b"# frozen_string_literal: true\n"
                    assert reference_after == prefix + original.replace(b"\r\n", b"\n"), name
                    # 本家の有効な挿入は採用し、copy範囲の原本改行だけを保持する意図差。
                    pure_crlf = b"\r\n" in original and b"\n" not in original.replace(b"\r\n", b"")
                    expected = (prefix.replace(b"\n", b"\r\n") if pure_crlf else prefix) + original
                (root / "example.rb").write_bytes(expected)
                repeated = run()
                assert (root / "example.rb").read_bytes() == expected
                data_hex = original.split(b"__END__", 1)[1].split(b"\n", 1)[1].hex() if b"__END__" in original else None
                records.append(dict(name=name, target=target, engine=engine, mode=mode,
                                    config=config, source_hex=original.hex(), first=first,
                                    reference_after_hex=reference_after.hex(), after_hex=expected.hex(),
                                    repeated=repeated, reference_repeated=reference_repeated,
                                    data_hex=data_hex))
fixture = dict(rubocop_version="1.89.0", ruby_version="4.0.8", records=records)
args.output.write_text(json.dumps(fixture, ensure_ascii=False, indent=2) + "\n")
