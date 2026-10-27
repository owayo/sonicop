"""RuboCop 1.89.0のCLI選択CSVを、候補を参照せず有限入力から記録する。"""

import argparse
import concurrent.futures
import json
import os
import subprocess
from pathlib import Path

SPACE = "Layout/SpaceAroundOperators"
STRING = "Style/StringLiterals"
SOURCE = 'x=1\nmessage = "text"\n'
YAML = (
    "AllCops:\n  NewCops: disable\n  DisabledByDefault: true\n  TargetRubyVersion: 2.7\n"
    "Layout/SpaceAroundOperators:\n  Enabled: true\nStyle/StringLiterals:\n  Enabled: true\n"
)
ENV_OPTIONS = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
OPTIONS_TRACE = r"""
gem 'parser', '=3.3.12.0'
gem 'rubocop', '=1.89.0'
require 'rubocop'
require 'json'
parser = RuboCop::Options.new
paths = nil
error = nil
begin
  options, paths = parser.parse(ARGV)
rescue StandardError => e
  options = parser.instance_variable_get(:@options)
  error = e.class.name
end
puts JSON.generate({ruby: RUBY_VERSION, parser: Parser::VERSION,
  rubocop: RuboCop::Version::STRING, only_present: options.key?(:only),
  only: options[:only], except_present: options.key?(:except),
  except: options[:except], paths: paths, error: error})
"""


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _environment():
    env = os.environ.copy()
    for name in ENV_OPTIONS:
        env.pop(name, None)
    return env


def _forms():
    values = [
        "",
        ",",
        ",,",
        " ",
        "\t",
        "\n",
        SPACE,
        SPACE + ",",
        SPACE + ",,",
        "," + SPACE,
        SPACE + ",," + STRING,
        " " + SPACE + " ",
        SPACE + ", " + STRING,
        SPACE + " ," + STRING,
        SPACE + ", ",
        " , ",
        "Lint/Syntax",
        "Syntax",
        "Lint/Syntax,",
        ",Lint/Syntax",
        "Unknown/Cop",
    ]
    forms = []
    for flag in ("--only", "--except"):
        for index, value in enumerate(values):
            coverage = "rejection_only"
            if index in (1, 2, 6, 7, 8, 12) or (flag == "--only" and index in (16, 17, 18)):
                coverage = "full_json"
            if flag == "--except" and index in (16, 17, 18, 19):
                coverage = "rejection_envelope"
            if index == 5:
                coverage = "parse_boundary"
            forms.append(
                {"id": f"{flag[2:]}_{index:02}", "options": [flag, value], "coverage": coverage}
            )
    forms.append({"id": "unspecified", "options": [], "coverage": "full_json"})
    for flag in ("--only", "--except"):
        forms.extend(
            [
                {
                    "id": flag[2:] + "_missing_end",
                    "options": [],
                    "tail": [flag],
                    "coverage": "rejection_envelope",
                },
                {
                    "id": flag[2:] + "_equals_empty",
                    "options": [flag + "="],
                    "coverage": "rejection_only",
                },
                {
                    "id": flag[2:] + "_short",
                    "options": [flag, "SpaceAroundOperators"],
                    "coverage": "full_json",
                },
                {
                    "id": flag[2:] + "_namespace",
                    "options": [flag, "Wrong/SpaceAroundOperators"],
                    "coverage": "full_json",
                },
                {
                    "id": flag[2:] + "_syntax",
                    "options": [flag, ",,"],
                    "source": "def foo\n",
                    "coverage": "full_json",
                },
            ]
        )
    forms.append(
        {
            "id": "filename_value",
            "options": ["--only", "input.rb"],
            "target": [],
            "coverage": "rejection_only",
            "modes": ["file"],
        }
    )
    for name in ("Syntax", "Wrong/SpaceAroundOperators"):
        forms.append(
            {
                "id": "no_target_" + name.replace("/", "_"),
                "options": ["--only", name],
                "inputs": {},
                "target": [],
                "coverage": "full_json",
                "modes": ["file"],
            }
        )
    forms.append(
        {
            "id": "no_target_except_namespace",
            "options": ["--except", "Wrong/SpaceAroundOperators"],
            "inputs": {},
            "target": [],
            "coverage": "full_json",
            "modes": ["file"],
        }
    )
    for flag in ("--only", "--except"):
        forms.append(
            {
                "id": flag[2:] + "_two_files",
                "options": [flag, "Wrong/SpaceAroundOperators"],
                "inputs": {"input.rb": SOURCE, "second.rb": SOURCE},
                "target": ["input.rb", "second.rb"],
                "coverage": "full_json",
                "modes": ["file"],
            }
        )
        forms.append(
            {
                "id": flag[2:] + "_format_slots",
                "options": [flag, ",,"],
                "custom": ["-f", "json", "-o", "first.json", "-f", "json", "-o", "second.json"],
                "coverage": "full_json",
                "modes": ["file"],
            }
        )
    return forms


