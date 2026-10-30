"""解決後検証とparser拒否の51実CLI・9実getterを候補なしで再採取する。"""

import argparse
import hashlib
import json
import os
from pathlib import Path

from record_cache_parallel_preflight import _fresh, _same, _write_json
from record_config_cop_namespaces import LOADER
from record_remote_config_origins import _prepare
from record_target_ruby_environment import _capture, bind
from record_verbatim_config_origins import _windows_name

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/target_ruby_validation.json"
DATA_SHA256 = "c7c7bc4dc31c0431b57eddf8e89a67030ef1cadbe69ce7ebc1b7e0daa42d96aa"
CLI = LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n"
API = (
    LOADER
    + r"""
require 'json'; require 'digest'
roots = {
  'RUBOCOP_ROOT' => Gem.loaded_specs.fetch('rubocop').full_gem_path.tr('\\','/'),
  'AST_ROOT' => Gem.loaded_specs.fetch('rubocop-ast').full_gem_path.tr('\\','/')
}
rows = JSON.parse(STDIN.read).map do |row|
  Dir.chdir(row.fetch('root')) do
    config = RuboCop::ConfigLoader.load_file('.rubocop.yml')
    version = config.target_ruby_version
    {id: row.fetch('id'), getter_class: version.class.name,
     getter_inspect: version.inspect, parser_engine: config.parser_engine.to_s}
  end
end
names = %w[lib/rubocop/target_ruby.rb lib/rubocop/config.rb lib/rubocop/config_validator.rb
  lib/rubocop/config_loader.rb lib/rubocop/config_store.rb lib/rubocop/cli.rb LICENSE.txt]
sources = names.to_h do |p|
  [p, Digest::SHA256.file(File.join(roots.fetch('RUBOCOP_ROOT'), p)).hexdigest]
end
sources['rubocop-ast/processed_source.rb'] = Digest::SHA256.file(
  File.join(roots.fetch('AST_ROOT'), 'lib/rubocop/ast/processed_source.rb')).hexdigest
runtime = {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[psych parser prism rubocop-ast rubocop rainbow].to_h do |p|
    [p, Gem.loaded_specs.fetch(p).version.to_s]
  end}
print JSON.generate(runtime: runtime, source_sha256: sources, roots: roots, rows: rows)
"""
)


def validate(value):
    """全入力・実raw・型・比較範囲を固定する。

    Raises:
        ValueError: 固定した実本家参照と異なる場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定参照の入力・型・raw・scopeが変わりました")
    _same(len(value["cases"]), 51)
    _same(len(value["api"]), 9)


def template():
    """実本家で採取した全条件を読む。

    Returns:
        dict: 固定した比較範囲と実raw。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def prepare(root, files):
    """新規caseの入れ子設定もexclusiveに用意する。"""
    _prepare(root, {})
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(body.encode())


def collect(ruby, work, output):
    """52実processの全出力・出典・原本を照合した後だけ新規保存する。

    Returns:
        dict: 全件を再採取して一致した参照。

    Raises:
        ValueError: 保存先、実行環境、原本又は出力の照合に失敗した場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windows参照専用。非Windows実走へ投影しません")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    root = work / "api"
    files = {"Gemfile": ""}
    _prepare(root, files)
    forms, expected = [], []
    for row in value["api"]:
        local = {
            "Gemfile": "",
            ".rubocop.yml": "AllCops:\n  NewCops: disable\n  TargetRubyVersion: "
            + row["scalar_yaml"]
            + "\n",
        }
        _prepare(root / row["id"], local)
        files.update({row["id"] + "/" + name: body for name, body in local.items()})
        forms.append({"id": row["id"], "root": _windows_name(root / row["id"])})
        expected.append({key: val for key, val in row.items() if key != "scalar_yaml"})
    done, before, after = _capture(
        ruby, ["-e", API], Path(_windows_name(root)), files, {}, json.dumps(forms).encode()
    )
    _same(done.returncode, 0)
    _same(done.stderr, b"")
    _same(before, {})
    _same(after, {})
    observed = json.loads(done.stdout)
    roots = observed["roots"]
    _same(set(roots), {"RUBOCOP_ROOT", "AST_ROOT"})
    _same(
        observed,
        {
            "runtime": value["runtime"],
            "source_sha256": value["source_sha256"],
            "roots": roots,
            "rows": expected,
        },
    )
    for row in value["cases"]:
        root = work / row["id"]
        prepare(root, row["files"])
        cwd = _windows_name(root)
        originals = {key: val for key, val in row["files"].items() if key != "report.json"}
        done, before, after = _capture(
            ruby,
            ["-e", CLI, "--", *row["args"]],
            Path(cwd),
            originals,
            row["environment"],
            bytes.fromhex(row["stdin_hex"]),
        )
        _same(done.returncode, row["exit"])
        _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
        _same(
            done.stderr,
            bind(
                row["stderr_template"],
                {
                    "CWD": cwd,
                    "CWD_FWD": cwd.replace("\\", "/"),
                    **roots,
                },
            ).encode(),
        )
        _same(before, after)
        _same(bool(after), row["report_present"])
    validate(value)
    _write_json(output, value)
    return value


def main():
    """固定版Rubyだけを新規workで実行する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
