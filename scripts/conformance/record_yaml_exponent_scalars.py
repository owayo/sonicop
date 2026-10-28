"""Psychの指数scalarと通常copでの影響を、有限の実入力から記録する。"""

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

BASE = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
LOAD = (
    "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; "
    "gem 'rainbow','3.1.1'; gem 'rubocop','1.89.0'; require 'rubocop'; require 'json'; "
)
ENV_KEYS = {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
SCALARS = [
    ("plain-exponent", "1e3"),
    ("positive-capital-exponent", "+1E3"),
    ("negative-exponent", "-1e-3"),
    ("positive-exponent-sign", "1e+3"),
    ("decimal-unsigned-exponent", "1.0e3"),
    ("decimal-signed-exponent", "1.0e+3"),
    ("fraction-unsigned-exponent", ".5e3"),
    ("fraction-capital-signed-exponent", ".5E+3"),
    ("trailing-dot-unsigned-exponent", "1.e3"),
    ("trailing-dot-signed-exponent", "1.e+3"),
    ("single-quoted", "'1e3'"),
    ("double-quoted", '"1e3"'),
    ("explicit-string", "!!str 1e3"),
    ("explicit-float", "!!float 1e3"),
    ("explicit-quoted-float", '!!float "1e3"'),
    ("explicit-string-signed-decimal", "!!str 1.0e+3"),
    ("ordinary-integer", "1000"),
    ("ordinary-decimal", "1.25"),
    ("ordinary-negative-decimal", "-0.5"),
]
TYPE_CASES = [
    {
        "id": name,
        "config": BASE + "Lint/BigDecimalNew:\n  Description: " + scalar + "\n",
        "keys": ["Description"],
    }
    for name, scalar in SCALARS
] + [
    {
        "id": "unicode-byte-offset",
        "config": BASE
        + "# 日本語の前置き\nLint/BigDecimalNew:\n  StyleGuide: 日本語\n  Description: 1e3\n",
        "keys": ["Description", "StyleGuide"],
    },
    {
        "id": "anchor-alias",
        "config": BASE + "Lint/BigDecimalNew:\n  Description: &number 1e3\n  StyleGuide: *number\n",
        "keys": ["Description", "StyleGuide"],
    },
    {
        "id": "directive-string",
        "config": "%TAG !kind! tag:yaml.org,2002:\n---\n"
        + BASE
        + "Lint/BigDecimalNew:\n  Description: !kind!str 1e3\n",
        "keys": ["Description"],
    },
    {
        "id": "directive-float",
        "config": "%TAG !kind! tag:yaml.org,2002:\n---\n"
        + BASE
        + "Lint/BigDecimalNew:\n  Description: !kind!float 1e3\n",
        "keys": ["Description"],
    },
]
BEHAVIORS = [
    ("allowed-plain", "1e3", "value = '1e3 and padding'\n"),
    ("allowed-quoted", "'1e3'", "value = '1e3 and padding'\n"),
    ("allowed-decimal-unsigned", "1.0e3", "value = '1.0e3 and padding'\n"),
]


def child_environment(parent=None):
    """親とGEM/PATH/OS値を保ち、4変数だけを子copyから落とす。

    Returns:
        dict: Rubyへの注入変数を除いた子プロセス用の環境。

    """
    environment = dict(os.environ if parent is None else parent)
    for key in list(environment):
        if (key.upper() if os.name == "nt" else key) in ENV_KEYS:
            environment.pop(key)
    return environment


def run(ruby, code, args, cwd):
    """親のRubyオプションを除き、指定版の本家だけを別プロセスで起動する。

    Returns:
        subprocess.CompletedProcess: 本家の終了値と未変換のstdout・stderr。

    """
    return subprocess.run(
        [str(ruby), "-e", LOAD + code, "--", *args],
        cwd=cwd,
        env=child_environment(),
        capture_output=True,
        check=False,
    )


def collect(ruby, selected=None):
    """明示した入力だけを一時領域へ作り、ConfigLoaderと実CLIを実行する。

    Returns:
        dict: 実行環境、未変換の出力、設定値の型と診断の参照記録。

    Raises:
        ValueError: 未知のケース名が指定された場合。
        RuntimeError: 本家の採取が失敗したか、入力ファイルが変わった場合。

    """
    known = {case["id"] for case in TYPE_CASES} | {name for name, _, _ in BEHAVIORS}
    unknown = set(selected or ()) - known
    if unknown:
        raise ValueError("unknown case(s): " + ", ".join(sorted(unknown)))
    with tempfile.TemporaryDirectory(prefix="yaml-exponent-") as folder:
        root = Path(folder)
        chosen = [case for case in TYPE_CASES if not selected or case["id"] in selected]
        inputs = []
        for index, case in enumerate(chosen):
            path = root / f"type-{index}.yml"
            path.write_text(case["config"], encoding="utf-8", newline="\n")
            inputs.append({"id": case["id"], "path": path.name, "keys": case["keys"]})
        (root / "inputs.json").write_text(json.dumps(inputs), encoding="utf-8", newline="\n")
        code = (
            "inputs=JSON.parse(File.read(ARGV[0])); rows=inputs.map do |item|; "
            "config=RuboCop::ConfigLoader.load_file(item.fetch('path')); "
            "values=item.fetch('keys').to_h do |key|; value=config.for_cop('Lint/BigDecimalNew').fetch(key); "
            "[key,{type:value.class.name,value:value}]; end; {id:item.fetch('id'),values:values}; end; "
            "puts JSON.generate({runtime:{ruby:RUBY_VERSION,psych:Psych::VERSION,gems:Gem.loaded_specs.transform_values{|s|s.version.to_s}},cases:rows})"
        )
        types = run(ruby, code, ["inputs.json"], root)
        if types.returncode != 0 or types.stderr:
            raise RuntimeError(
                "ConfigLoader採取失敗: " + types.stderr.decode("utf-8", errors="replace")
            )
        parsed = json.loads(types.stdout)
        typed = [
            dict(case, expected=row["values"])
            for case, row in zip(chosen, parsed["cases"], strict=True)
        ]
        for index, case in enumerate(chosen):
            if (root / f"type-{index}.yml").read_bytes() != case["config"].encode("utf-8"):
                raise RuntimeError("本家が設定を変更した")
        behaviors = []
        for name, scalar, source in BEHAVIORS:
            if selected and name not in selected:
                continue
            config = (
                BASE + "Layout/LineLength:\n  Max: 10\n  AllowedPatterns:\n    - " + scalar + "\n"
            )
            (root / "config.yml").write_text(config, encoding="utf-8", newline="\n")
            (root / "case.rb").write_text(source, encoding="utf-8", newline="\n")
            args = [
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--only",
                "Layout/LineLength",
                "--format",
                "json",
                "case.rb",
            ]
            actual = run(ruby, "load Gem.bin_path('rubocop','rubocop','1.89.0')", args, root)
            if actual.returncode not in [0, 1] or actual.stderr:
                raise RuntimeError(
                    "cop採取失敗: " + actual.stderr.decode("utf-8", errors="replace")
                )
            report = json.loads(actual.stdout)
            if (root / "config.yml").read_bytes() != config.encode() or (
                root / "case.rb"
            ).read_bytes() != source.encode():
                raise RuntimeError("本家が原本を変更した")
            behaviors.append(
                {
                    "id": name,
                    "config": config,
                    "source": source,
                    "args": args,
                    "exit": actual.returncode,
                    "stdout_hex": actual.stdout.hex(),
                    "stderr_hex": actual.stderr.hex(),
                    "report": {key: report[key] for key in ["files", "summary"]},
                }
            )
        return {
            "schema": 1,
            "runtime": parsed["runtime"],
            "ruby_binary_sha256": hashlib.sha256(ruby.read_bytes()).hexdigest(),
            "type_probe": {
                "exit": types.returncode,
                "stdout_hex": types.stdout.hex(),
                "stderr_hex": types.stderr.hex(),
            },
            "types": typed,
            "behaviors": behaviors,
        }


def main():
    """採取が完了した結果だけを書き出し、不完全な参照記録を残さない。"""
    parser = argparse.ArgumentParser()
    parser.add_argument("--ruby", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--case", action="append")
    args = parser.parse_args()
    result = collect(args.ruby, set(args.case or []))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    print(f"recorded {len(result['types'])} type cases / {len(result['behaviors'])} cop cases")


if __name__ == "__main__":
    main()
