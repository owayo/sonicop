"""空config引数の解析とI/O phaseを、実RuboCopから有限記録する。"""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, reference_environment

BASE = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n"

NOTICE = b"-P/--parallel is being ignored because it is not compatible with --cache false.\r\n"
NORMAL = ["--only", "Layout/SpaceAroundOperators", "--format", "json", "--out", "out.json"]
API = (
    LOADER
    + """
require 'json'
require 'stringio'
require 'digest'
rows = JSON.parse(File.binread(ARGV.fetch(0))).map do |input|
  Dir.chdir(input.fetch('cwd')) do
    out, err = $stdout, $stderr
    previous_env = input.fetch('environment', {}).to_h { |key, _| [key, ENV[key]] }
    ENV.update(input.fetch('environment', {}))
    $stdout, $stderr = StringIO.new, StringIO.new
    begin
      options, paths = RuboCop::Options.new.parse(input.fetch('args').dup)
      {id: input.fetch('id'), config_present: options.key?(:config), config: options[:config],
       paths: paths, error_class: nil}
    rescue StandardError, SystemExit => error
      {id: input.fetch('id'), error_class: error.class.name, message: error.message}
    ensure
      $stdout, $stderr = out, err
      previous_env.each { |key, value| value.nil? ? ENV.delete(key) : ENV[key] = value }
    end
  end
end
spec = Gem.loaded_specs.fetch('rubocop')
puts JSON.generate(
  runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
    gems: %w[parser prism rubocop-ast rubocop rainbow].to_h do |name|
      [name, Gem.loaded_specs.fetch(name).version.to_s]
    end,
    rainbow_environment: {TERM: ENV['TERM'], CLICOLOR_FORCE: ENV['CLICOLOR_FORCE']}},
  source_hashes: %w[options.rb config_loader.rb].to_h do |name|
    [name, Digest::SHA256.file(File.join(spec.full_gem_path, 'lib/rubocop', name)).hexdigest]
  end,
  options: rows
)
"""
)


def inputs():
    """空値と値不足を分け、他Issueの既存残差も別分類で残す。

    Returns:
        list: 本家へ渡す20条件の引数・原本・分類。

    """
    cases = []

    def add(name, prefix, kind, suffix=None, extra=None, environment=None):
        cases.append(
            {
                "id": name,
                "args": prefix + (NORMAL + ["case.rb"] if suffix is None else suffix),
                "kind": kind,
                "files": {
                    "case.rb": "x = 1\r\n",
                    "Gemfile": "",
                    ".rubocop.yml": BASE,
                    "good.yml": BASE,
                    **(extra or {}),
                },
                "environment": environment or {},
            }
        )

    add("empty_separate", ["--config", ""], "io")
    add("empty_equals", ["--config="], "io")
    add("empty_short", ["-c", ""], "io")
    add(
        "empty_notice",
        ["--config", ""],
        "io",
        NORMAL + ["--no-parallel", "--cache", "false", "case.rb"],
    )
    add("empty_cache_false", ["--config", ""], "io", NORMAL + ["--cache", "false", "case.rb"])
    add("empty_no_parallel", ["--config", ""], "io", NORMAL + ["--no-parallel", "case.rb"])
    add("empty_version", ["--config", ""], "io", ["--out", "out.json", "--version"])
    add("empty_existing_out", ["--config", ""], "io", extra={"out.json": "KEEP\r\n"})
    add("directory", ["--config", "configdir"], "io")
    add(
        "normal",
        ["--config", "good.yml"],
        "success",
        NORMAL + ["--no-parallel", "--cache", "false", "case.rb"],
    )
    add("omitted", [], "success", NORMAL + ["--no-parallel", "--cache", "false", "case.rb"])
    add("enoent", ["--config", "missing.yml"], "missing")
    add("missing_long", ["--config"], "missing_argument", [])
    add("missing_short", ["-c"], "missing_argument", [])
    add("empty_help", ["--config", ""], "help", ["--help"])
    add(
        "empty_invalid_cache",
        ["--config", ""],
        "argument_priority",
        ["--cache", "invalid", "case.rb"],
    )
    add("empty_unknown_option", ["--config", ""], "argument_priority", ["--unknown-config-probe"])
    add(
        "empty_then_normal",
        ["--config", "", "--config", "good.yml"],
        "override",
        NORMAL + ["--no-parallel", "--cache", "false", "case.rb"],
    )
    add("normal_then_empty", ["--config", "good.yml", "--config", ""], "override")
    add(
        "environment_then_empty",
        ["--config", ""],
        "override",
        environment={"RUBOCOP_OPTS": "--config good.yml"},
    )
    return cases


CASE_IDS = (
    "empty_separate",
    "empty_equals",
    "empty_short",
    "empty_notice",
    "empty_cache_false",
    "empty_no_parallel",
    "empty_version",
    "empty_existing_out",
    "directory",
    "normal",
    "omitted",
    "enoent",
    "missing_long",
    "missing_short",
    "empty_help",
    "empty_invalid_cache",
    "empty_unknown_option",
    "empty_then_normal",
    "normal_then_empty",
    "environment_then_empty",
)


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _selected_inputs():
    forms = inputs()
    _require(tuple(row["id"] for row in forms) == CASE_IDS, "固定20入力の欠落・重複・順序差")
    return forms


