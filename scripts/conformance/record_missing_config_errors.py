"""候補を呼ばず、Windows本家のmissing-config本文とI/O陰性対照を保存する。"""

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

from record_config_cop_namespaces import LOADER, VERSIONS, _registry, reference_environment

BASE = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n"
NOTICE = b"-P/--parallel is being ignored because it is not compatible with --cache false.\r\n"
MISSING = "RuboCop::ConfigNotFoundError"
API = (
    LOADER
    + """
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
inputs = JSON.parse(File.binread(ARGV.fetch(0)))
classes = inputs.map do |input|
  Dir.chdir(input.fetch('cwd')) do
    begin
      RuboCop::ConfigLoader.load_file(input.fetch('config'))
      {id: input.fetch('id'), error_class: nil, message: nil}
    rescue StandardError => error
      {id: input.fetch('id'), error_class: error.class.name, message: error.message}
    end
  end
end
versions = %w[parser prism rubocop-ast rubocop rainbow].to_h do |name|
  [name, Gem.loaded_specs.fetch(name).version.to_s]
end
source_hashes = %w[config_loader.rb cli.rb].to_h do |name|
  [name, Digest::SHA256.file(File.join(spec.full_gem_path, 'lib/rubocop', name)).hexdigest]
end
puts JSON.generate(
  runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM, gems: versions,
    external_encoding: Encoding.default_external.name,
    stderr_encoding: $stderr.external_encoding&.name,
    rainbow_initial: Rainbow.enabled,
    rainbow_environment: {TERM: ENV['TERM'], CLICOLOR_FORCE: ENV['CLICOLOR_FORCE']}},
  source_hashes: source_hashes, classes: classes
)
"""
)


def inputs():
    """読み取りphaseの違いを、実測済みの17条件へ固定する。

    Returns:
        入力ファイル、引数、期待するエラー分類の一覧。

    """
    cases = []

    def add(
        name,
        config,
        extra=None,
        kind="missing",
        directories=None,
        environment=None,
        version=False,
        color=False,
        discovered=False,
    ):
        args = [] if discovered else ["--config", config]
        if version:
            args += ["--out", "out.json", "--version"]
        else:
            if color:
                args += ["--color"]
            args += [
                "--only",
                "Layout/SpaceAroundOperators",
                "--no-display-cop-names",
                "--format",
                # 入れ子はphaseの色を測り、finishedのsummaryを出さないfilesを使う。
                "files" if discovered else "json",
            ]
            if not discovered:
                args += ["--out", "out.json"]
            args += [
                "--no-parallel",
                "--cache",
                "false",
                "sub/case.rb" if discovered else "case.rb",
            ]
        cases.append(
            {
                "id": name,
                "config": config,
                "kind": kind,
                "args": args,
                "files": {"case.rb": "x=1\n", "Gemfile": "", **(extra or {})},
                "directories": directories or [],
                "environment": environment or {},
                "version": version,
                "discovered": discovered,
            }
        )

    add("relative", "missing.yml")
    add("absolute", "{cwd}/missing.yml")
    add("parent", "absent/../missing.yml")
    add("backslash", ".\\absent\\..\\missing.yml")
    add("unicode", "未設定 名.yml")
    add("inherit", "good.yml", {"good.yml": "inherit_from: absent/../missing.yml\n" + BASE})
    add(
        "nested_inherit",
        "configs/good.yml",
        {"configs/good.yml": "inherit_from: ../absent/../missing.yml\n" + BASE},
    )
    add("directory", "configdir", kind="io", directories=["configdir"])
    add("empty", "", kind="io")
    add("normal", "good.yml", {"good.yml": BASE, "case.rb": "x = 1\r\n"}, kind="success")
    add("version", "missing.yml", version=True)
    add("color_option", "missing.yml", color=True)
    add("color_environment", "missing.yml", environment={"CLICOLOR_FORCE": "1"})
    add("existing_out", "missing.yml", {"out.json": "KEEP\r\n"})
    add(
        "nested_missing",
        "sub/.rubocop.yml",
        {
            ".rubocop.yml": BASE,
            "sub/.rubocop.yml": "inherit_from: missing.yml\n" + BASE,
            "sub/case.rb": "x=1\n",
        },
        color=True,
        discovered=True,
    )
    add("verbatim", "{verbatim}\\missing.yml")
    add(
        "verbatim_inherit",
        "{verbatim}\\good.yml",
        {"good.yml": "inherit_from: absent/../missing.yml\n" + BASE},
    )
    for case in cases:
        case["coverage"] = (
            "known_residual"
            if case["id"] == "empty"
            else "missing"
            if case["kind"] == "missing"
            else "json"
            if case["kind"] == "success"
            else "directory_envelope"
        )
    return cases


