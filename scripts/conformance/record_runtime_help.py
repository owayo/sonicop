"""固定本家だけでhelp前のrequireと終了処理の二十五CLIを再採取する。"""

import argparse
import hashlib
import json
import os
from pathlib import Path

from record_cache_parallel_preflight import _fresh, _same, _write_json
from record_remote_config_origins import _prepare
from record_runtime_requires import CLI, PINNED_LOADER
from record_target_ruby_environment import _capture

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/runtime_help.json"
DATA_SHA256 = "b974aa5b8bafe608d8ede9357a7192cc08eef920bfbacf200c027340a8fd6ffa"
API = (
    PINNED_LOADER
    + """
require 'json'; require 'digest'; require 'rbconfig'
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
library = RbConfig::CONFIG.fetch('rubylibdir')
puts JSON.generate(runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
 gems: %w[psych parser prism rubocop-ast rubocop rainbow lint_roller].to_h {|n|
 [n, Gem.loaded_specs.fetch(n).version.to_s]}},
 source_sha256: %w[cli.rb options.rb feature_loader.rb arguments_env.rb arguments_file.rb].to_h {|n|
 [n, Digest::SHA256.file(File.join(root,'lib/rubocop',n)).hexdigest]}.merge(
 'ruby/optparse.rb' => Digest::SHA256.file(File.join(library,'optparse.rb')).hexdigest),
 roots: {RUBOCOP_ROOT: root.tr('\\\\','/'), RUBY_LIB: library.tr('\\\\','/')})
"""
)


def validate(value):
    """全入力・型・本家help本文・全raw・出典・比較範囲の欠落を拒否する。

    Raises:
        ValueError: 固定した実本家の参照が変わった場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定した実参照の入力・全raw・scopeが変わりました")
    _same(len(value["cases"]), 25)


def template():
    """実本家から保存した全参照を、候補を呼ばずに読み込む。

    Returns:
        dict: 実参照の完全な入力・出力と出典。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def stdout(value, row):
    """共通のhelp本文を展開し、実参照のstdout全bytesを復元する。

    Returns:
        bytes: prefix、help本文、suffixを順序通りに結合したstdout。

    """
    return (
        bytes.fromhex(row["stdout_prefix_hex"])
        + (bytes.fromhex(value["reference_help_hex"]) if row["help"] else b"")
        + bytes.fromhex(row["stdout_suffix_hex"])
    )


def collect(ruby, work, output):
    """版と出典、二十五CLIの全raw、原本とreportを照合してから新規保存する。

    Returns:
        dict: 検証した固定参照。

    Raises:
        ValueError: 保存先、版、出典、出力又は原本の保持を満たさない場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windows参照の再採取専用です")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    root = work / "runtime"
    files = {"Gemfile": "", "reference.rb": API}
    _prepare(root, files)
    done, before, after = _capture(ruby, [str(root / "reference.rb")], root, files, {})
    _same((done.returncode, done.stderr, before, after), (0, b"", {}, {}))
    observed = json.loads(done.stdout)
    roots = observed.pop("roots")
    _same(set(roots), {"RUBOCOP_ROOT", "RUBY_LIB"})
    _same(observed, value["primary"])
    for row in value["cases"]:
        root = work / row["id"]
        _prepare(root, row["files"])
        done, before, after = _capture(
            ruby, ["-e", CLI, "--", *row["args"]], root, row["files"], row["environment"]
        )
        _same(before, after)
        _same(done.returncode, row["exit"])
        _same(done.stdout, stdout(value, row))
        errors = row["stderr_template"]
        for name, actual in roots.items():
            errors = errors.replace(f"<{name}>", actual)
        _same(done.stderr, errors.encode())
    validate(value)
    _write_json(output, value)
    return value


def main():
    """固定版の本家Rubyだけを実行し、候補exeの指定は受け付けない。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