def _options(form):
    ident = form["id"]
    failures = {
        "missing_long": ("OptionParser::MissingArgument", "missing argument: --config"),
        "missing_short": ("OptionParser::MissingArgument", "missing argument: -c"),
        "empty_help": ("SystemExit", "exit"),
        "empty_invalid_cache": (
            "RuboCop::OptionArgumentError",
            "-C/--cache argument must be true or false",
        ),
        "empty_unknown_option": (
            "OptionParser::InvalidOption",
            "invalid option: --unknown-config-probe",
        ),
    }
    if ident in failures:
        error, message = failures[ident]
        return {"id": ident, "error_class": error, "message": message}
    config = {
        "directory": "configdir",
        "normal": "good.yml",
        "empty_then_normal": "good.yml",
        "enoent": "missing.yml",
        "omitted": None,
    }.get(ident, "")
    return {
        "id": ident,
        "config_present": ident != "omitted",
        "config": config,
        "paths": [] if ident == "empty_version" else ["case.rb"],
        "error_class": None,
    }


def _report(raw):
    report = json.loads(raw)
    identity = report["metadata"]
    _require(
        identity["rubocop_version"] == "1.89.0"
        and identity["ruby_version"] == "4.0.7"
        and identity["ruby_engine"] == "ruby",
        "本家report identityが不一致",
    )
    summary = report["summary"]
    _require(
        all(
            type(summary.get(key)) is int
            for key in ("offense_count", "target_file_count", "inspected_file_count")
        ),
        "完了件数はintだけを認める",
    )
    _require(
        summary == {"offense_count": 0, "target_file_count": 1, "inspected_file_count": 1}
        and [row["path"] for row in report["files"]] == ["case.rb"]
        and report["files"][0]["offenses"] == [],
        "reportが未完了・別path・offenseと不整合",
    )


def validate(document):
    """元rawの有限分類を守り、入力・Options・完了状態の矛盾を拒否する。

    Raises:
        ValueError: schemaや実Oracleの証拠が不完全な場合。

    """
    if document.get("schema") != 1 or type(document.get("schema")) is not int:
        raise ValueError("schemaが不一致")
    runtime = document["runtime"]
    _require(
        runtime["ruby"] == "4.0.7"
        and runtime["gems"] == VERSIONS
        and "mingw" in runtime["platform"],
        "本家runtimeが不一致",
    )
    _require(
        runtime["rainbow_environment"] == {"TERM": "dumb", "CLICOLOR_FORCE": None},
        "Rainbow参照profileが不一致",
    )
    sources = document["source_hashes"]
    _require(
        set(sources) == {"options.rb", "config_loader.rb"}
        and all(
            isinstance(value, str)
            and len(value) == 64
            and all(c in "0123456789abcdef" for c in value)
            for value in sources.values()
        ),
        "source SHA256が不完全",
    )
    forms = _selected_inputs()
    rows = document["cases"]
    _require(tuple(row["id"] for row in rows) == CASE_IDS, "固定20記録の欠落・重複・順序差")
    for row, form in zip(rows, forms, strict=True):
        _require({key: row[key] for key in form} == form, "固定args/files/kind/environmentが不一致")
        api = row["options"]
        _require(
            "config_present" not in api or type(api["config_present"]) is bool,
            "Optionsのpresenceはboolだけを認める",
        )
        _require(row["options"] == _options(form), "Options実APIが不一致")
        _require(type(row["exit"]) is int, "exitが整数でない")
        ident = row["id"]
        completed = ident in {"normal", "omitted", "empty_then_normal"}
        _require(
            row["exit"] == (0 if completed or row["kind"] == "help" else 2), "参照exitが不一致"
        )
        stdout = bytes.fromhex(row["stdout_hex"])
        if ident == "empty_help":
            _require(
                stdout.startswith(b"Usage: rubocop [options]") and b"General Options:" in stdout,
                "helpが未完了",
            )
        else:
            expected = (
                NOTICE
                if ident in {"empty_notice", "normal", "omitted", "empty_then_normal"}
                else b""
            )
            _require(stdout == expected, "参照stdout/通知が不一致")
        if row["kind"] == "io" or ident in {"normal_then_empty", "environment_then_empty"}:
            expected_line = "Is a directory @ rb_sysopen - {cwd}" + (
                "/configdir" if ident == "directory" else ""
            )
        elif ident == "enoent":
            expected_line = "Configuration file not found: {cwd}/missing.yml"
        else:
            expected_line = (
                _options(form).get("message", "") if not completed and ident != "empty_help" else ""
            )
        _require(row["stderr_first_line"] == expected_line, "参照I/O/引数phaseが不一致")
        digest = row["stderr_sha256"]
        _require(
            isinstance(digest, str)
            and len(digest) == 64
            and all(c in "0123456789abcdef" for c in digest),
            "stderr SHA256が不完全",
        )
        if not expected_line:
            _require(digest == hashlib.sha256(b"").hexdigest(), "正常/helpで隠れたstderrがある")
        if completed:
            _require(isinstance(row["out_hex"], str) and row["out_hex"], "正常outが未作成")
            _report(bytes.fromhex(row["out_hex"]))
        else:
            expected_out = form["files"].get("out.json")
            _require(
                row["out_hex"]
                == (expected_out.encode().hex() if expected_out is not None else None),
                "拒否でoutを作成・変更した",
            )
        _require(
            row["after_hex"] == {name: text.encode().hex() for name, text in form["files"].items()},
            "原本bytesが変わった",
        )


