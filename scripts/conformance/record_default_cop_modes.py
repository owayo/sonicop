"""既定cop modeの63CLI/15APIを、候補を起動せず固定RuboCopで記録する。"""

import argparse
import copy
import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

ROOT = Path(__file__).resolve().parents[2]
GROUPS = ("modes", "queries", "lists")
CONFIG_SOURCE = "f782fd539108614e655d15f5eb07f19737c5479de6d56b8e8be18da9d7c07729"
RUNTIME = {
    "ruby": "4.0.7",
    "revision": "229531a6cfbf07e3caef30dbac24a2a3f3fed482",
    "platform": "x64-mingw-ucrt",
    "gems": {
        "parser": "3.3.12.0",
        "prism": "1.8.1",
        "rubocop-ast": "1.50.0",
        "rubocop": "1.89.0",
        "rainbow": "3.1.1",
    },
    "source_sha256": {
        "options.rb": "adfe551344fc98ea2d13f21852b56ff1cd445a6fdf36cd0bde8cd2a154a4d8c4",
        "config_loader_resolver.rb": "f8c40f5e08ad81ba25b76b8296e93ebd766af12730ab4d2bea770710d963c485",
        "cop/registry.rb": "41af570b3bbf8f3d28778e18be26d69130f5d23006327783d4b22e57e684fff7",
    },
}
IDS = {
    "modes": [
        "disabled-true-none",
        "disabled-true-disable",
        "disabled-true-enable",
        "disabled-false-none",
        "disabled-false-disable",
        "disabled-false-enable",
        "disabled-null-none",
        "disabled-null-disable",
        "disabled-null-enable",
        "disabled-pending-none",
        "disabled-pending-disable",
        "disabled-pending-enable",
        "disabled-empty-none",
        "disabled-empty-disable",
        "disabled-empty-enable",
        "disabled-absent-none",
        "disabled-absent-disable",
        "disabled-absent-enable",
        "enabled-true-none",
        "enabled-true-disable",
        "enabled-true-enable",
        "enabled-false-none",
        "enabled-false-disable",
        "enabled-false-enable",
        "enabled-empty-none",
        "enabled-empty-disable",
        "enabled-empty-enable",
        "enabled-absent-none",
        "enabled-absent-disable",
        "enabled-absent-enable",
        "normal-true-disable",
        "normal-false-disable",
        "normal-absent-disable",
        "department-disabled-explicit-true",
        "department-enabled-empty",
        "department-enabled-absent",
        "inherit-true",
        "inherit-false",
        "inherit-null",
        "inherit-empty",
        "only-disabled",
        "only-department-disabled",
        "except-enabled",
        "pending-enabled",
        "pending-disabled",
    ],
    "queries": [
        "ordinary",
        "enable-default",
        "enable-except-neighbor",
        "disable-only-neighbor",
        "pending-none",
        "null-none",
        "false-none",
        "pending-enable",
        "null-enable",
        "pending-disable",
        "null-disable",
        "typed-absent-none",
        "typed-pending-none",
        "typed-null-none",
        "typed-absent-enable",
    ],
    "lists": ["list-default-only", "list-enable-only-except", "list-disable-only"],
}
FORM_HASHES = {
    "modes": "861b1623e82c90564325bf9552dedee2b30abb506284fddb896f7510625b737d",
    "queries": "85e896bf6e73ee0b1b300c4c002e21d173053f5c1b0a632e32e489c835969cb0",
    "lists": "2bb5d264d55f51aad68e1400e6e5569abba676bcec72fea90da8f310ba1b041e",
}
QUERIES = [
    {
        "id": "ordinary",
        "query": "Style/UnlessElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "enable-default",
        "query": "Style/UnlessElse",
        "value": True,
        "ruby_class": "TrueClass",
        "truthy": True,
    },
    {
        "id": "enable-except-neighbor",
        "query": "Style/UnlessElse",
        "value": True,
        "ruby_class": "TrueClass",
        "truthy": True,
    },
    {
        "id": "disable-only-neighbor",
        "query": "Style/UnlessElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "pending-none",
        "query": "Style/UnlessElse",
        "value": "pending",
        "ruby_class": "String",
        "truthy": True,
    },
    {
        "id": "null-none",
        "query": "Style/UnlessElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "false-none",
        "query": "Style/UnlessElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "pending-enable",
        "query": "Style/UnlessElse",
        "value": "pending",
        "ruby_class": "String",
        "truthy": True,
    },
    {
        "id": "null-enable",
        "query": "Style/UnlessElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "pending-disable",
        "query": "Style/UnlessElse",
        "value": "pending",
        "ruby_class": "String",
        "truthy": True,
    },
    {
        "id": "null-disable",
        "query": "Style/UnlessElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "typed-absent-none",
        "query": "Style/MissingElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "typed-pending-none",
        "query": "Style/MissingElse",
        "value": "pending",
        "ruby_class": "String",
        "truthy": True,
    },
    {
        "id": "typed-null-none",
        "query": "Style/MissingElse",
        "value": False,
        "ruby_class": "FalseClass",
        "truthy": False,
    },
    {
        "id": "typed-absent-enable",
        "query": "Style/MissingElse",
        "value": True,
        "ruby_class": "TrueClass",
        "truthy": True,
    },
]

