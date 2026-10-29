"""固定36CLIと8API processのpending通知を、本家だけで全field再検証する。"""

import argparse
import copy
import hashlib
import json
import os
import re
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/pending_cops.json"
DATA_SHA256 = "66df43c469d5387c53a7c183f903b655ee234417153dce0976ba186c975a8489"
INPUT_FIELDS = ("id", "files", "args", "env", "comparison", "directories")

SCHEMA_API = (
    LOADER
    + r"""
require 'json'
require 'digest'
config = RuboCop::ConfigLoader.default_configuration
before = Marshal.dump(config.to_h)
pending = config.pending_cops.select do |cop|
  RuboCop::Cop::Registry.qualified_cop?(cop.name)
end.map do |cop|
  {name: cop.name, enabled: cop.metadata['Enabled'], version_added: cop.metadata['VersionAdded']}
end
table = config.to_h.map do |name, metadata|
  {name: name, enabled: metadata['Enabled'], version_added: metadata['VersionAdded']}
end
spec = Gem.loaded_specs.fetch('rubocop')
gems = %w[parser prism rubocop-ast rubocop rainbow].to_h do |name|
  [name, Gem.loaded_specs.fetch(name).version.to_s]
end
sources = %w[
  config/default.yml lib/rubocop/config.rb
  lib/rubocop/pending_cops_reporter.rb lib/rubocop/cli.rb
  lib/rubocop/config_loader_resolver.rb lib/rubocop/cop/badge.rb lib/rubocop/config_validator.rb
].to_h do |file|
  [file, Digest::SHA256.file(File.join(spec.full_gem_path,file)).hexdigest]
end
puts JSON.generate({ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: gems, sources: sources,
  table: table, pending: pending, raw_unchanged: before == Marshal.dump(config.to_h)})
"""
)
VERSION_API = (
    LOADER
    + r"""
require 'json'
require 'digest'
values = [
  '1.85', '1.85.0', '1.85.1', '1.85.pre', '1.85.0.pre', '1.85-rc1', '1.85.a', '1.85A',
  '01.085', '999999999999999999999999999999.0', 'N/A', '<<next>>', nil, false, true, 1.85, 0, 1.0e-9
]
config = RuboCop::Config.new({})
rows = values.map do |value|
  parsed = config.send(:comparable_version, value)
  {value: value, ruby_class: value.class.name, ruby_to_s: value.to_s,
   display: value || 'N/A', parsed: parsed&.to_s,
   covered_by_1_85: parsed ? parsed <= Gem::Version.new('1.85') : false}
end
file = Gem::Version.instance_method(:initialize).source_location.first
puts JSON.generate({ruby: RUBY_VERSION, rubygems: Gem::VERSION,
  version_source_sha256: Digest::SHA256.file(file).hexdigest, rows: rows})
"""
)
QUALIFIED_API = (
    LOADER
    + r"""
require 'json'
require 'digest'
registry = RuboCop::Cop::Registry
names = registry.global.names
names += [
  'Layout/Unknown', 'Lint/DeprecatedReferences', 'LineLength',
  'Wrong/LineLength', 'layout/line_length'
]
file = File.join(Gem.loaded_specs.fetch('rubocop').full_gem_path, 'lib/rubocop/cop/registry.rb')
puts JSON.generate({rows: names.map { |name| [name, registry.qualified_cop?(name)] },
  source_sha256: Digest::SHA256.file(file).hexdigest})
"""
)
POLICY_API = (
    LOADER
    + r"""
require 'json'
config = RuboCop::ConfigLoader.merge_with_default(
  RuboCop::ConfigLoader.load_file('config.yml'), 'config.yml'
)
before = Marshal.dump(config.to_h)
pending = config.pending_cops.select do |cop|
  RuboCop::Cop::Registry.qualified_cop?(cop.name)
end.map do |cop|
  {name: cop.name, enabled: cop.metadata['Enabled'], version_added: cop.metadata['VersionAdded']}
end
puts JSON.generate({pending: pending, raw_unchanged: before == Marshal.dump(config.to_h)})
"""
)


def _same(actual, expected):
    if type(actual) is not type(expected):
        raise ValueError("参照の型が変わりました")
    if isinstance(actual, dict):
        if actual.keys() != expected.keys():
            raise ValueError("参照のfield集合が変わりました")
        for key in actual:
            _same(actual[key], expected[key])
    elif isinstance(actual, list):
        if len(actual) != len(expected):
            raise ValueError("参照の件数が変わりました")
        for left, right in zip(actual, expected, strict=True):
            _same(left, right)
    elif actual != expected:
        raise ValueError("参照値が変わりました")


def _fresh(work, output):
    for path in (work, output):
        if os.path.lexists(path):
            raise ValueError("保存先entryは既に存在します")
    first, second = (os.path.normcase(str(path.resolve(strict=False))) for path in (work, output))
    if (
        first == second
        or Path(first) in Path(second).parents
        or Path(second) in Path(first).parents
    ):
        raise ValueError("workとoutputの保存先が重なっています")


def _write_json(path, value):
    with path.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def _inventory(root, names):
    result = {}
    for name in names:
        path = root / name
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("原本が通常fileではありません")
        result[name] = {
            "bytes_hex": path.read_bytes().hex(),
            "mtime_ns": info.st_mtime_ns,
            "mode": info.st_mode,
        }
    return result


def _template():
    return json.loads(FIXTURE.read_bytes())