def _fresh_paths(work, output):
    _require(
        not any(os.path.lexists(path) for path in (work, output)),
        "既存entryやdangling symlinkを上書きしない",
    )
    # 元entryを先に守り、保存先identityだけは親のsymlink・dotdot・Windows case差を畳む。
    resolved = tuple(path.resolve(strict=False) for path in (work, output))
    identities = {os.path.normcase(os.path.normpath(str(path))) for path in resolved}
    _require(len(identities) == 2, "work/outputが同じ保存先")
    _require(not any(os.path.lexists(path) for path in resolved), "正規化先が既存entry")
    return resolved


def _inventory(cwd, names):
    return {name: ((cwd / name).read_bytes(), (cwd / name).stat().st_mtime_ns) for name in names}


def _capture(ruby, wrapper, work, case, environment):
    cwd = work / case["id"]
    cwd.mkdir(exist_ok=False)
    for name, text in case["files"].items():
        (cwd / name).write_bytes(text.encode())
    (cwd / "configdir").mkdir()
    before = _inventory(cwd, case["files"])
    # 注入ENV4は共通filterへ任せ、優先順の明示入力だけを子copyへ戻す。
    child_env = {**environment, **case["environment"]}
    done = subprocess.run(
        [str(ruby), str(wrapper), *case["args"]],
        cwd=cwd,
        env=child_env,
        capture_output=True,
        timeout=40,
        check=False,
    )
    _require(before == _inventory(cwd, case["files"]), "原本bytes/mtimeが変わった")
    out = cwd / "out.json"
    row = {
        **case,
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
        "after_hex": {name: (cwd / name).read_bytes().hex() for name in case["files"]},
        "out_hex": out.read_bytes().hex() if out.exists() else None,
    }
    with (cwd / "raw.json").open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(row, handle, ensure_ascii=True, indent=2)
        handle.write("\n")
    return cwd, row


def record(ruby, output, work):
    """固定版の実Rubyだけを起動し、全20件が完了してから新規保存する。

    Returns:
        dict: 元の入力とOptions APIを保持する公開記録。

    Raises:
        RuntimeError: Windows以外で実profileを採取しようとした場合。
        ValueError: Options metadataが不完全な場合。

    """
    work, output = _fresh_paths(work, output)
    if os.name != "nt":
        raise RuntimeError("this records the Windows reference profile")
    forms = _selected_inputs()
    work.mkdir(parents=True, exist_ok=False)
    wrapper = work / "reference.rb"
    with wrapper.open("x", encoding="utf-8", newline="\n") as handle:
        handle.write(LOADER + "\nload Gem.bin_path('rubocop','rubocop','1.89.0')\n")
    environment = reference_environment()
    # 実記録のRainbow条件を明示し、親やGEM/OS探索環境には触れない。
    environment["TERM"] = "dumb"
    environment.pop("CLICOLOR_FORCE", None)
    rows = [_capture(ruby, wrapper, work, case, environment) for case in forms]
    options = work / "options-input.json"
    with options.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(
            [
                {
                    "id": row["id"],
                    "cwd": str(cwd),
                    "args": row["args"],
                    "environment": row["environment"],
                }
                for cwd, row in rows
            ],
            handle,
        )
    api_run = subprocess.run(
        [str(ruby), "-e", API, str(options)],
        env=environment,
        capture_output=True,
        timeout=40,
        check=True,
    )
    if api_run.stderr:
        raise ValueError("Options metadata wrote stderr")
    metadata = json.loads(api_run.stdout)
    apis = metadata.pop("options")
    if tuple(row["id"] for row in apis) != CASE_IDS:
        raise ValueError("Options API identityが不完全")
    recorded = []
    for (cwd, raw), api in zip(rows, apis, strict=True):
        row = dict(raw)
        stderr = bytes.fromhex(row.pop("stderr_hex"))
        # 私用backtraceはraw.jsonだけへ残し、公開するのは先頭行と全量hashに限る。
        row["stderr_first_line"] = (
            stderr.split(b"\r\n", 1)[0].decode().replace(cwd.as_posix(), "{cwd}")
        )
        row["stderr_sha256"] = hashlib.sha256(stderr).hexdigest()
        row["options"] = api
        recorded.append(row)
    result = {"schema": 1, **metadata, "cases": recorded}
    validate(result)
    with output.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(result, handle, ensure_ascii=True, indent=2)
        handle.write("\n")
    return result


def main():
    """実Rubyと未使用の保存先を明示して採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    record(args.ruby.absolute(), args.output.absolute(), args.work_dir.absolute())


if __name__ == "__main__":
    main()