METADATA = (
    LOADER
    + """
require 'json'; require 'digest'
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
print JSON.generate({runtime: {
 ruby: RUBY_VERSION, revision: RUBY_REVISION, platform: RUBY_PLATFORM,
 gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] },
 source_sha256: %w[options.rb config_loader_resolver.rb cop/registry.rb].to_h { |name| [name, Digest::SHA256.file(File.join(root,'lib/rubocop',name)).hexdigest] }
}, config_source_sha256: Digest::SHA256.file(File.join(root,'lib/rubocop/config.rb')).hexdigest})
"""
)
QUERY_API = (
    LOADER
    + """
require 'json'
rows = JSON.parse(File.binread(ARGV.fetch(0))).map do |row|
  Dir.chdir(row.fetch('cwd')) do
    options, = RuboCop::Options.new.parse(row.fetch('args').dup)
    RuboCop::ConfigLoader.disabled_by_default = options[:disable_all_cops]
    RuboCop::ConfigLoader.enabled_by_default = options[:enable_all_cops]
    config = RuboCop::ConfigLoader.configuration_from_file(File.join(Dir.pwd,'config.yml'))
    value = config.for_cop(row.fetch('query'))['Enabled']
    {id: row.fetch('id'), query: row.fetch('query'), value: value, ruby_class: value.class.name,
     truthy: config.cop_enabled?(row.fetch('query'))}
  end
end
print JSON.generate(rows)
"""
)


def _same(actual, expected):
    if type(actual) is not type(expected):
        raise ValueError("field型が変わりました")
    if isinstance(expected, dict):
        if actual.keys() != expected.keys():
            raise ValueError("field集合が変わりました")
        for key, value in expected.items():
            _same(actual[key], value)
    elif isinstance(expected, list):
        if len(actual) != len(expected):
            raise ValueError("field件数が変わりました")
        for left, right in zip(actual, expected, strict=True):
            _same(left, right)
    elif actual != expected:
        raise ValueError(f"fieldが変わりました: {actual!r} != {expected!r}")


def _forms(data):
    return [
        {key: row[key] for key in ("id", "query", "kind", "files", "args") if key in row}
        for row in data["cases"]
    ]


def _fixture(group):
    if group not in GROUPS:
        raise ValueError("unknown group")
    return json.loads((ROOT / f"tests/fixtures/default_cop_{group}.json").read_bytes())


def completed_report(raw):
    """空/不整合/型違いのJSONを完了reportにしない。

    Args:
        raw: 実Rubyのstdout bytes。

    Returns:
        dict: 全offense fieldsを保つ完了report。

    Raises:
        ValueError: metadata又は完了件数が不正の場合。

    """
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError("reportはobjectです")
    files = value.get("files")
    summary = value.get("summary", {})
    metadata = value.get("metadata", {})
    if not isinstance(summary, dict) or not isinstance(metadata, dict):
        raise ValueError("metadata/summaryはobjectです")
    if (
        not isinstance(files, list)
        or len(files) != 1
        or not isinstance(files[0], dict)
        or files[0].get("path") != "a.rb"
        or not isinstance(files[0].get("offenses"), list)
    ):
        raise ValueError("a.rb一件の完全な検査が必要です")
    if any(
        type(summary.get(key)) is not int
        for key in ("offense_count", "target_file_count", "inspected_file_count")
    ):
        raise ValueError("summary件数は整数です")
    if summary != {
        "offense_count": len(files[0]["offenses"]),
        "target_file_count": 1,
        "inspected_file_count": 1,
    }:
        raise ValueError("不完全なsummaryです")
    if (
        metadata.get("ruby_engine") != "ruby"
        or metadata.get("ruby_version") != "4.0.7"
        or metadata.get("rubocop_version") != "1.89.0"
    ):
        raise ValueError("固定runtimeのreportが必要です")
    return value