def child_environment(intentional=None):
    """ENV4とambient NO_COLORを子copyで除去し、GEM/PATH/OSは保つ。

    Returns:
        dict: TERM=dumbとcase固有の色/cacheを反映した環境。

    """
    child = reference_environment()
    for key in list(child):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            child.pop(key)
    child["TERM"] = "dumb"
    child.update(intentional or {})
    return child


def validate_fixture(value):
    """独立参照の全型・入力・rawと、有限のCLI/API範囲を固定する。

    Raises:
        ValueError: schema、版、出典、入力、完了件数又は元の出力が変わった場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != DATA_SHA256 or type(value.get("schema")) is not int or value["schema"] != 1:
        raise ValueError("固定参照の型・入力・rawが変わりました")
    _same(value["primary"]["gems"], VERSIONS)
    if (
        len(value["cases"]) != 36
        or len(value["primary"]["table"]) != 610
        or len(value["primary"]["pending"]) != 159
        or len(value["version_api"]["rows"]) != 18
        or len(value["qualified_api"]["rows"]) != 614
        or len(value["policy_api"]) != 5
    ):
        raise ValueError("実CLI/APIが未完了です")


def case_inputs():
    """固定した36入力を独立copyで返し、候補exeを受け取らない。

    Returns:
        list: 元の順序と入力byteを保持する辞書。

    """
    value = _template()
    validate_fixture(value)
    return [{key: copy.deepcopy(row[key]) for key in INPUT_FIELDS} for row in value["cases"]]


def pending_names(stderr):
    """実stderrの通知行を、ANSIと元の順序を保ったまま識別する。

    Returns:
        list: 通知に含まれたcop名。

    """
    return [
        match.decode()
        for match in re.findall(rb"(?:\x1b\[33m)?([^:\r\n\x1b]+): # new in [^\r\n\x1b]*", stderr)
    ]


def _output_state(root):
    path = root / "report.json"
    if not os.path.lexists(path):
        return {"exists": False}
    info = path.lstat()
    # 新規entryは種類だけを記録し、callerのsymlink/junctionの先を読まない。
    return {
        "exists": True,
        "mode": info.st_mode,
        "size": info.st_size,
        "mtime_ns": info.st_mtime_ns,
    }


def _prepare(root, files, directories=()):
    if os.path.lexists(root):
        raise ValueError("child directoryが既に存在します")
    root.mkdir()
    for name in directories:
        (root / name).mkdir()
    for name, text in files.items():
        (root / name).write_bytes(text.encode())


def _run(ruby, arguments, root, files, intentional=None):
    before = _inventory(root, files)
    output_before = _output_state(root)
    done = subprocess.run(
        [str(ruby), *arguments],
        cwd=root,
        env=child_environment({"XDG_CACHE_HOME": str(root / "cache"), **(intentional or {})}),
        capture_output=True,
        timeout=60,
        check=False,
    )
    after = _inventory(root, files)
    output_after = _output_state(root)
    _write_json(
        root / "private-run0.json",
        {
            "exit": done.returncode,
            "stdout_hex": done.stdout.hex(),
            "stderr_hex": done.stderr.hex(),
            "inputs_before": before,
            "inputs_after": after,
            "out_before": output_before,
            "out_after": output_after,
        },
    )
    _same(before, after)
    _same(output_before, {"exists": False})
    _same(output_after, {"exists": False})
    if done.returncode != 0:
        raise ValueError("本家processが正常完了しませんでした")
    return done


def _api(ruby, source, root, files, expected):
    _prepare(root, files)
    done = _run(ruby, ["-e", source], root, files)
    if done.stderr:
        raise ValueError("本家APIにstderrがありました")
    value = json.loads(done.stdout)
    _same(value, expected)
    return value


def _record(ruby, expected, launcher, root):
    _prepare(root, expected["files"], expected["directories"])
    done = _run(ruby, [str(launcher), *expected["args"]], root, expected["files"], expected["env"])
    row = copy.deepcopy(expected)
    row.update(
        exit=done.returncode,
        stdout_hex=done.stdout.hex(),
        stderr_hex=done.stderr.hex(),
        pending_names=pending_names(done.stderr),
        bytes_mtime_unchanged=True,
    )
    # 全streamを同時に比較し、色・footer・完了件数の同時改変も通さない。
    _same(row, expected)
    return row


def collect(ruby, work, output):
    """44個の実processと保持検査の成功後だけ、公開先をexclusive作成する。

    Raises:
        ValueError: 保存先、参照、実process、原本又は出力の契約が変わった場合。

    """
    _fresh(work, output)
    if not output.parent.is_dir():
        raise ValueError("出力先の親directoryがありません")
    value = _template()
    validate_fixture(value)
    work.mkdir(parents=True)
    value["primary"] = _api(ruby, SCHEMA_API, work / "api-schema", {}, value["primary"])
    value["version_api"] = _api(ruby, VERSION_API, work / "api-version", {}, value["version_api"])
    value["qualified_api"] = _api(
        ruby, QUALIFIED_API, work / "api-qualified", {}, value["qualified_api"]
    )
    policy = []
    for row in value["policy_api"]:
        case = next(case for case in value["cases"] if case["id"] == row["id"])
        files = {name: case["files"][name] for name in ("Gemfile", "config.yml")}
        actual = _api(ruby, POLICY_API, work / ("api-" + row["id"]), files, row["values"])
        policy.append({"id": row["id"], "values": actual})
    value["policy_api"] = policy
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n')
    value["cases"] = [
        _record(ruby, row, launcher.absolute(), work / row["id"]) for row in value["cases"]
    ]
    validate_fixture(value)
    _write_json(output, value)


def main():
    """固定版の本家だけを実行し、既存entryを上書きしない。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
