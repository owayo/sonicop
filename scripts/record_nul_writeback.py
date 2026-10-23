#!/usr/bin/env python3
"""RuboCop 1.89.0 の NUL 補正を実ファイルで再計測する。

mise exec -- python3 scripts/record_nul_writeback.py tests/fixtures/nul_writeback_cli.json
"""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

cases = [
    ('comment', 'a=1\n# \x00 kept\n'),
    ('soh-comment', 'a=1\n# \x01 kept\n'),
    ('multiple-comment', 'a=1\n# \x00\x01\x00 kept\nb=2\n# tail \x00\n'),
    ('comment-before-code', '# \x00 kept\na=1\n'),
    ('comment-eof', 'a=1\n# kept \x00'),
    ('comment-only', '# kept \x00\n'),
    ('literal-double', 'a="x\x00y"\nb=2\n'),
    ('literal-single', "a='x\x00y'\nb=2\n"),
    ('literal-regexp', 'a=/x\x00y/\nb=2\n'),
    ('literal-heredoc', 'a=<<~TXT\nx\x00y\nTXT\nb=2\n'),
    ('literal-words', 'a=%w[x\x00y]\nb=2\n'),
    ('code-middle', 'a=1\x00b=2\nignored ) tail   \n'),
    ('code-line-start', 'a=1\n\x00b=2\nignored ) tail   \n'),
    ('code-eof', 'a=1\x00'),
    ('code-first', '\x00a=1\n'),
    ('code-only', '\x00'),
    ('code-long-tail', 'a=1\x00xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('code-comment-tail', 'a=1\x00# TODO tail   \n\n\n'),
    ('data', 'a=1\n__END__\n\x00\x01\x00 data   \n'),
    ('data-long', 'a=1\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\x00   \n\n\n'),
    ('unicode-comment', '名前=1\n# あ\x00い\x01う\x00 preserved\n'),
    ('plain-control', 'a=1\n# normal\n'),
    ('code-data', 'a=1\x00x\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('first-code-data', '\x00\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('whitespace-code-data', ' \t\\\n\x00\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('comment-code-data', '# comment\n\x00\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('semi-code-data', ';\x00\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('literal-fake-data', 'a="\n__END__\n"\n\x00\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('heredoc-fake-data', 'a=<<~TXT\n__END__\nTXT\n# trailing\n\x00\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('comment-fake-data', '# __END__\na=1\x00\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('marker-space', 'a=1\x00\n__END__ \nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('marker-sameline', 'a=1\x00__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('multi-code-data', 'a=1\n# \x00\x01\x00 kept\n\x00\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
    ('bom-code-data', '\ufeff\x00\n__END__\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx   \n\n\n'),
]


def record_stdin(output, groups):
    selected = {
        "comment", "soh-comment", "multiple-comment", "literal-double",
        "code-middle", "code-only", "data", "plain-control",
        "code-data", "first-code-data", "comment-code-data",
        "literal-fake-data", "heredoc-fake-data", "marker-space",
    }
    records = []
    for group in groups:
        if group["selection"] != "file-cops":
            continue
        for case in group["cases"]:
            if case["name"] not in selected:
                continue
            with tempfile.TemporaryDirectory(prefix="sonicop-nul-stdin-") as temporary:
                directory = Path(temporary)
                (directory / ".rubocop.yml").write_text(
                    f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {group['ruby']}\n"
                    f"  ParserEngine: {group['engine']}\n", encoding="utf-8",
                )
                source = bytes.fromhex(case["source_hex"])
                command = [
                    "rubocop", "_1.89.0_", "--config", ".rubocop.yml", "--cache", "false", "--only",
                    "Layout/SpaceAroundOperators,Layout/TrailingWhitespace,Layout/TrailingEmptyLines,Layout/LineLength,Lint/EmptyFile",
                    "--stdin", "example.rb",
                ]
                if group["mode"] != "inspect":
                    command.append(group["mode"])
                json_run = subprocess.run(
                    command + ["-f", "json"], input=source, cwd=directory,
                    capture_output=True, timeout=120, check=False,
                )
                assert json_run.returncode in (0, 1) and not json_run.stderr, json_run.stderr
                report = json.loads(json_run.stdout)
                assert report["metadata"]["rubocop_version"] == "1.89.0"
                assert report["metadata"]["ruby_version"] == "4.0.8"
                summary = report["summary"]
                assert summary["target_file_count"] == summary["inspected_file_count"] == 1
                assert len(report["files"]) == 1 and report["files"][0]["path"] == "example.rb"
                simple = subprocess.run(
                    command + ["-f", "simple"], input=source, cwd=directory,
                    capture_output=True, timeout=120, check=False,
                )
                assert simple.returncode == json_run.returncode and not simple.stderr
                separator = b"====================\n"
                after = simple.stdout.split(separator, 1)[1] if separator in simple.stdout else None
                assert (after is None) == (group["mode"] == "inspect")
                records.append({
                    **{key: group[key] for key in ["ruby", "engine", "eol", "mode"]},
                    "name": case["name"], "source_hex": case["source_hex"],
                    "exit": json_run.returncode, "offenses": report["files"][0]["offenses"],
                    "stdout_hex": simple.stdout.hex(), "after_hex": None if after is None else after.hex(),
                })
    assert len(records) == 252
    output.write_text(json.dumps(records, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def record_endings(output):
    cases = [
        ('code-tail-crlf', 'a=1\x00ignored\ntrailing\r\n'),
        ('comment-tail-crlf', '# comment\n\x00ignored\r\n'),
        ('blank-tail-crlf', ' \n\x00ignored\r\n'),
        ('semi-tail-crlf', ';\x00ignored\r\n'),
        ('string-tail-crlf', 'a="line\n__END__\n"\n\x00ignored\r\n'),
        ('heredoc-tail-crlf', 'a=<<~TXT\nx\nTXT\n# trailing\n\x00ignored\r\n'),
        ('heredoc-body-crlf', 'a=<<~TXT\nx\r\nTXT\n\x00ignored\n'),
        ('real-data-crlf', 'a=1\x00x\n__END__\r\nignored\r\n'),
        ('nul-only', '\x00'),
        ('soh-control', 'a = 1\n# \x01 kept\n'),
    ]
    records = []
    for ruby, engine in [('2.7', 'parser_whitequark'), ('3.3', 'parser_whitequark'), ('3.4', 'parser_prism')]:
        for mode in ['inspect', '-a', '-A']:
            with tempfile.TemporaryDirectory(prefix='sonicop-nul-endings-') as tmp:
                d = Path(tmp)
                (d / '.rubocop.yml').write_text(f'AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {engine}\nLayout/EndOfLine:\n  EnforcedStyle: lf\n')
                for i, (_, source) in enumerate(cases):
                    (d / f'input_{i:03}.rb').write_bytes(source.encode())
                cmd = ['rubocop', '_1.89.0_', '--config', '.rubocop.yml', '--cache', 'false', '--only', 'Layout/SpaceAroundOperators,Layout/EndOfLine,Style/FrozenStringLiteralComment,Layout/SpaceAfterSemicolon', '-f', 'json']
                if mode != 'inspect':
                    cmd.append(mode)
                run = subprocess.run(cmd + ['.'], cwd=d, capture_output=True, timeout=120)
                report = json.loads(run.stdout)
                assert run.returncode in (0, 1) and (not run.stderr) and (report['metadata']['rubocop_version'] == '1.89.0')
                assert report['summary']['target_file_count'] == report['summary']['inspected_file_count'] == 10 and len(report['files']) == 10
                by = {f['path']: f for f in report['files']}
                assert set(by) == {f'input_{i:03}.rb' for i in range(10)}
                records.append({'ruby': ruby, 'engine': engine, 'mode': mode, 'exit': run.returncode, 'cases': [{'name': name, 'source_hex': source.encode().hex(), 'after_hex': (d / f'input_{i:03}.rb').read_bytes().hex(), 'offenses': by[f'input_{i:03}.rb']['offenses']} for i, (name, source) in enumerate(cases)]})
    assert len(records) == 9
    output.write_text(json.dumps(records, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


def record_semicolons(output):
    # 本家のtoken走査と、原本を残すNUL解析投影の境界を別々に検査する。
    cases = [
        ("same-line-tail", "a=1;\0kept;\n"),
        ("later-line-tail", "a=1;\0\nkept;\n"),
        ("tail-only-semicolon", "a=1\0kept;\n"),
        ("tokenless-tail", "\0ignored;\n"),
        ("multiple-nuls", "a=1;\0kept;\0tail;\n"),
        ("nul-before-data", "a=1;\0x\n__END__\nkept;\n"),
        ("double-quoted-nul", "a=\"x\0y\";\n"),
        ("single-quoted-nul", "a='x\0y';\n"),
        ("comment-nul", "a=1; # \0 kept;\n"),
        ("original-soh-literal", "a=\"x\1y\";\n"),
        ("original-soh-comment", "a=1; # \1 kept;\n"),
        ("expression-separators", "a=1;b=2;\0ignored;\n"),
        ("heredoc-nul", "a=<<~TXT\nx\0;y\nTXT\nb=2;\n"),
        ("unicode-before-nul", "名前=1;\0ignored;\n"),
        ("ordinary-semicolon", "a=1;\n"),
        ("literal-semicolon", "a=\"x\0;y\";\n"),
    ]
    groups = []
    for ruby, engine in [("2.7", "parser_whitequark"), ("3.3", "parser_whitequark"), ("3.4", "parser_prism")]:
        for eol in ["LF", "CRLF"]:
            for mode in ["inspect", "-a", "-A"]:
                with tempfile.TemporaryDirectory(prefix="sonicop-nul-semicolons-") as temporary:
                    directory = Path(temporary)
                    (directory / ".rubocop.yml").write_text(
                        f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {engine}\n",
                        encoding="utf-8",
                    )
                    sources = []
                    for index, (_, source) in enumerate(cases):
                        raw = source.encode("utf-8")
                        if eol == "CRLF":
                            raw = raw.replace(b"\n", b"\r\n")
                        sources.append(raw)
                        (directory / f"input_{index:03}.rb").write_bytes(raw)
                    command = ["rubocop", "_1.89.0_", "--config", ".rubocop.yml", "--cache", "false", "--only", "Style/Semicolon", "--format", "json"]
                    if mode != "inspect":
                        command.append(mode)
                    run = subprocess.run(command + ["."], cwd=directory, capture_output=True, timeout=120)
                    assert run.returncode in (0, 1) and not run.stderr, run.stderr
                    report = json.loads(run.stdout)
                    assert report["metadata"]["rubocop_version"] == "1.89.0"
                    assert report["metadata"]["ruby_version"] == "4.0.8"
                    assert report["summary"]["target_file_count"] == report["summary"]["inspected_file_count"] == len(cases)
                    files = report["files"]
                    assert len(files) == len(cases)
                    by_path = {file["path"]: file for file in files}
                    assert set(by_path) == {f"input_{index:03}.rb" for index in range(len(cases))}
                    groups.append({
                        "ruby": ruby, "engine": engine, "eol": eol, "mode": mode, "exit": run.returncode,
                        "cases": [{"name": name, "source_hex": sources[index].hex(), "after_hex": (directory / f"input_{index:03}.rb").read_bytes().hex(), "offenses": by_path[f"input_{index:03}.rb"]["offenses"]} for index, (name, _) in enumerate(cases)],
                    })
    assert len(groups) == 18 and len(cases) == 16
    output.write_text(json.dumps(groups, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--stdin-output", type=Path)
    parser.add_argument("--endings-output", type=Path)
    parser.add_argument("--semicolons-only", action="store_true")
    args = parser.parse_args()
    if args.semicolons_only:
        record_semicolons(args.output)
        return
    records = []
    for ruby, engine in [("2.7", "parser_whitequark"), ("3.3", "parser_whitequark"), ("3.4", "parser_prism")]:
        for eol in ["LF", "CRLF"]:
            for mode in ["inspect", "-a", "-A"]:
                for selection, only in [
                    ("operator", "Layout/SpaceAroundOperators"),
                    ("file-cops", "Layout/SpaceAroundOperators,Layout/TrailingWhitespace,Layout/TrailingEmptyLines,Layout/LineLength,Lint/EmptyFile"),
                ]:
                    with tempfile.TemporaryDirectory(prefix="sonicop-nul-oracle-") as temporary:
                        directory = Path(temporary)
                        (directory / ".rubocop.yml").write_text(
                            f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {engine}\n", encoding="utf-8"
                        )
                        inputs = {}
                        for index, (name, source) in enumerate(cases):
                            data = source.encode("utf-8")
                            if eol == "CRLF":
                                data = data.replace(b"\n", b"\r\n")
                            path = f"input_{index:03}.rb"
                            inputs[path] = data
                            (directory / path).write_bytes(data)
                        command = ["rubocop", "_1.89.0_", "--config", ".rubocop.yml", "--cache", "false", "--format", "json", "--only", only]
                        if mode != "inspect":
                            command.append(mode)
                        run = subprocess.run(command + ["."], cwd=directory, capture_output=True, timeout=120, check=False)
                        report = json.loads(run.stdout)
                        assert run.returncode in (0, 1) and not run.stderr, run.stderr
                        assert report["metadata"]["rubocop_version"] == "1.89.0"
                        assert report["metadata"]["ruby_version"] == "4.0.8"
                        summary = report["summary"]
                        assert summary["target_file_count"] == summary["inspected_file_count"] == len(cases)
                        files = report["files"]
                        assert len(files) == len(cases) and {file["path"] for file in files} == set(inputs)
                        by_path = {file["path"]: file for file in files}
                        outputs = []
                        for index, (name, _) in enumerate(cases):
                            path = f"input_{index:03}.rb"
                            outputs.append({"name": name, "source_hex": inputs[path].hex(), "after_hex": (directory / path).read_bytes().hex(), "offenses": by_path[path]["offenses"]})
                        records.append({"ruby": ruby, "engine": engine, "eol": eol, "mode": mode, "selection": selection, "exit": run.returncode, "cases": outputs})
    assert len(cases) == 34 and len(records) == 36
    result = {"reference": "RuboCop 1.89.0 / Ruby 4.0.8", "cases": [{"name": name, "source": source} for name, source in cases], "groups": records}
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.stdin_output:
        record_stdin(args.stdin_output, records)
    if args.endings_output:
        record_endings(args.endings_output)


if __name__ == "__main__":
    main()