def validate_fixture(data, group):
    """固定ID/入力/版/全raw/型と15APIを事前検査する。

    Args:
        data: 公開参照document。
        group: modes/queries/listsのいずれか。

    Raises:
        ValueError: schema、入力、参照の整合性が崩れた場合。

    """
    if group not in GROUPS or type(data.get("schema")) is not int or data["schema"] != 1:
        raise ValueError("unsupported schema/group")
    _same(data.get("runtime"), RUNTIME)
    rows = data.get("cases", [])
    if [row.get("id") for row in rows] != IDS[group]:
        raise ValueError("固定ID/順序/件数が必要です")
    digest = hashlib.sha256(
        json.dumps(_forms(data), sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != FORM_HASHES[group]:
        raise ValueError("固定入力が変わりました")
    for row in rows:
        if type(row.get("exit")) is not int or row.get("coverage") != (
            "list_newline_155" if group == "lists" else "full_report"
        ):
            raise ValueError("終了値/比較範囲が変わりました")
        if bytes.fromhex(row["stderr_hex"]):
            raise ValueError("参照stderrは空です")
        raw = bytes.fromhex(row["stdout_hex"])
        if group == "lists":
            names = raw.decode().splitlines()
            if (
                not names
                or names != sorted(set(names))
                or row["report"] is not None
                or row["exit"] != 0
            ):
                raise ValueError("空/重複/不整合listです")
            _same(row["cop_names"], names)
            if row["id"] == "list-disable-only" and names != ["Lint/Syntax"]:
                raise ValueError("mandatory Syntaxのlistが変わりました")
        else:
            report = completed_report(raw)
            _same(row["report"], report)
            if row["exit"] != int(report["summary"]["offense_count"] != 0):
                raise ValueError("終了値とreportが不整合です")
        _same(row["after_hex"], {name: text.encode().hex() for name, text in row["files"].items()})
    if group == "queries":
        _same(data.get("queries"), QUERIES)


def child_environment():
    """ENV4とNO_COLORを子copyから除き、TERM=dumbの測定条件を明示する。

    そのほかの親/GEM/PATH/OS環境は保つ。

    Returns:
        dict: 非TTY採取の子環境。

    """
    environment = reference_environment()
    for key in list(environment):
        if (key.upper() if os.name == "nt" else key) == "NO_COLOR":
            environment.pop(key)
    environment["TERM"] = "dumb"
    return environment


def _fresh(paths):
    identities = []
    for path in paths:
        if os.path.lexists(path):
            raise ValueError("保存先entryは既に存在します")
        identities.append(os.path.normcase(str(path.resolve(strict=False))))
    if len(set(identities)) != len(paths):
        raise ValueError("別表記の同じ保存先です")


def _inventory(root, files):
    return {
        name: {
            "bytes_hex": (root / name).read_bytes().hex(),
            "mtime_ns": (root / name).stat().st_mtime_ns,
            "mode": (root / name).stat().st_mode,
        }
        for name in files
    }


def _out_entry(root):
    path = root / "report.json"
    if not os.path.lexists(path):
        return None
    mode = path.lstat().st_mode
    if stat.S_ISLNK(mode):
        return {"kind": "symlink", "target": os.readlink(path), "mode": mode}
    if not stat.S_ISREG(mode):
        return {"kind": "non_file", "mode": mode}
    return _inventory(root, ["report.json"])


def _write_json(path, value):
    with path.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(json.dumps(value, ensure_ascii=True, indent=2) + "\n")


def _run(arguments, cwd):
    return subprocess.run(
        arguments, cwd=cwd, env=child_environment(), capture_output=True, timeout=60, check=False
    )


def record_case(ruby, wrapper, root, expected, group):
    """一つの実CLIと全原本before/after属性を独立に記録する。

    Args:
        ruby: 固定Ruby実体。
        wrapper: 実RuboCopだけを起動するwrapper。
        root: fresh case directory。
        expected: 固定参照行。
        group: 参照群。

    Returns:
        dict: 実stdout/stderr/exit/report/原本bytes。

    Raises:
        ValueError: 入力、完了、原本又は出力の契約を満たさない場合。

    """
    template = _fixture(group)
    validate_fixture(template, group)
    matches = [row for row in template["cases"] if row["id"] == expected.get("id")]
    if len(matches) != 1:
        raise ValueError("unknown case")
    _same(expected, matches[0])
    root.mkdir(parents=True)
    for name, text in expected["files"].items():
        (root / name).write_bytes(text.encode())
    before = _inventory(root, expected["files"])
    output = _run([str(ruby), str(wrapper), *expected["args"]], root)
    after = _inventory(root, expected["files"])
    out = root / "report.json"
    _write_json(
        root / "private-cli.json",
        {
            "exit": output.returncode,
            "stdout_hex": output.stdout.hex(),
            "stderr_hex": output.stderr.hex(),
            "inputs_before": before,
            "inputs_after": after,
            "out_before": None,
            "out_after": _out_entry(root),
        },
    )
    if before != after or os.path.lexists(out):
        raise ValueError("CLIが原本/出力先を変えました")
    row = copy.deepcopy(expected)
    row.update(
        exit=output.returncode,
        stdout_hex=output.stdout.hex(),
        stderr_hex=output.stderr.hex(),
        after_hex={name: value["bytes_hex"] for name, value in after.items()},
    )
    row["report"] = None if group == "lists" else completed_report(output.stdout)
    if group == "lists":
        row["cop_names"] = output.stdout.decode().splitlines()
    _same(row, expected)
    return row


def record_queries(ruby, work, cases, expected):
    """元15APIを同じRuby process内で観測し、原本/outの保持を検査する。

    Args:
        ruby: 固定Ruby。
        work: 完了したCLI15入力を保持するprivate work。
        cases: queries群の固定行。
        expected: 実API15参照。

    Returns:
        list: 値/型/truthinessの実15観測。

    Raises:
        ValueError: 固定入力、観測、保持の契約が崩れた場合。

    """
    template = _fixture("queries")
    validate_fixture(template, "queries")
    _same(cases, template["cases"])
    _same(expected, QUERIES)
    before = {row["id"]: _inventory(work / row["id"], row["files"]) for row in cases}
    if any(os.path.lexists(work / row["id"] / "report.json") for row in cases):
        raise ValueError("API前のoutは未作成である必要があります")
    input_file = work / "private-query-input.json"
    _write_json(
        input_file,
        [
            {
                "id": row["id"],
                "query": row["query"],
                "args": row["args"],
                "cwd": str((work / row["id"]).absolute()),
            }
            for row in cases
        ],
    )
    output = _run([str(ruby), "-e", QUERY_API, str(input_file.absolute())], work)
    after = {row["id"]: _inventory(work / row["id"], row["files"]) for row in cases}
    out = {row["id"]: _out_entry(work / row["id"]) for row in cases}
    _write_json(
        work / "private-api.json",
        {
            "exit": output.returncode,
            "stdout_hex": output.stdout.hex(),
            "stderr_hex": output.stderr.hex(),
            "inputs_before": before,
            "inputs_after": after,
            "out_before": {row["id"]: None for row in cases},
            "out_after": out,
        },
    )
    if (
        type(output.returncode) is not int
        or output.returncode != 0
        or output.stderr
        or before != after
        or any(os.path.lexists(work / row["id"] / "report.json") for row in cases)
    ):
        raise ValueError("API観測が未完了又は原本/outが変わりました")
    values = json.loads(output.stdout)
    _same(values, expected)
    return values


def _runtime(ruby, work):
    output = _run([str(ruby), "-e", METADATA], work)
    _write_json(
        work / "private-runtime.json",
        {
            "exit": output.returncode,
            "stdout_hex": output.stdout.hex(),
            "stderr_hex": output.stderr.hex(),
        },
    )
    if output.returncode != 0 or output.stderr:
        raise ValueError("runtime照会に失敗しました")
    value = json.loads(output.stdout)
    _same(value, {"runtime": RUNTIME, "config_source_sha256": CONFIG_SOURCE})
    _same(value["runtime"]["gems"], VERSIONS)
    return value["runtime"]


def collect(ruby, work, output, queries_output, lists_output):
    """全63CLI/15APIが完了してから三つの保存先をexclusive作成する。

    Args:
        ruby: 固定実Ruby。
        work: fresh private work。
        output: modes45保存先。
        queries_output: CLI15/API15保存先。
        lists_output: list3保存先。

    Raises:
        ValueError: fresh又は実観測の契約を満たさない場合。

    """
    _fresh([work, output, queries_output, lists_output])
    documents = {group: _fixture(group) for group in GROUPS}
    for group, value in documents.items():
        validate_fixture(value, group)
    if os.name != "nt":
        raise ValueError("公開Windows rawは同じOSから再採取します")
    work.mkdir(parents=True)
    runtime = _runtime(ruby, work)
    wrapper = work / "reference.rb"
    with wrapper.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(LOADER + "\nload Gem.bin_path('rubocop','rubocop','1.89.0')\n")
    for group, value in documents.items():
        value["runtime"] = runtime
        value["cases"] = [
            record_case(ruby, wrapper.absolute(), work / group / row["id"], row, group)
            for row in value["cases"]
        ]
    documents["queries"]["queries"] = record_queries(
        ruby, work / "queries", documents["queries"]["cases"], documents["queries"]["queries"]
    )
    for path, group in zip((output, queries_output, lists_output), GROUPS, strict=True):
        _write_json(path, documents[group])


def main():
    """候補を受け取らず、版を固定した本家だけを再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "output", "queries-output", "lists-output"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.output, args.queries_output, args.lists_output)


if __name__ == "__main__":
    main()
