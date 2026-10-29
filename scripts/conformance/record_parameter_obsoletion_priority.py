"""最低版getterの追補13CLI/API・8getter・2色条件を実RuboCopだけで再生成する。"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment
from record_parameter_obsoletion import _fresh_paths, _inventory

FIXTURES = Path(__file__).resolve().parents[2] / "tests/fixtures"
PRIORITY_IDS = (
    "bad-env-normal-fatal-0",
    "bad-env-normal-fatal-1",
    "bad-env-version-fatal-0",
    "bad-env-version-fatal-1",
    "bool-target-normal-fatal-0",
    "bool-target-normal-fatal-1",
    "bool-target-version-fatal-0",
    "bool-target-version-fatal-1",
    "parent-bool-root-valid-normal",
    "parent-bool-root-valid-version",
    "namespace-before-bool",
    "empty-env-normal",
    "empty-env-version",
)
COLOR_IDS = ("initial-force-color", "late-cli-color")
SOURCE_PATHS = (
    "lib/rubocop/target_ruby.rb",
    "lib/rubocop/config.rb",
    "lib/rubocop/config_validator.rb",
    "lib/rubocop/config_obsoletion.rb",
    "lib/rubocop/config_obsoletion/parameter_rule.rb",
)
META = (
    LOADER
    + r"""
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
paths = JSON.parse(ARGV.fetch(0))
puts JSON.generate({runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[psych parser prism rubocop-ast rubocop rainbow].to_h do |name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]
  end}, sources: paths.to_h do |path|
    [path, Digest::SHA256.file(File.join(spec.full_gem_path, path)).hexdigest]
  end})
"""
)
CLI = LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n'
LOAD_API = (
    LOADER
    + r"""
require 'json'
require 'stringio'
output = StringIO.new
errors = StringIO.new
$stdout = output
$stderr = errors
begin
  config = RuboCop::ConfigLoader.load_file('config.yml')
  result = {error_class: nil, target: config.target_ruby_version}
rescue StandardError => error
  result = {error_class: error.class.name, message: error.message}
ensure
  $stdout = STDOUT
  $stderr = STDERR
end
result[:stdout_hex] = output.string.unpack1('H*')
result[:stderr_hex] = errors.string.unpack1('H*')
puts JSON.generate(result)
"""
)
GETTERS = (
    LOADER
    + r"""
require 'json'
rows = ['3.10', '3.2suffix', ' 3.2e0', ''].flat_map do |value|
  ['environment', 'configuration'].map do |kind|
    ENV.delete('RUBOCOP_TARGET_RUBY_VERSION')
    hash = {'AllCops' => {'TargetRubyVersion' => kind == 'configuration' ? value : 2.7}}
    ENV['RUBOCOP_TARGET_RUBY_VERSION'] = value if kind == 'environment'
    config = RuboCop::Config.new(hash, 'config.yml')
    {kind: kind, value: value, target: config.target_ruby_version, error_class: nil}
  end
