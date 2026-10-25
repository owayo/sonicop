#!/usr/bin/env python3
"""本家の末尾 token 行を、EndOfLine の EOF 条件から独立して記録する。"""

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

ENV_KEYS = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
GROUPS = (
    ("2.7", "parser_whitequark"),
    ("3.3", "parser_whitequark"),
    ("3.4", "parser_whitequark"),
    ("3.3", "parser_prism"),
    ("3.4", "parser_prism"),
    ("4.0", "parser_prism"),
    ("4.1", "parser_prism"),
)
PROBE = r"""
gem 'parser', '3.3.12.0'
gem 'prism', '1.8.1'
gem 'rubocop', '1.89.0'
require 'rubocop'
require 'json'
require 'digest'
input = JSON.parse(File.read(ARGV.fetch(0)))
groups = input.fetch('groups').map do |target, engine|
  records = input.fetch('cases').filter_map do |name, hex|
    # 既存 SOH 文言差は末尾 token の同値性へ混ぜず、Prism 側は記録する。
    next if engine == 'parser_whitequark' && name.start_with?('soh_code-')
    bytes = [hex].pack('H*')
    source = RuboCop::ProcessedSource.new(
      bytes, target.to_f, name, parser_engine: engine.to_sym
    )
    {
      name: name, source_sha256: Digest::SHA256.hexdigest(bytes),
      valid_syntax: source.valid_syntax?,
      last_line: source.tokens.last&.line,
      tokens: source.tokens.map { |token| [token.type, token.line, token.text] },
      diagnostics: source.diagnostics.map do |diagnostic|
        [diagnostic.level, diagnostic.message]
      end
    }
  end
  { target: target, engine: engine, records: records }
end
puts JSON.generate(
  ruby_version: RUBY_VERSION, ruby_platform: RUBY_PLATFORM,
  gem_versions: %w[rubocop prism parser rubocop-ast].to_h do |name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]
  end,
  groups: groups
)
"""


def _cases(base):
    cases = dict(base)
    block = "=begin\nkept\n=end\n"
    additions = {
        "block_then_lines": "a=1\n" + block + "# first\n# kept",
        "line_block_line": "a=1\n# first\n" + block + "# kept",
        "inline_block_line": "a=1 # inline\n" + block + "# kept",
        "block_blank_line": "a=1\n" + block + "\n# kept",
        "block_nul_code": "a=1\n" + block + "\0ignored\n# kept",
        "block_nul_comment": "a=1\n" + block + "# kept\0",
        "block_nul_data": "a=1\n" + block + "\0ignored\n__END__\nbody",
        "block_comment_data": "a=1\n" + block + "# kept\n__END__\nbody",
        "block_semicolon": "a=1\n" + block + ";\n# kept",
        "block_code_line": "a=1\n" + block + "b=2\n# kept",
        "block_unicode": "猫=1\n=begin\n日本語\n=end\n# 日本語",
        "block_only_line": block + "# kept",
        "two_blocks_line": "a=1\n" + block + block + "# kept",
        "unterminated_block": "a=1\n=begin\nkept\n# kept",
        "continued_block": "a=1\\\n" + block + "# kept",
        "blank_before_comment": "a=1\n \t\n# kept",
        "blank_between_comments": "a=1\n# first\n\n# kept\n",
        "continued_blank": "a=1\n\\\n# kept",
        "two_continuations": "a=1\\\n\\\n# kept",
        "heredoc_empty_body": "a=<<A\n\nA\n# kept",
        "heredoc_empty_delimiter": "a=<<''\nkept\n\n# kept",
        "heredoc_blank_after_end": "a=<<A\nkept\nA\n\n# kept",
        "heredoc_comment_blank": "a=<<A\nkept\nA\n# first\n\n# kept",
        "heredoc_chain_blank": "a=(<<A\nkept\nA\n).upcase\n\n# kept",
        "blank_nul_comment": "a=1\n\n# kept\0",
        "blank_nul_code": "a=1\n\n\0ignored\n# kept",
    }
    for name, text in additions.items():
        for ending in ("lf", "crlf"):
            source = text.replace("\n", "\r\n" if ending == "crlf" else "\n")
            cases[name + "-" + ending + ".rb"] = source.encode().hex()
    return cases