def _badge_forms():
    values = [
        ("wrong_department", "wrong/SpaceAroundOperators", "full_json"),
        ("snake_cop", "Wrong/space_around_operators", "full_json"),
        ("canonical", SPACE, "full_json"),
        ("empty", "", "rejection_only"),
        ("separators", ",", "full_json"),
        ("unknown", "Unknown/Cop", "rejection_only"),
        ("bare_snake", "space_around_operators", "full_json"),
        ("registered_badge_original", "layout/space_around_operators", "rejection_only"),
    ]
    return [
        {"id": f"{flag[2:]}_{name}", "options": [flag, value], "coverage": coverage}
        for flag in ("--only", "--except")
        for name, value, coverage in values
    ]


def _directory_forms():
    # #114 の directory stdin を経由しても、cop を初めて使う時点で一度だけ警告する。
    one = {"directory/input.rb": "disk_original = true\n"}
    two = {
        "directory/a.rb": "disk_original = true\n",
        "directory/b.rb": "disk_original = true\n",
    }
    forms = []
    for name, inputs, options, tail, coverage in [
        ("one", one, ["--only", "SpaceAroundOperators"], [], "full_json"),
        ("two", two, ["--only", "SpaceAroundOperators"], [], "full_json"),
        ("empty", {}, ["--only", "SpaceAroundOperators"], [], "full_json"),
        (
            "listing",
            one,
            ["--only", "SpaceAroundOperators"],
            ["--list-target-files"],
            "listing",
        ),
        (
            "safe",
            one,
            ["--autocorrect", "--only", "SpaceAroundOperators"],
            [],
            "full_json",
        ),
        (
            "except_namespace",
            one,
            ["--except", "Wrong/SpaceAroundOperators"],
            [],
            "full_json",
        ),
    ]:
        forms.append(
            {
                "id": "directory_" + name,
                "inputs": inputs,
                "options": options,
                "target": ["--stdin", "directory"],
                "directories": ["directory"],
                "tail": tail,
                "coverage": coverage,
                "modes": ["stdin"],
            }
        )
    return forms


def _report(data):
    report = json.loads(data)
    metadata = report.pop("metadata")
    _require(metadata["rubocop_version"] == "1.89.0", "本家RuboCop版が不一致")
    _require(
        metadata["ruby_engine"] == "ruby" and metadata["ruby_version"] == "4.0.7",
        "本家Ruby版が不一致",
    )
    _require("mingw" in metadata["ruby_platform"], "Windows参照専用")
    return report


def _validate_report(report, paths):
    _require([file["path"] for file in report["files"]] == paths, "検査対象path/順序が不一致")
    _require(report["summary"]["target_file_count"] == len(paths), "対象件数が不一致")
    _require(report["summary"]["inspected_file_count"] == len(paths), "未検査の対象がある")
    _require(
        report["summary"]["offense_count"]
        == sum(len(file["offenses"]) for file in report["files"]),
        "診断集計が不一致",
    )


def _validate_outputs(outputs, custom):
    expected = {"first.json", "second.json"} if custom else set()
    actual = {name for name, value in outputs.items() if value is not None}
    _require(actual == expected, "formatterの保存先集合が不一致")