CASE_IDS = (
    "relative",
    "absolute",
    "parent",
    "backslash",
    "unicode",
    "inherit",
    "nested_inherit",
    "directory",
    "empty",
    "normal",
    "version",
    "color_option",
    "color_environment",
    "existing_out",
    "nested_missing",
    "verbatim",
    "verbatim_inherit",
)


def _selected_inputs(only):
    cases = inputs()
    if [case["id"] for case in cases] != list(CASE_IDS):
        raise ValueError("incomplete input matrix")
    if only is not None and (not only or len(set(only)) != len(only) or set(only) - set(CASE_IDS)):
        raise ValueError("empty, duplicate or unknown selected cases")
    return [case for case in cases if only is None or case["id"] in only]


def expand(value, cwd):
    """実験ディレクトリを、元の引数のslash表記を壊さず展開する。

    Args:
        value: cwdまたはverbatimのplaceholderを持つ文字列。
        cwd: 条件ごとに作った独立ディレクトリ。

    Returns:
        参照へ渡す実パスまたは引数。

    """
    return value.replace("{cwd}", cwd.as_posix()).replace("{verbatim}", "\\\\?\\" + str(cwd))


def validate_metadata(metadata, selected):
    """版違いとerror classの欠落を、正常記録へ固定しない。

    Args:
        metadata: 実gem APIから採った版とエラー型。
        selected: 採取を依頼した条件IDの集合。

    Raises:
        ValueError: 版または条件集合が一致しない場合。

    """
    if not selected or len(set(selected)) != len(selected):
        raise ValueError("empty or duplicate selected cases")
    if metadata["runtime"]["ruby"] != "4.0.7" or metadata["runtime"]["gems"] != VERSIONS:
        raise ValueError("unexpected loaded reference versions")
    classes = metadata["classes"]
    if len(classes) != len(selected) or {row["id"] for row in classes} != set(selected):
        raise ValueError("incomplete reference error classes")


def validate_case(case, error, stdout, stderr, out_hex, initial_color):
    """ENOENT以外の握り潰しと、空の偽CLIを拒む。

    Args:
        case: 固定した条件。
        error: 同じ入力に対する本家load_fileの型と本文。
        stdout: 実CLIの生stdout。
        stderr: 実CLIの生stderr。
        out_hex: CLIが出力先へ書いた生バイト。未作成ならNone。
        initial_color: 本家Rainbowの初期状態。

    Raises:
        ValueError: 本文、通知、型、完了数または出力先の証拠が不一致の場合。

    """
    if case["kind"] not in {"missing", "io", "success"}:
        raise ValueError("unknown case kind")
    if stdout != (b"" if case["version"] else NOTICE):
        raise ValueError("unexpected reference stdout before config failure")
    if case["kind"] == "missing":
        if error["error_class"] != MISSING:
            raise ValueError("missing config did not raise ConfigNotFoundError")
        color = initial_color or case["environment"].get("CLICOLOR_FORCE") == "1"
        color = color or case["discovered"]
        text = error["message"].encode("utf-8")
        expected = b"\x1b[31m" + text + b"\x1b[0m\r\n" if color else text + b"\r\n"
        if stderr != expected:
            raise ValueError("incomplete missing-config body")
    elif case["kind"] == "io":
        if (
            error["error_class"] != "Errno::EISDIR"
            or not stderr
            or b"Configuration file not found:" in stderr
        ):
            raise ValueError("non-ENOENT error was hidden as missing config")
    else:
        if error["error_class"] is not None or stderr or out_hex is None:
            raise ValueError("normal config did not complete")
        report = json.loads(bytes.fromhex(out_hex))
        metadata = report["metadata"]
        if (
            metadata["rubocop_version"] != "1.89.0"
            or metadata["ruby_engine"] != "ruby"
            or metadata["ruby_version"] != "4.0.7"
        ):
            raise ValueError("unexpected report identity")
        if report["summary"] != {
            "offense_count": 0,
            "target_file_count": 1,
            "inspected_file_count": 1,
        }:
            raise ValueError("incomplete normal inspection")
        if [file["path"] for file in report["files"]] != ["case.rb"] or any(
            file["offenses"] for file in report["files"]
        ):
            raise ValueError("incomplete normal target paths")
    if case["kind"] != "success":
        original = case["files"].get("out.json")
        if out_hex != (original.encode().hex() if original is not None else None):
            raise ValueError("config error created or overwrote --out")