def _record_cli(ruby, root, cases, env):
    root.mkdir(parents=True, exist_ok=True)
    wrapper = root / "reference.rb"
    wrapper.write_text(
        "gem 'parser', '3.3.12.0'\ngem 'prism', '1.8.1'\n"
        "gem 'rubocop', '1.89.0'\n"
        "load Gem.bin_path('rubocop', 'rubocop', '1.89.0')\n",
        encoding="utf-8",
        newline="\n",
    )
    records = []
    for target, engine in (GROUPS[0], GROUPS[4]):
        names = sorted(
            name
            for name in cases
            if engine != "parser_whitequark" or not name.startswith("soh_code-")
        )
        for style in ("lf", "crlf", "native"):
            directory = root / f"{target}-{engine}-{style}"
            directory.mkdir(exist_ok=True)
            config = (
                f"AllCops:\n  NewCops: disable\n  TargetRubyVersion: {target}\n"
                f"  ParserEngine: {engine}\nLayout/EndOfLine:\n"
                f"  EnforcedStyle: {style}\n"
            )
            (directory / ".rubocop.yml").write_text(
                config, encoding="utf-8", newline="\n"
            )
            for name in names:
                (directory / name).write_bytes(bytes.fromhex(cases[name]))
            for mode in ("inspect", "a", "A"):
                args = [
                    "--config",
                    ".rubocop.yml",
                    "--cache",
                    "false",
                    "--only",
                    "Layout/EndOfLine",
                    "--format",
                    "json",
                ]
                if mode != "inspect":
                    args.append("-" + mode)
                result = subprocess.run(
                    [ruby, str(wrapper), *args, *names],
                    cwd=directory,
                    env=env,
                    capture_output=True,
                    check=False,
                    timeout=180,
                )
                prefix = root / f"{target}-{engine}-{style}-{mode}"
                stdout_path = Path(str(prefix) + ".stdout")
                stderr_path = Path(str(prefix) + ".stderr")
                stdout_path.write_bytes(result.stdout)
                stderr_path.write_bytes(result.stderr)
                assert result.returncode in (0, 1), result.stderr
                assert not result.stderr, result.stderr
                report = json.loads(result.stdout)
                assert report["metadata"]["rubocop_version"] == "1.89.0"
                assert report["summary"]["target_file_count"] == len(names)
                assert report["summary"]["inspected_file_count"] == len(names)
                assert [file["path"] for file in report["files"]] == names
                assert report["summary"]["offense_count"] == sum(
                    len(file["offenses"]) for file in report["files"]
                )
                assert all(
                    (directory / name).read_bytes() == bytes.fromhex(cases[name])
                    for name in names
                )
                records.append(
                    {
                        "target": target,
                        "engine": engine,
                        "style": style,
                        "mode": mode,
                        "exit": result.returncode,
                        "args": args + names,
                        "directory": str(directory),
                        "stdout": str(stdout_path),
                        "stderr": str(stderr_path),
                        "unchanged_source_bytes": True,
                    }
                )
    (root / "records.json").write_text(
        json.dumps(records, indent=2) + "\n", encoding="utf-8", newline="\n"
    )


def main():
    """版・環境・原本 hash と実 token 列を保存し、候補の値を期待値へ使わない。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--cli-records", type=Path)
    args = parser.parse_args()
    base_path = (
        Path(__file__).resolve().parents[1] / "tests/fixtures/end_of_line_tokens.json"
    )
    base = json.loads(base_path.read_text(encoding="utf-8"))
    env = dict(os.environ)
    for key in ENV_KEYS:
        env.pop(key, None)
    cases = _cases(base["cases"])
    with tempfile.TemporaryDirectory(prefix="sonicop-last-token-") as directory:
        root = Path(directory)
        probe = root / "probe.rb"
        probe.write_text(PROBE, encoding="utf-8", newline="\n")
        inputs = root / "input.json"
        inputs.write_text(
            json.dumps({"cases": cases, "groups": GROUPS}),
            encoding="utf-8",
            newline="\n",
        )
        result = subprocess.run(
            [args.ruby, str(probe), str(inputs)],
            capture_output=True,
            env=env,
            check=False,
            timeout=180,
        )
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        assert not result.stderr, result.stderr.decode(errors="replace")
        report = json.loads(result.stdout)
    assert report["gem_versions"] == {
        "rubocop": "1.89.0",
        "prism": "1.8.1",
        "parser": "3.3.12.0",
        "rubocop-ast": "1.50.0",
    }
    assert len(base["cases"]) == 92
    for group in report["groups"]:
        expected = len(cases) - (2 if group["engine"] == "parser_whitequark" else 0)
        assert len(group["records"]) == expected
        assert len({record["name"] for record in group["records"]}) == expected
        for record in group["records"]:
            source = bytes.fromhex(cases[record["name"]])
            assert record["source_sha256"] == hashlib.sha256(source).hexdigest()
    report.update(
        cases=cases,
        base_fixture_sha256=hashlib.sha256(base_path.read_bytes()).hexdigest(),
        environment={key: None for key in ENV_KEYS},
    )
    args.output.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    if args.cli_records:
        _record_cli(args.ruby, args.cli_records.resolve(), cases, env)


if __name__ == "__main__":
    main()