end
puts JSON.generate(rows)
"""
)


def _templates():
    # 候補出力は読まず、公開済みの本家入力だけを再利用する。期待rawから結果を合成しない。
    result = tuple(
        json.loads((FIXTURES / name).read_bytes())
        for name in (
            "parameter_obsoletion_priority.json",
            "parameter_obsoletion_getter.json",
            "parameter_obsoletion_getter_color.json",
        )
    )
    priority, getter, colors = result
    if (
        type(priority.get("schema")) is not int
        or priority["schema"] != 1
        or type(colors.get("schema")) is not int
        or colors["schema"] != 1
        or tuple(row["id"] for row in priority["cases"]) != PRIORITY_IDS
        or tuple(row["id"] for row in colors["cases"]) != COLOR_IDS
        or [(row["kind"], row["value"]) for row in getter["cases"]]
        != [
            (kind, value)
            for value in ("3.10", "3.2suffix", " 3.2e0", "")
            for kind in ("environment", "configuration")
        ]
    ):
        raise ValueError("incomplete additional input matrix")
    scopes = [row["scope"] for row in priority["cases"]]
    if (
        scopes.count("early_version_or_cop_fatal") != 4
        or scopes.count("error_priority_prefix_envelope") != 7
        or scopes.count("existing_target_parser_lifecycle_residual") != 2
    ):
        raise ValueError("unexpected additional coverage")
    return result


def _run(ruby, script, root, environment, *args):
    return subprocess.run(
        [ruby, "-e", script, "--", *args],
        cwd=root,
        env=environment,
        capture_output=True,
        timeout=60,
        check=False,
    )


def _metadata(ruby, work, expected):
    child = _run(ruby, META, work, reference_environment(), json.dumps(SOURCE_PATHS))
    if type(child.returncode) is not int or child.returncode != 0 or child.stderr:
        raise ValueError("reference metadata failed")
    actual = json.loads(child.stdout)
    if (
        actual != {"runtime": expected["runtime"], "sources": expected["sources"]}
        or actual["runtime"]["ruby"] != "4.0.7"
        or actual["runtime"]["gems"] != {"psych": "5.3.1", **VERSIONS}
    ):
        raise ValueError("reference runtime/source identity changed")
    return actual


def _rejection_report(raw):
    report = json.loads(raw)
    identity = report.get("metadata", {})
    summary = report.get("summary", {})
    if (
        identity.get("ruby_engine") != "ruby"
        or identity.get("ruby_version") != "4.0.7"
        or identity.get("rubocop_version") != "1.89.0"
        or report.get("files") != []
        or summary != {"offense_count": 0, "target_file_count": 1, "inspected_file_count": 0}
        or any(type(value) is not int for value in summary.values())
    ):
        raise ValueError("invalid deferred error JSON")


def _validate_api(result, expected):
    if not isinstance(result, dict) or result != expected:
        raise ValueError("load_file getter/priority differs")
    if result.get("error_class") is None:
        if type(result.get("target")) is not float:
            raise ValueError("getter target must be a float")
    elif not isinstance(result["error_class"], str) or not isinstance(result.get("message"), str):
        raise ValueError("invalid getter exception identity")


def record(ruby, template, root, *, api):
    """実CLI/APIの優先順とplain先頭行を保存し、内部Ruby stackを公開しない。

    Returns:
        dict: 本家の公開安全な有限projection。

    Raises:
        ValueError: 原本・完了段階・先頭行・型・out保持が実測範囲と異なる場合。

    """
    allowed = PRIORITY_IDS if api else COLOR_IDS
    if template.get("id") not in allowed:
        raise ValueError("unknown additional scenario")
    root.mkdir(parents=True, exist_ok=False)
    for name, text in template["files"].items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode())
    names = [name for name in template["files"] if name != "report.json"]
    before = _inventory(root, names)
    out = root / "report.json"
    out_before = _inventory(root, ["report.json"])
    environment = {**reference_environment(), "CLICOLOR_FORCE": "0", **template["env"]}
    child = _run(ruby, CLI, root, environment, *template["args"])
    if type(child.returncode) is not int or child.returncode != template["exit"]:
        raise ValueError("unexpected additional exit")
    if before != _inventory(root, names):
        raise ValueError("reference changed original bytes/mtime")
    residual = (
        template["scope"] == "existing_target_parser_lifecycle_residual"
        or template["id"] == "late-cli-color"
    )
    if residual:
        _rejection_report(out.read_bytes())
    elif out_before != _inventory(root, ["report.json"]):
        raise ValueError("reference changed preserved output bytes/mtime")
    lines = child.stderr.splitlines(keepends=True)
    if api and template["scope"] == "early_version_or_cop_fatal":
        prefix = child.stderr
        if child.returncode == 0:
            if child.stdout != b"1.89.0\r\n" or prefix:
                raise ValueError("early version failed")
        elif (
            child.stdout
            or b"Error: The `Gemspec/DateAssignment` cop has been removed." not in prefix
        ):
            raise ValueError("cop fatal did not precede later validation")
    elif api and residual:
        prefix = lines[0] if lines else b""
        if child.stdout or not prefix.startswith(b"RuboCop supports target Ruby versions"):
            raise ValueError("unexpected parser lifecycle residual")
    else:
        marker = b"undefined method 'to_f' for true\r\n"
        offset = child.stderr.find(marker)
        prefix = child.stderr[: offset + len(marker)] if offset >= 0 else b""
        if child.stdout or not prefix or b"\x1b" in child.stderr:
            raise ValueError("getter did not fail with plain prefix")
        before_error = prefix[: -len(marker)]
        if before_error != (
            b"config.yml: Warning: no department given for SpaceAroundOperators.\r\n"
            if template["id"] == "namespace-before-bool"
            else b""
        ):
            raise ValueError("unexpected getter warning prefix")
    row = {key: template[key] for key in ("id", "args", "env", "files", "scope")}
    if api:
        row["version"] = template["version"]
    row.update(
        exit=child.returncode,
        stdout_hex=child.stdout.hex(),
        out_hex=out.read_bytes().hex(),
        stderr_prefix_hex=prefix.hex(),
        input_bytes_mtime_unchanged=True,
        preserved_input_paths=names,
        out_before_hex=out_before["report.json"][0].hex(),
        out_preserved=not residual,
    )
    if api:
        api_out_before = _inventory(root, ["report.json"])
        observed = _run(ruby, LOAD_API, root, environment)
        if type(observed.returncode) is not int or observed.returncode != 0 or observed.stderr:
            raise ValueError("load_file API record failed")
        result = json.loads(observed.stdout)
        _validate_api(result, template["api"])
        if before != _inventory(root, names):
            raise ValueError("load_file API changed original")
        if api_out_before != _inventory(root, ["report.json"]):
            raise ValueError("load_file API changed output bytes/mtime")
        row["api"] = result
    # private work内には全rawも保存するが、公開containerへbacktrace/pathを混ぜない。
    with (root / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(
            {
                "exit": child.returncode,
                "stdout_hex": child.stdout.hex(),
                "stderr_hex": child.stderr.hex(),
                "raw_stderr_sha256": hashlib.sha256(child.stderr).hexdigest(),
                "projection": row,
            },
            handle,
            ensure_ascii=False,
            indent=2,
        )
        handle.write("\n")
    return row


def main():
    """候補引数を持たない実RuboCop専用入口で、追補3containerをfresh保存する。

    Raises:
        ValueError: getter APIの失敗・型・8条件の欠落がある場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    for name in ("work-dir", "output", "getter-output", "color-output"):
        parser.add_argument("--" + name, type=Path, required=True)
    options = parser.parse_args()
    work, output, getter_output, color_output = _fresh_paths(
        options.work_dir, options.output, options.getter_output, options.color_output
    )
    priority, getters, colors = _templates()
    work.mkdir(parents=True, exist_ok=False)
    meta = _metadata(options.ruby, work, priority)
    rows = [record(options.ruby, row, work / row["id"], api=True) for row in priority["cases"]]
    color_rows = [record(options.ruby, row, work / row["id"], api=False) for row in colors["cases"]]
    child = _run(options.ruby, GETTERS, work, reference_environment())
    if type(child.returncode) is not int or child.returncode != 0 or child.stderr:
        raise ValueError("getter API failed")
    getter_rows = json.loads(child.stdout)
    if (
        getter_rows != getters["cases"]
        or len(getter_rows) != 8
        or any(type(row.get("target")) is not float for row in getter_rows)
    ):
        raise ValueError("getter cases/type changed")
    values = [
        (output, {"schema": 1, **meta, "scope": priority["scope"], "cases": rows}),
        (getter_output, {**meta, "cases": getter_rows}),
        (color_output, {"schema": 1, **meta, "cases": color_rows}),
    ]
    for path, value in values:
        with path.open("x", encoding="utf-8", newline="\n") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")


if __name__ == "__main__":
    main()