def _record(job, args):
    index, form, mode = job
    directory = Path(args.work) / f"{index:02}-{mode}"
    directory.mkdir(parents=True, exist_ok=False)
    (directory / ".rubocop.yml").write_bytes(YAML.encode())
    for name in form.get("directories", []):
        (directory / name).mkdir(parents=True, exist_ok=True)
    inputs = form.get("inputs", {"input.rb": form.get("source", SOURCE)})
    for name, source in inputs.items():
        (directory / name).parent.mkdir(parents=True, exist_ok=True)
        (directory / name).write_bytes(source.encode())
    target = form.get("target", ["--stdin", "input.rb"] if mode == "stdin" else ["input.rb"])
    command = (
        ["--cache", "false"]
        + form.get("custom", ["-f", "json"])
        + form["options"]
        + target
        + form.get("tail", [])
    )
    done = subprocess.run(
        [args.ruby, args.launcher] + command,
        cwd=directory,
        env=_environment(),
        input=form.get("source", SOURCE).encode(),
        capture_output=True,
        check=False,
        timeout=60,
    )
    _require(done.returncode in (0, 1, 2), "本家CLIが異常終了")
    reference = {
        "args": command,
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
    }
    if done.stdout.startswith(b"{"):
        reference["report"] = _report(done.stdout)
    traced = subprocess.run(
        [args.ruby, "-e", OPTIONS_TRACE, "--"] + command,
        cwd=directory,
        env=_environment(),
        input=form.get("source", SOURCE).encode(),
        capture_output=True,
        check=False,
        timeout=60,
    )
    _require(traced.returncode == 0 and not traced.stderr, "Options.parseの観測に失敗")
    trace = json.loads(traced.stdout)
    _require(
        (trace["ruby"], trace["parser"], trace["rubocop"]) == ("4.0.7", "3.3.12.0", "1.89.0"),
        "Options.parseの実版が不一致",
    )
    outputs = {}
    for name in ("first.json", "second.json"):
        path = directory / name
        outputs[name] = (
            {"raw_hex": path.read_bytes().hex(), "report": _report(path.read_bytes())}
            if path.exists()
            else None
        )
    _validate_outputs(outputs, "custom" in form)
    if form["coverage"] == "listing":
        # 一覧表示は検査前に戻る。JSON の未完走と混同せず、実パス列と無警告を照合する。
        expected = "".join(name + "\r\n" for name in inputs).encode()
        _require(done.returncode == 0, "対象一覧の取得に失敗")
        _require(done.stdout == expected and not done.stderr, "対象一覧または警告が不一致")
    elif form["coverage"] == "full_json":
        _require(done.returncode in (0, 1), "正常対照が失敗")
        reports = ([reference["report"]] if "report" in reference else []) + [
            value["report"] for value in outputs.values() if value is not None
        ]
        _require(reports, "対象JSONが無い")
        for report in reports:
            _validate_report(report, list(inputs))
        if not inputs:
            _require(not done.stderr, "対象ゼロでcop名警告が出た")
    else:
        _require(done.returncode == 2 and done.stderr, "否定対照が拒否されなかった")
        if form["coverage"] == "rejection_envelope":
            _require(not done.stdout, "parse/compatibility拒否にstdoutがある")
    for name, source in inputs.items():
        _require((directory / name).read_bytes() == source.encode(), "非補正原本が変わった")
    result = {
        "id": form["id"] + "_" + mode,
        "mode": mode,
        "coverage": form["coverage"],
        "residual_issue": 118 if form["coverage"] == "rejection_only" else None,
        "inputs": inputs,
        "stdin_source": form.get("source", SOURCE),
        "reference": reference,
        "options_trace": trace,
        "outputs": outputs,
    }
    if "directories" in form:
        result["directories"] = form["directories"]
    return result


def main():
    """候補を読まず、必要な全条件が完了した場合だけfixtureを保存する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "launcher", "work", "out"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--set", choices=("csv", "badge", "directory"), default="csv")
    args = parser.parse_args()
    forms = {"csv": _forms, "badge": _badge_forms, "directory": _directory_forms}[args.set]()
    jobs = [
        (index, form, mode)
        for index, form in enumerate(forms)
        for mode in form.get("modes", ["file", "stdin"])
    ]
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        cases = list(pool.map(lambda job: _record(job, args), jobs))
    _require(
        len(cases) == {"csv": 114, "badge": 32, "directory": 6}[args.set],
        "有限行列の件数が変わった",
    )
    fixture = {
        "reference_version": {
            "rubocop": "1.89.0",
            "parser": "3.3.12.0",
            "ruby": "4.0.7",
            "platform": "Windows",
        },
        "yaml": YAML,
        "cases": cases,
    }
    Path(args.out).write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
    )
    print(f"recorded {len(cases)} cases")


if __name__ == "__main__":
    main()
