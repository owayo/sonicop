#!/usr/bin/env python3
"""本家1.89.0からEndOfLine末尾tokenの診断・非補正bytesを記録する。"""

import argparse
import json
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("output", type=Path)
args = parser.parse_args()
ruby_version = subprocess.check_output(["ruby", "-e", "print RUBY_VERSION"], text=True)
assert ruby_version == "4.0.8", ruby_version
assert subprocess.check_output(["rubocop", "_1.89.0_", "--version"], text=True).strip() == "1.89.0"
base = {'code_comment': 'a=1\n# kept\n',
 'code_soh_comment': 'a=1\n# \x01 kept\n',
 'code_nul_comment': 'a=1\n# \x00 kept\n',
 'unicode_comment': '猫=1\n# 日本語\n',
 'code_blank_comment': 'a=1\n\n# kept\n\n',
 'code_blank': 'a=1\n\n',
 'comments_only': '# first\n# kept\n\n',
 'block_comment_only': '=begin\nkept\n=end\n',
 'code_block_comment': 'a=1\n=begin\nkept\n=end\n',
 'semicolon_comment': 'a=1;\n# kept\n',
 'only_semicolon_comment': ';\n# kept\n',
 'continued_comment': 'a=1\\\n# kept\n',
 'continued_backslash_comment': 'a=1\\\n# kept\\\n',
 'heredoc_comment': 'puts <<~TEXT\nbody\nTEXT\n# kept\n',
 'heredoc_chain_comment': 'puts(<<~TEXT\nbody\nTEXT\n).upcase\n# kept\n',
 'code_nul_tail': 'a=1\x00ignored\n# kept\n',
 'code_nul_line': 'a=1\n\x00ignored\n# kept\n',
 'nul_comment_marker': 'a=1\n# \x00 kept\n__END__\nbody\n',
 'nul_code_marker': 'a=1\n\x00ignored\n__END__\nbody\n',
 'soh_code': 'a=1\n\x01# kept\n',
 'only_marker': '__END__\nbody\n',
 'code_marker': 'a=1\n__END__\nbody\n',
 'empty': '',
 'only_nul': '\x00',
 'comment_eof': '# kept',
 'code_eof': 'a=1',
 'eof_line_comment': 'a=1\n# kept',
 'eof_line_comment_cr': 'a=1\n# kept\r',
 'eof_multi_comment': 'a=1\n# first\n# kept',
 'eof_block_comment': 'a=1\n=begin\nkept\n=end',
 'eof_block_comment_space': 'a=1\n=begin\nkept\n=end  ',
 'eof_block_then_line': 'a=1\n=begin\nkept\n=end\n# kept',
 'eof_line_then_block': 'a=1\n# first\n=begin\nkept\n=end',
 'eof_heredoc_comment': 'a=<<A\nbody\nA\n# kept',
 'eof_heredoc_comment_cr': 'a=<<A\nbody\nA\n# kept\r',
 'eof_continued_comment': 'a=1\\\n# kept',
 'eof_semicolon_comment': 'a=1;\n# kept',
 'eof_embedded_comment_only': '=begin\nkept\n=end',
 'eof_nul_comment': 'a=1\n# \x00 kept',
 'eof_soh_comment': 'a=1\n# \x01 kept',
 'eof_line_trailing_space': 'a=1\n# kept \t',
 'eof_code_nul': 'a=1\x00ignored\n# kept',
 'eof_line_blank_space': 'a=1\n# kept\n '}
cases = {}
for name, source in base.items():
    for ending in ["lf", "crlf"]:
        cases[name + "-" + ending + ".rb"] = source.replace(
            "\n", "\r\n" if ending == "crlf" else "\n"
        ).encode().hex()
mixed = {
    "mixed_lf_code": "a=1\n# \x01 kept\r\n",
    "mixed_crlf_code": "a=1\r\n# \x01 kept\n",
    "mixed_lf_blank": "a=1\n\r\n",
    "mixed_crlf_blank": "a=1\r\n\n",
    "mixed_heredoc": "puts <<~TEXT\nbody\r\nTEXT\r\n# kept\r\n",
    "mixed_marker": "a=1\n__END__\r\nbody\r\n",
}
cases.update({name + ".rb": source.encode().hex() for name, source in mixed.items()})
fixture = dict(
    rubocop_version="1.89.0", ruby_version=ruby_version,
    only="Layout/EndOfLine", cases=cases, groups=[],
)
for target, engine in [("2.7", "parser_whitequark"), ("3.4", "parser_prism")]:
    for style in ["lf", "crlf", "native"]:
        config = (
            f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {target}\n"
            f"  ParserEngine: {engine}\nLayout/EndOfLine:\n  EnforcedStyle: {style}\n"
        )
        expected = None
        for mode in [None, "-a", "-A"]:
            with tempfile.TemporaryDirectory(prefix="sonicop-endings-") as directory:
                root = Path(directory)
                (root / ".rubocop.yml").write_text(config)
                for name, value in cases.items():
                    (root / name).write_bytes(bytes.fromhex(value))
                command = [
                    "rubocop", "_1.89.0_", "--cache", "false",
                    "--only", fixture["only"], "--format", "json",
                ]
                if mode:
                    command.append(mode)
                result = subprocess.run(
                    command + sorted(cases), cwd=root, capture_output=True, timeout=180
                )
                report = json.loads(result.stdout)
                # 未完了・重複・対象欠落を一致へ数えず、SOH除外より先に全対象を確かめる。
                assert result.returncode in (0, 1), result.returncode
                assert report["metadata"]["rubocop_version"] == "1.89.0"
                assert report["summary"]["target_file_count"] == len(cases) > 0
                assert report["summary"]["inspected_file_count"] == len(cases)
                paths = [Path(file["path"]).name for file in report["files"]]
                assert len(paths) == len(set(paths)) == len(cases)
                assert set(paths) == set(cases)
                assert report["summary"]["offense_count"] == sum(
                    len(file["offenses"]) for file in report["files"]
                )
                files = [
                    dict(path=Path(file["path"]).name, offenses=file["offenses"])
                    for file in report["files"]
                ]
                # SOH自体の白parser文言差は別件として扱い、Prism対照は残す。
                files = [file for file in files if engine != "parser_whitequark"
                         or not file["path"].startswith("soh_code-")]
                current = dict(exit=result.returncode, files=files)
                assert not result.stderr, result.stderr
                assert all((root / name).read_bytes().hex() == value
                           for name, value in cases.items())
                if expected is None:
                    expected = current
                else:
                    assert current == expected, (target, engine, style, mode)
        fixture["groups"].append(dict(
            target=target, engine=engine, style=style, config=config, **expected
        ))
args.output.write_text(json.dumps(fixture, ensure_ascii=False, indent=2) + "\n")
