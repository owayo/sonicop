"""cache-root callbackの42実CLIと出典を、全件照合後だけ再保存する。"""

import argparse
import hashlib
import json
import os
from pathlib import Path

from record_cache_parallel_preflight import _fresh, _inventory, _same, _write_json
from record_config_cop_namespaces import LOADER
from record_remote_config_origins import _marker
from record_target_ruby_environment import _capture
from record_target_ruby_validation import prepare

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/cache_root_option_order.json"
DATA_SHA256 = "236f1d51c4cb715e3ca611ad580d8286bf249e7ea209284df4cd9eabd605dbc9"
API = (
    LOADER
    + "\nrequire 'json'; require 'digest'\nroot = Gem.loaded_specs.fetch('rubocop').full_gem_path\nsources = %w[lib/rubocop/options.rb lib/rubocop/cli.rb lib/rubocop/config_loader.rb lib/rubocop/cache_config.rb LICENSE.txt].to_h do |p|\n  [p, Digest::SHA256.file(File.join(root,p)).hexdigest]\nend\nruntime = {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,\n  gems: %w[psych parser prism rubocop-ast rubocop rainbow].to_h {|p| [p, Gem.loaded_specs.fetch(p).version.to_s]}}\nprint JSON.generate(runtime: runtime, source_sha256: sources)\n"
)

CLI = LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n"


def validate(value):
    """入力・順序・原本・全rawと比較範囲の削減を拒否する。

    Raises:
        ValueError: 固定した実本家の参照と異なる場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定参照の入力・型・raw・scopeが変わりました")
    _same(len(value["cases"]), 42)


def template():
    """候補exeを使わず、実本家から採取した42条件を読む。

    Returns:
        dict: 固定した出典・入力・全rawと比較範囲。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def record_case(ruby, root, row):
    """一件の全raw、marker、cache rootと原本を照合する。"""
    files = row["files"]
    prepare(root, files)
    before = _inventory(root, files)
    _write_json(root / "private-source-before.json", before)
    # -aのa.rbだけは実補正を許し、それ以外は共通_captureがbytes/mtime/modeを保持する。
    protected = {
        name: body
        for name, body in files.items()
        if not (name == "a.rb" and row["corrected_source_hex"] is not None)
    }
    done, report_before, report_after = _capture(
        ruby, ["-e", CLI, "--", *row["args"]], root, protected, row["environment"]
    )
    after = _inventory(root, files)
    _write_json(root / "private-source-after.json", after)
    _same(report_before, {})
    _same(report_after, {})
    _same(done.returncode, row["exit"])
    _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
    _same(done.stderr, bytes.fromhex(row["stderr_hex"]))
    for name in files:
        _same(after[name]["mode"], before[name]["mode"])
        if name == "a.rb" and row["corrected_source_hex"] is not None:
            _same(after[name]["bytes_hex"], row["corrected_source_hex"])
        else:
            _same(after[name], before[name])
    marker = _marker(root)
    _same(None if marker is None else marker["bytes_hex"], row["marker_hex"])
    _same(
        {name: (root / name).is_dir() for name in row["cache_roots_present"]},
        row["cache_roots_present"],
    )


def collect(ruby, work, output):
    """43実processの出典・全出力・原本が一致した後だけexclusive保存する。

    Returns:
        dict: 全42 CLIと出典を再検証した参照。

    Raises:
        ValueError: 保存先、実環境、属性、本文又は全rawが異なる場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windowsの物理stream専用です")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    root = work / "source-api"
    prepare(root, {"Gemfile": ""})
    done, before, after = _capture(ruby, ["-e", API], root, {"Gemfile": ""}, {})
    _same(done.returncode, 0)
    _same(done.stderr, b"")
    _same(before, {})
    _same(after, {})
    _same(done.stdout, bytes.fromhex(value["api_stdout_hex"]))
    _same(
        json.loads(done.stdout),
        {"runtime": value["runtime"], "source_sha256": value["source_sha256"]},
    )
    for row in value["cases"]:
        record_case(ruby, work / row["id"], row)
    validate(value)
    _write_json(output, value)
    return value


def main():
    """候補を受け取らず、新規private保存先へ実本家を採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
