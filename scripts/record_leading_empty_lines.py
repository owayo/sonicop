#!/usr/bin/env python3
"""RuboCop 1.89.0 の先頭tokenとLeadingEmptyLinesの実CLI記録を再生成する。"""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

CASES = [
    ("empty", ""),
    ("one_blank", "\n"),
    ("whitespace_only", " \t\n\n"),
    ("escaped_lf", "\\\n"),
    ("escaped_lf_nul", "\\\n\0"),
    ("escaped_lf_nul_tail", "\\\n\0a=1\n"),
    ("blank_escaped_nul", "\n \\\n\0ignored\n"),
    ("two_escaped_nul", "\\\n\\\n\0"),
    ("nul_only", "\0"),
    ("blank_nul", "\n\0"),
    ("nul_tail", " \0ignored\n"),
    ("first_comment", "# comment\n"),
    ("blank_comment", "\n# comment\n"),
    ("escaped_comment", "\\\n# comment\n"),
    ("blank_escaped_comment", "\n\\\n# comment\n"),
    ("comment_nul_soh", "\n# \0\x01 kept\n"),
    ("first_block_comment", "=begin\ncomment\n=end\n"),
    ("blank_block_comment", "\n=begin\ncomment\n=end\n"),
    ("escaped_block_comment", "\\\n=begin\ncomment\n=end\n"),
    ("first_code", "puts 1\n"),
    ("blank_code", "\nputs 1\n"),
    ("two_blank_code", "\n\nputs 1\n"),
    ("blank_spaces_code", "\n \tputs 1\n"),
    ("escaped_code", "\\\nputs 1\n"),
    ("blank_escaped_code", "\n\\\nputs 1\n"),
    ("unicode_code", "\n名前 = 1\n"),
    ("first_semicolon", ";"),
    ("blank_semicolon", "\n;"),
    ("escaped_semicolon", "\\\n;"),
    ("semicolon_nul", ";\0ignored\n"),
    ("first_heredoc", "a = <<~TXT\nbody\nTXT\n"),
    ("blank_heredoc", "\na = <<~TXT\nbody\nTXT\n"),
    ("literal_escaped_lf", "\"a\\\nb\"\n"),
    ("literal_nul_soh", "\n\"a\0\x01b\"\n"),
    ("data_only", "__END__\nignored\n"),
    ("blank_data_only", "\n__END__\nignored\n"),
    ("escaped_data_only", "\\\n__END__\nignored\n"),
    ("blank_comment_data", "\n# comment\n__END__\nignored\n"),
    ("blank_code_nul_data", "\na = 1\0ignored\n__END__\ndata\n"),
    ("blank_nul_data", "\n\0\n__END__\ndata\n"),
    ("blank_quoted", "\n\"abc\"\n"),
    ("blank_single_quoted", "\n'abc'\n"),
    ("blank_empty_quoted", "\n\"\"\n"),
    ("blank_escaped_literal", "\n\"a\\\nb\"\n"),
    ("blank_escaped_single", "\n'a\\\nb'\n"),
    ("blank_multiline_literal", "\n\"a\nb\"\n"),
    ("blank_multiline_single", "\n'a\nb'\n"),
    ("blank_nul_single", "\n'a\0b'\n"),
    ("blank_interpolation", "\n\"a#{1}b\"\n"),
    ("blank_percent_literal", "\n%q(a)\n"),
    ("blank_symbol", "\n:foo\n"),
    ("blank_regexp", "\n/abc/\n"),
    ("bom_only", "\ufeff"),
    ("bom_blank", "\ufeff\n"),
    ("bom_blank_comment", "\ufeff\n# comment\n"),
    ("bom_blank_code", "\ufeff\nputs 1\n"),
]

def inspect(command, directory, mode):
    args = ["--config", ".rubocop.yml", "--cache", "false", "--only",
            "Layout/LeadingEmptyLines", "--format", "json"]
    if mode != "inspect":
        args.append(mode)
    run = subprocess.run(command + args + ["."], cwd=directory, capture_output=True, timeout=120)
    report = json.loads(run.stdout)
    assert run.returncode in (0, 1) and not run.stderr, run.stderr
    assert report["metadata"]["rubocop_version"] == "1.89.0"
    assert report["metadata"]["ruby_version"] == "4.0.8"
    report.pop("metadata")
    assert report["summary"]["target_file_count"] == report["summary"]["inspected_file_count"] == len(CASES)
    assert len(report["files"]) == len(CASES)
    assert {f["path"] for f in report["files"]} == {f"input_{i:03}.rb" for i in range(len(CASES))}
    return {"exit": run.returncode, "report": report,
            "after_hex": [(directory / f"input_{i:03}.rb").read_bytes().hex() for i in range(len(CASES))]}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    records = []
    for ruby, engine in [("2.7", "parser_whitequark"), ("3.4", "parser_prism")]:
        for ending in ["LF", "CRLF"]:
            for mode in ["inspect", "-a", "-A"]:
                with tempfile.TemporaryDirectory(prefix="sonicop-leading-empty-") as temporary:
                    directory = Path(temporary)
                    (directory / ".rubocop.yml").write_text(
                        f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {engine}\n",
                        encoding="utf-8",
                    )
                    sources = [(s.replace("\n", "\r\n") if ending == "CRLF" else s).encode() for _, s in CASES]
                    for i, source in enumerate(sources):
                        (directory / f"input_{i:03}.rb").write_bytes(source)
                    first = inspect(["rubocop", "_1.89.0_", "--raise-cop-error"], directory, mode)
                    second = inspect(["rubocop", "_1.89.0_", "--raise-cop-error"], directory, mode)
                    records.append({"ruby": ruby, "engine": engine, "ending": ending, "mode": mode,
                                    "source_hex": [s.hex() for s in sources], "first": first, "second": second})
    result = {"reference": "RuboCop 1.89.0 / Ruby 4.0.8", "cases": [name for name, _ in CASES], "groups": records}
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

if __name__ == "__main__":
    main()