def _fresh_paths(work, output):
    if output.absolute() == work.absolute() or any(
        os.path.lexists(path) for path in (output, work)
    ):
        raise ValueError("既存work/outputを上書きしない")


def record(ruby, destination, work, only=None):
    """原本を保った実CLIと実error classだけを公開記録へ保存する。

    Args:
        ruby: 参照Rubyの実行ファイル。
        destination: 公開するJSONの出力先。
        work: 独立した実験入力の保存先。
        only: 再採取する条件ID。未指定なら全条件。

    Raises:
        ValueError: 未完了、版違い、原本変更、条件欠落がある場合。
        RuntimeError: Windowsの参照プロファイル以外で実行した場合。

    """
    _fresh_paths(work, destination)
    cases = _selected_inputs(only)
    if os.name != "nt":
        raise RuntimeError("this recorder captures the Windows reference profile")
    env = reference_environment()
    work.mkdir(parents=True, exist_ok=False)
    wrapper = work / "reference.rb"
    wrapper.write_text(
        LOADER + "\nload Gem.bin_path('rubocop','rubocop','1.89.0')\n",
        encoding="utf-8",
        newline="\n",
    )
    rows = []
    api_inputs = []
    for case in cases:
        cwd = work / case["id"]
        cwd.mkdir(parents=True, exist_ok=False)
        for path, text in case["files"].items():
            file = cwd / path
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(text.encode())
        for path in case["directories"]:
            (cwd / path).mkdir()
        mtimes = {path: (cwd / path).stat().st_mtime_ns for path in case["files"]}
        args = [expand(arg, cwd) for arg in case["args"]]
        run = subprocess.run(
            [str(ruby), str(wrapper), *args],
            cwd=cwd,
            env={**env, **case["environment"]},
            capture_output=True,
            timeout=40,
        )
        if run.returncode != (0 if case["kind"] == "success" else 2):
            raise ValueError("unexpected reference exit")
        after = {path: (cwd / path).read_bytes().hex() for path in case["files"]}
        if after != {path: text.encode().hex() for path, text in case["files"].items()}:
            raise ValueError("reference changed an original")
        if any((cwd / path).stat().st_mtime_ns != modified for path, modified in mtimes.items()):
            raise ValueError("reference changed an original mtime")
        out = cwd / "out.json"
        out_hex = out.read_bytes().hex() if out.exists() else None
        raw = {
            **case,
            "exit": run.returncode,
            "stdout_hex": run.stdout.hex(),
            "stderr_hex": run.stderr.hex(),
            "after_hex": after,
            "out_hex": out_hex,
        }
        (cwd / "raw.json").write_text(json.dumps(raw, indent=2) + "\n", newline="\n")
        rows.append((cwd, raw))
        api_inputs.append(
            {"id": case["id"], "cwd": str(cwd), "config": expand(case["config"], cwd)}
        )
    input_file = work / "api-inputs.json"
    input_file.write_text(json.dumps(api_inputs), newline="\n")
    api_run = subprocess.run(
        [str(ruby), "-e", API, str(input_file)],
        env=env,
        capture_output=True,
        check=True,
        timeout=40,
    )
    if api_run.stderr:
        raise ValueError("reference metadata wrote stderr")
    metadata = json.loads(api_run.stdout)
    validate_metadata(metadata, [case["id"] for case in cases])
    classes = {row["id"]: row for row in metadata.pop("classes")}
    recorded = []
    for cwd, raw in rows:
        error = classes[raw["id"]]
        stdout = bytes.fromhex(raw["stdout_hex"])
        stderr = bytes.fromhex(raw["stderr_hex"])
        validate_case(
            raw, error, stdout, stderr, raw["out_hex"], metadata["runtime"]["rainbow_initial"]
        )
        message = error["message"]
        raw["error_class"] = error["error_class"]
        raw["error_message_template"] = (
            message.replace(cwd.as_posix(), "{cwd}") if message else None
        )
        raw["stderr_sha256"] = hashlib.sha256(stderr).hexdigest()
        # 別I/O errorのbacktraceは私用raw.jsonへ保持し、本件の本文一致へ数えない。
        raw["stderr_hex"] = (
            stderr.replace(cwd.as_posix().encode(), b"{cwd}").hex() if raw["kind"] != "io" else None
        )
        recorded.append(raw)
    # 採取中に現れたentryやdangling symlinkも、出力時のx modeで守る。
    with destination.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(
            json.dumps({"schema": 1, **metadata, "cases": recorded}, ensure_ascii=True, indent=2)
            + "\n"
        )


