#!/usr/bin/env python3
"""RuboCop 1.89.0 の multibyte 構文診断を、実 CLI から保存する。"""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path


def ruby_environment():
    # 参照Rubyへの注入だけを除き、gem解決やOS起動に必要な親の環境は維持する。
    environment = os.environ.copy()
    for name in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(name, None)
    return environment


def sources():
    return {
        "diamond_lf_eof.rb": "%💎\n💎".encode(),
        "diamond_crlf_eof.rb": "%💎\r\n💎".encode(),
        "diamond_lf_terminated.rb": "%💎\n💎\n".encode(),
        "diamond_crlf_terminated.rb": "%💎\r\n💎\r\n".encode(),
        "three_byte_eof.rb": "%あ\nあ".encode(),
        "two_byte_eof.rb": "%é\né".encode(),
        "percent_eof.rb": "%💎".encode(),
        "unicode_prefix.rb": "# éあ💎\n%💎\n💎".encode(),
        "operator_eof.rb": "💎 +".encode(),
        "valid_percent.rb": "%q(💎éあ)\n".encode(),
        "valid_percent_crlf.rb": "%Q{💎#{1}éあ}\r\n".encode(),
        "valid_character.rb": "?💎\n".encode(),
        "valid_character_crlf.rb": "?あ\r\n".encode(),
        "valid_unicode.rb": 'é = "あ💎"\né\n'.encode(),
        "binary_percent.rb": "# encoding: ASCII-8BIT\n%💎\n💎".encode(),
        "empty.rb": b"",
        "invalid_utf8.rb": b'# coding: UTF-8\n"\xff"\n',
        "invalid_ascii.rb": '# coding: US-ASCII\n"é"\n'.encode(),
    }


def record(ruby):
    cases = sources()
    groups = []
    with tempfile.TemporaryDirectory(prefix="sonicop-multibyte-syntax-") as temporary:
        directory = Path(temporary)
        wrapper = directory / "reference.rb"
        wrapper.write_text(
            'gem "parser", "3.3.12.0"\n'
            'gem "prism", "1.8.1"\n'
            'gem "rubocop", "1.89.0"\n'
            'load Gem.bin_path("rubocop", "rubocop", "1.89.0")\n',
            encoding="utf-8",
            newline="\n",
        )
        environment = ruby_environment()
        for parser, targets in [
            ("parser_prism", ["3.3", "3.4", "4.0", "4.1"]),
            ("parser_whitequark", ["2.7", "3.3", "3.4"]),
        ]:
            for target in targets:
                yaml = (
                    f"AllCops:\n  TargetRubyVersion: {target}\n"
                    f"  ParserEngine: {parser}\n  NewCops: disable\n"
                )
                (directory / ".rubocop.yml").write_text(
                    yaml, encoding="utf-8", newline="\n"
                )
                for name, source in cases.items():
                    (directory / name).write_bytes(source)
                command = [
                    ruby,
                    str(wrapper),
                    "--cache",
                    "false",
                    "--only",
                    "Lint/Syntax",
                    "--format",
                    "json",
                    *cases,
                ]
                result = subprocess.run(
                    command,
                    cwd=directory,
                    env=environment,
                    capture_output=True,
                    check=False,
                )
                if result.returncode not in (0, 1) or result.stderr:
                    raise RuntimeError(
                        f"{parser} {target}: {result.returncode} {result.stderr!r}"
                    )
                report = json.loads(result.stdout)
                if report["summary"]["inspected_file_count"] != len(cases):
                    raise RuntimeError(f"{parser} {target}: incomplete inspection")
                for name, source in cases.items():
                    if (directory / name).read_bytes() != source:
                        raise RuntimeError(f"{parser} {target}: source changed: {name}")
                groups.append(
                    {
                        "target": target,
                        "parser": parser,
                        "yaml": yaml,
                        "exit": result.returncode,
                        "files": report["files"],
                        "summary": report["summary"],
                    }
                )
    return {
        "provenance": {
            "rubocop": "1.89.0",
            "prism": "1.8.1",
            "parser": "3.3.12.0",
            "command": "rubocop _1.89.0_ --cache false --only Lint/Syntax --format json <files>",
            "description": "実 CLI の原本 byte 入力と診断全フィールド。非補正。",
        },
        "sources": [
            {"path": name, "bytes": list(source)} for name, source in cases.items()
        ],
        "groups": groups,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("tests/fixtures/multibyte_syntax_reports.json"),
    )
    arguments = parser.parse_args()
    fixture = record(arguments.ruby)
    arguments.output.write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(
        f"recorded {len(fixture['groups'])} groups x {len(fixture['sources'])} inputs"
    )


if __name__ == "__main__":
    main()