EARLY_IDS = (
    "version-valid",
    "version-invalid-target",
    "force-default-invalid-target",
    "version-parameter",
    "verbose-valid",
    "version-inherit-missing",
    "force-inherit-missing",
    "version-scalar",
)


def _validate_early_template(fixture):
    if fixture["schema"] != 1 or [case["id"] for case in fixture["cases"]] != list(EARLY_IDS):
        raise ValueError("incomplete early phase matrix")


def _record_early(ruby, destination, work, only=None):
    _fresh_paths(work, destination)
    if only is not None:
        raise ValueError("early phase採取は全8条件を要求する")
    if os.name != "nt":
        raise RuntimeError("this recorder captures the Windows reference profile")
    template = Path(__file__).parents[2] / "tests/fixtures/missing_config_early_phases.json"
    fixture = json.loads(template.read_text(encoding="utf-8"))
    _validate_early_template(fixture)
    work.mkdir(parents=True, exist_ok=False)
    api = _registry(str(ruby), work)
    if api["runtime"] != fixture["runtime"]:
        raise ValueError("unexpected early runtime")
    wrapper = work / "reference.rb"
    wrapper.write_text(
        LOADER + "\nload Gem.bin_path('rubocop','rubocop','1.89.0')\n",
        encoding="utf-8",
        newline="\n",
    )
    rows = []
    for case in fixture["cases"]:
        cwd = work / case["id"]
        cwd.mkdir()
        for name, source in fixture["files"].items():
            (cwd / name).write_bytes(source.encode())
        mtimes = {name: (cwd / name).stat().st_mtime_ns for name in fixture["files"]}
        done = subprocess.run(
            [str(ruby), str(wrapper), *case["args"]],
            cwd=cwd,
            env={**reference_environment(), "CLICOLOR_FORCE": "0", "TERM": "dumb"},
            capture_output=True,
            check=False,
            timeout=40,
        )
        stderr = done.stderr.replace(cwd.as_posix().encode(), b"{cwd}")
        if (
            done.returncode != case["exit"]
            or done.stdout.hex() != case["stdout_hex"]
            or stderr.hex() != case["stderr_hex"]
        ):
            raise ValueError(f"early phase参照が不一致: {case['id']}")
        for name, source in fixture["files"].items():
            if (cwd / name).read_bytes() != source.encode() or (
                cwd / name
            ).stat().st_mtime_ns != mtimes[name]:
                raise ValueError("early phase参照が原本を変更した")
        raw = {
            "exit": done.returncode,
            "stdout_hex": done.stdout.hex(),
            "stderr_hex": done.stderr.hex(),
        }
        (cwd / "raw.json").write_text(
            json.dumps(raw, indent=2) + "\n", encoding="utf-8", newline="\n"
        )
        rows.append(
            {
                **case,
                "exit": done.returncode,
                "stdout_hex": done.stdout.hex(),
                "stderr_hex": stderr.hex(),
                "raw_stderr_sha256": hashlib.sha256(done.stderr).hexdigest(),
            }
        )
    result = {**fixture, "runtime": api["runtime"], "cases": rows}
    _validate_early_template(result)
    with destination.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(json.dumps(result, ensure_ascii=True, indent=2) + "\n")


def main():
    """参照Rubyと実験入力の保存先を明示して再生成する入口。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--case", action="append")
    parser.add_argument(
        "--early", action="store_true", help="版表示とforce-defaultの8負対照を別に採取"
    )
    args = parser.parse_args()
    recorder = _record_early if args.early else record
    if args.work_dir:
        recorder(args.ruby.resolve(), args.output.absolute(), args.work_dir.absolute(), args.case)
    else:
        with tempfile.TemporaryDirectory() as directory:
            recorder(
                args.ruby.resolve(), args.output.absolute(), Path(directory) / "work", args.case
            )


if __name__ == "__main__":
    main()
